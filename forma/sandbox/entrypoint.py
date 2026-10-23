"""Runs one code-execution request, start to finish, and exits.

The forma runtime starts this container once per run (a Cloud Run Job
execution, or `docker run` locally) with a single environment variable,
`CODE_EXEC_MANIFEST_URL`: a short-lived signed URL to a JSON manifest that says
what to download, what to run and where to put the results. Everything else
this process can reach in the bucket is reached through the signed URLs in that
manifest; the container's own service account has no roles at all.

The trust boundary inside the container is a Unix user. This process runs as
root and holds the manifest (and so every signed URL) in memory only. The
command runs as `runner`, an unprivileged user with a scrubbed environment, so
the code it runs cannot read the manifest, cannot overwrite the logs or the
result, and cannot upload anything itself. Once the command is done, every
process owned by `runner` is killed *before* its outputs are read, so nothing
can swap a file for a symlink while it is being collected.

Only metadata is ever printed. The command's output goes to files, never to
this process's stdout/stderr, so none of it reaches the platform's logs.

Standard library only: this must keep working whatever the image's Python
packages are, and it runs as root, so it imports as little as possible.
"""

import json
import os
import pwd
import signal
import stat
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid

MANIFEST_ENV = "CODE_EXEC_MANIFEST_URL"
RUNNER_USER = "runner"
WORK_DIR = "/work"
OUT_DIR = "/work/out"
# Root-only: where the command's stdout/stderr are captured.
PRIVATE_DIR = "/run/sandbox"

MANIFEST_MAX_BYTES = 1024 * 1024
HTTP_TIMEOUT_SECONDS = 60
MAX_PATH_BYTES = 400
# Stops a directory tree with millions of entries from stalling collection.
MAX_WALK_ENTRIES = 10_000
# Keeps result.json small however many files were refused.
MAX_SKIPPED_REPORTED = 50
MANIFEST_VERSION = 1


class SandboxError(Exception):
    """A failure that is reported to the agent in `result.json`."""


def log(message):
    """Prints one line of metadata. Never pass it anything the command wrote."""
    print(f"sandbox: {message}", file=sys.stderr, flush=True)


# ---- paths -------------------------------------------------------------------


def validate_rel_path(path):
    """Returns `path` if it is a safe path relative to the work directory.

    Refuses absolute paths, `..` and `.` segments, empty segments, control
    characters and overlong names, rather than normalising them: a path the
    agent wrote that needs fixing is a mistake worth telling it about.
    """
    if not isinstance(path, str) or not path:
        raise SandboxError("a file path is empty")
    if len(path.encode("utf-8")) > MAX_PATH_BYTES:
        raise SandboxError(f"the file path '{path[:60]}…' is too long")
    if path.startswith("/"):
        raise SandboxError(f"'{path}' must be relative to the work directory")
    if any(ord(c) < 0x20 or ord(c) == 0x7F for c in path) or "\\" in path:
        raise SandboxError(f"'{path}' contains characters that are not allowed")
    for segment in path.split("/"):
        if segment in ("", ".", ".."):
            raise SandboxError(f"'{path}' is not a plain relative path")
    return path


# ---- media types -------------------------------------------------------------

_MAGIC = [
    (b"\x89PNG\r\n\x1a\n", "image/png"),
    (b"\xff\xd8\xff", "image/jpeg"),
    (b"GIF87a", "image/gif"),
    (b"GIF89a", "image/gif"),
    (b"%PDF-", "application/pdf"),
    (b"PK\x03\x04", "application/zip"),
]

XLSX_TYPE = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
PPTX_TYPE = "application/vnd.openxmlformats-officedocument.presentationml.presentation"

# The only zips that mean something, and only under these names.
_ZIP_BY_EXTENSION = {
    ".xlsx": XLSX_TYPE,
    ".pptx": PPTX_TYPE,
}

_TEXT_BY_EXTENSION = {
    ".csv": "text/csv",
    ".tsv": "text/tab-separated-values",
    ".json": "application/json",
    ".md": "text/markdown",
    ".html": "text/plain",  # never served as HTML
    ".svg": "text/plain",  # never rendered: SVG can carry script
}


def sniff_media_type(name, data):
    """Says what `data` is, judging by its content first and its name last.

    The name only refines text (a CSV is text either way); it never turns bytes
    into something they are not. Anything that is neither a known binary format
    nor valid UTF-8 is `application/octet-stream`.
    """
    if data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        return "image/webp"
    for magic, media_type in _MAGIC:
        if data.startswith(magic):
            # A zip is only ever a workbook or a deck, and only when it is
            # called one.
            if media_type == "application/zip":
                ext = os.path.splitext(name)[1].lower()
                return _ZIP_BY_EXTENSION.get(ext, media_type)
            return media_type
    try:
        data.decode("utf-8")
    except UnicodeDecodeError:
        return "application/octet-stream"
    if b"\x00" in data:
        return "application/octet-stream"
    ext = os.path.splitext(name)[1].lower()
    media_type = _TEXT_BY_EXTENSION.get(ext, "text/plain")
    if media_type.startswith("text/"):
        media_type += "; charset=utf-8"
    return media_type


# ---- HTTP --------------------------------------------------------------------


def http(method, url, body=None, headers=None, max_bytes=None, attempts=3):
    """Makes one request, retrying transient failures. Returns the body."""
    last_error = None
    for attempt in range(attempts):
        request = urllib.request.Request(url, data=body, method=method)
        for key, value in (headers or {}).items():
            request.add_header(key, value)
        try:
            with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT_SECONDS) as response:
                if max_bytes is None:
                    return response.read()
                data = response.read(max_bytes + 1)
                if len(data) > max_bytes:
                    raise SandboxError("a download was larger than allowed")
                return data
        except urllib.error.HTTPError as e:
            # Never log the URL: it is a credential.
            last_error = f"HTTP {e.code}"
            if e.code < 500 and e.code != 429:
                break
        except (urllib.error.URLError, TimeoutError, ConnectionError) as e:
            last_error = type(e).__name__
        time.sleep(1 + attempt * 2)
    raise SandboxError(f"{method} failed ({last_error})")


def multipart_body(fields, file_name, file_bytes, content_type):
    """Builds a `multipart/form-data` body; the file goes last, as GCS needs."""
    boundary = uuid.uuid4().hex
    parts = []
    for key, value in fields:
        parts.append(
            f'--{boundary}\r\nContent-Disposition: form-data; name="{key}"\r\n\r\n'
            f"{value}\r\n".encode("utf-8")
        )
    safe_name = file_name.replace('"', "_")
    parts.append(
        f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{safe_name}"\r\n'
        f"Content-Type: {content_type}\r\n\r\n".encode("utf-8")
    )
    parts.append(file_bytes)
    parts.append(f"\r\n--{boundary}--\r\n".encode("utf-8"))
    return b"".join(parts), f"multipart/form-data; boundary={boundary}"


def upload_output(policy, rel_path, data, media_type):
    """Uploads one output through the run's signed POST policy."""
    fields = [("key", policy["key_prefix"] + rel_path), ("Content-Type", media_type)]
    fields += sorted(policy["fields"].items())
    body, content_type = multipart_body(fields, rel_path.rsplit("/", 1)[-1], data, media_type)
    http("POST", policy["url"], body=body, headers={"Content-Type": content_type})


def put_signed(target, data, content_type):
    headers = dict(target.get("headers") or {})
    headers["Content-Type"] = content_type
    http("PUT", target["url"], body=data, headers=headers)


# ---- running -----------------------------------------------------------------


class Capture(threading.Thread):
    """Copies one of the command's streams to a file, up to `limit` bytes."""

    def __init__(self, stream, path, limit):
        super().__init__(daemon=True)
        self.stream = stream
        self.path = path
        self.limit = limit
        self.total = 0

    def run(self):
        with open(self.path, "wb") as f:
            while True:
                chunk = self.stream.read(64 * 1024)
                if not chunk:
                    break
                room = self.limit - self.total
                if room > 0:
                    f.write(chunk[:room])
                self.total += len(chunk)

    @property
    def truncated(self):
        return self.total > self.limit


def kill_everything_owned_by(uid, gid):
    """Kills every process `uid` owns, including any that escaped the group.

    `kill(-1)` from a process running as `uid` signals every process that user
    may signal: all of its own. Done a few times, so a process that was forking
    while the first round went out is caught by the next.
    """
    script = "import os, signal\ntry:\n    os.kill(-1, signal.SIGKILL)\nexcept OSError:\n    pass\n"
    for _ in range(3):
        subprocess.run(
            [sys.executable, "-I", "-c", script],
            user=uid,
            group=gid,
            extra_groups=[],
            env={},
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        time.sleep(0.1)


def run_command(command, timeout, uid, gid, log_limit):
    """Runs `command` as the runner user. Returns what happened."""
    env = {
        "PATH": "/usr/local/bin:/usr/bin:/bin",
        "HOME": WORK_DIR,
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "MPLBACKEND": "Agg",
        "MPLCONFIGDIR": "/tmp/matplotlib",
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONUNBUFFERED": "1",
        "OUT": OUT_DIR,
        "TMPDIR": "/tmp",
    }
    started = time.monotonic()
    proc = subprocess.Popen(
        ["/bin/bash", "-c", command],
        cwd=WORK_DIR,
        env=env,
        user=uid,
        group=gid,
        extra_groups=[],
        umask=0o022,
        start_new_session=True,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        close_fds=True,
    )
    stdout = Capture(proc.stdout, os.path.join(PRIVATE_DIR, "stdout.txt"), log_limit)
    stderr = Capture(proc.stderr, os.path.join(PRIVATE_DIR, "stderr.txt"), log_limit)
    stdout.start()
    stderr.start()

    timed_out = False
    try:
        exit_code = proc.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except OSError:
            pass
        exit_code = proc.wait()
    duration_ms = int((time.monotonic() - started) * 1000)

    # Whatever the command left running in the background goes too, before a
    # single output is read.
    kill_everything_owned_by(uid, gid)
    stdout.join(timeout=10)
    stderr.join(timeout=10)

    return {
        "exit_code": exit_code,
        "timed_out": timed_out,
        "duration_ms": duration_ms,
        "stdout": stdout,
        "stderr": stderr,
    }


# ---- inputs and outputs ------------------------------------------------------


def prepare_work_dir(uid, gid):
    os.makedirs(PRIVATE_DIR, mode=0o700, exist_ok=True)
    os.chmod(PRIVATE_DIR, 0o700)
    for path in (WORK_DIR, OUT_DIR):
        os.makedirs(path, exist_ok=True)
        os.chown(path, uid, gid)
        os.chmod(path, 0o755)


def download_inputs(inputs, uid, gid, max_file_bytes, max_total_bytes):
    """Writes every input into the work directory, owned by the runner."""
    total = 0
    for item in inputs:
        rel = validate_rel_path(item["path"])
        data = http("GET", item["url"], max_bytes=max_file_bytes)
        total += len(data)
        if total > max_total_bytes:
            raise SandboxError("the inputs are larger than allowed in total")
        try:
            write_input(os.path.join(WORK_DIR, rel), data, uid, gid)
        except OSError:
            raise SandboxError(f"could not place the input '{rel}' (is it given twice?)")


def write_input(target, data, uid, gid):
    parent = os.path.dirname(target)
    os.makedirs(parent, exist_ok=True)
    # Every directory between the work dir and the file belongs to the runner,
    # so the command can add files next to its inputs.
    d = parent
    while d.startswith(WORK_DIR + "/"):
        os.chown(d, uid, gid)
        d = os.path.dirname(d)
    # O_EXCL: two inputs with one path is a mistake, not an overwrite.
    fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o755)
    with os.fdopen(fd, "wb") as f:
        f.write(data)
    os.chown(target, uid, gid)


def read_owned_regular_file(path, uid, max_bytes):
    """Reads `path` only if it is a plain file the runner made.

    Symlinks are not followed, and a file with more than one link, or not owned
    by the runner, is refused: that is how a hard link to something only root
    can read would otherwise be smuggled out. Returns `(data, reason)`.
    """
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    except OSError:
        return None, "not a regular file"
    with os.fdopen(fd, "rb") as f:
        info = os.fstat(f.fileno())
        if not stat.S_ISREG(info.st_mode):
            return None, "not a regular file"
        if info.st_uid != uid or info.st_nlink != 1:
            return None, "not a file the command created"
        if info.st_size > max_bytes:
            return None, f"larger than {max_bytes} bytes"
        data = f.read(max_bytes + 1)
        if len(data) > max_bytes:
            return None, f"larger than {max_bytes} bytes"
        return data, None


def collect_outputs(out_dir, uid, max_files, max_file_bytes, max_total_bytes):
    """Finds the files to upload. Returns `(files, skipped)`.

    Must only run once every runner process is dead: nothing may be changing
    the tree while it is walked.
    """
    files, skipped = [], []
    try:
        top = os.lstat(out_dir)
    except OSError:
        return files, skipped
    if not stat.S_ISDIR(top.st_mode):
        skipped.append({"path": "out", "reason": "out is not a directory"})
        return files, skipped

    total, seen = 0, 0
    for root, dirs, names in os.walk(out_dir, followlinks=False):
        dirs.sort()
        for name in sorted(names):
            seen += 1
            if seen > MAX_WALK_ENTRIES:
                skipped.append({"path": "…", "reason": "too many files"})
                return files, skipped
            full = os.path.join(root, name)
            rel = os.path.relpath(full, out_dir)
            try:
                validate_rel_path(rel)
            except SandboxError:
                skipped.append({"path": rel[:MAX_PATH_BYTES], "reason": "unsupported file name"})
                continue
            if len(files) >= max_files:
                skipped.append({"path": rel, "reason": f"more than {max_files} output files"})
                continue
            data, reason = read_owned_regular_file(full, uid, max_file_bytes)
            if reason:
                skipped.append({"path": rel, "reason": reason})
                continue
            if total + len(data) > max_total_bytes:
                skipped.append({"path": rel, "reason": "outputs are larger than allowed in total"})
                continue
            total += len(data)
            files.append((rel, data))
    return files, skipped


def read_private(path):
    try:
        with open(path, "rb") as f:
            return f.read()
    except OSError:
        return b""


# ---- main --------------------------------------------------------------------


def execute(manifest):
    """Runs the manifest. Returns the result and the captured logs."""
    if manifest.get("version") != MANIFEST_VERSION:
        raise SandboxError("unsupported manifest version")
    limits = manifest["limits"]
    runner = pwd.getpwnam(RUNNER_USER)
    uid, gid = runner.pw_uid, runner.pw_gid

    prepare_work_dir(uid, gid)
    download_inputs(
        manifest["inputs"], uid, gid, limits["max_input_file_bytes"], limits["max_input_total_bytes"]
    )
    run = run_command(
        manifest["command"], manifest["timeout_seconds"], uid, gid, limits["max_log_bytes"]
    )
    files, skipped = collect_outputs(
        OUT_DIR,
        uid,
        limits["max_output_files"],
        limits["max_output_file_bytes"],
        limits["max_output_total_bytes"],
    )

    if len(skipped) > MAX_SKIPPED_REPORTED:
        more = len(skipped) - MAX_SKIPPED_REPORTED
        skipped = skipped[:MAX_SKIPPED_REPORTED]
        skipped.append({"path": "…", "reason": f"{more} more files skipped"})

    outputs = []
    for rel, data in files:
        media_type = sniff_media_type(rel, data)
        upload_output(manifest["outputs"], rel, data, media_type)
        outputs.append({"path": "out/" + rel, "size": len(data), "media_type": media_type})

    if run["timed_out"]:
        status = "timeout"
    elif run["exit_code"] == 0:
        status = "ok"
    else:
        status = "failed"
    return {
        "version": MANIFEST_VERSION,
        "status": status,
        "exit_code": run["exit_code"],
        "timed_out": run["timed_out"],
        "duration_ms": run["duration_ms"],
        "stdout_bytes": run["stdout"].total,
        "stdout_truncated": run["stdout"].truncated,
        "stderr_bytes": run["stderr"].total,
        "stderr_truncated": run["stderr"].truncated,
        "outputs": outputs,
        "skipped_outputs": skipped,
    }


def main():
    url = os.environ.pop(MANIFEST_ENV, None)
    if not url:
        log(f"{MANIFEST_ENV} is not set")
        return 1
    try:
        manifest = json.loads(http("GET", url, max_bytes=MANIFEST_MAX_BYTES))
    except (SandboxError, ValueError) as e:
        log(f"could not load the manifest: {e}")
        return 1

    try:
        result = execute(manifest)
    except SandboxError as e:
        # Something the agent can be told about (a bad path, an input that
        # would not download). The run did not happen.
        result = {"version": MANIFEST_VERSION, "status": "error", "error": str(e)}

    log(f"status={result['status']} exit_code={result.get('exit_code')} "
        f"outputs={len(result.get('outputs', []))}")

    logs = manifest["logs"]
    try:
        put_signed(logs["stdout"], read_private(os.path.join(PRIVATE_DIR, "stdout.txt")),
                   "text/plain; charset=utf-8")
        put_signed(logs["stderr"], read_private(os.path.join(PRIVATE_DIR, "stderr.txt")),
                   "text/plain; charset=utf-8")
        # Last: its presence is what tells the runtime the rest is in place.
        put_signed(logs["result"], json.dumps(result).encode("utf-8"), "application/json")
    except SandboxError as e:
        log(f"could not upload the results: {e}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

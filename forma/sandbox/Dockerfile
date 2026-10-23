# The code-execution sandbox: one container per run, started by the forma
# runtime as a Cloud Run Job execution (or `docker run` locally). See
# entrypoint.py for what happens inside, and infra/code-execution.tf for the
# job, its empty service account and its network without internet.
FROM python:3.12-slim-bookworm

# bash, coreutils, grep and sed come with the base image; awk is mawk there, so
# gawk is added for the text-processing use case.
RUN apt-get update \
    && apt-get install -y --no-install-recommends gawk \
    && rm -rf /var/lib/apt/lists/*

COPY requirements.txt /tmp/requirements.txt
RUN pip install --no-cache-dir -r /tmp/requirements.txt && rm /tmp/requirements.txt

# The command runs as this user; the entrypoint stays root so the code it runs
# cannot reach the manifest's signed URLs or tamper with the results.
RUN useradd --uid 10001 --no-create-home --home-dir /work --shell /usr/sbin/nologin runner

COPY entrypoint.py /opt/sandbox/entrypoint.py
RUN chown -R root:root /opt/sandbox && chmod 0700 /opt/sandbox && chmod 0600 /opt/sandbox/entrypoint.py

USER root
# -I: isolated mode, so nothing in the environment or the working directory can
# change what the root process imports.
ENTRYPOINT ["python3", "-I", "/opt/sandbox/entrypoint.py"]

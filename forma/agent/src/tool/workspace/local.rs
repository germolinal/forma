//! A [`DocStore`] in a folder on the machine running the agent.
//!
//! Chosen by whoever runs the server, with `workspaces: {store: local, dir}` in
//! its `runtime.yaml`, instead of Cloud Storage: the same agent, unchanged,
//! keeps its workspaces in a bucket in production and in a folder on a laptop.
//! The folder plays the bucket. A workspace's documents are the files under
//! `{dir}/{workspace_id}/`, at the path the Cloud Storage store would give the
//! object of the same name.
//!
//! Paths come from agents, and a file system — unlike an object store — gives
//! `..` and links a meaning. So every path is checked segment by segment, and
//! the store never follows a symbolic link: one inside the folder is not
//! listed, read, written or deleted through. Nothing here reaches outside the
//! workspace's own folder. (The folder `dir` names itself may be a link: it is
//! the operator's.)
//!
//! It cannot sign URLs, so code execution — which hands its sandbox signed URLs
//! — needs the Cloud Storage store.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use protocols::{
    AttachedDoc, DocMeta, DocStore, FormaError, SignedAccess, SignedRequest, SignedUpload,
};

use super::gcs::{GcsDocConfig, loose_name};

/// What [`DocStore::write`] stores: the agent writes text, and only text.
const TEXT_TYPE: &str = "text/plain; charset=utf-8";
/// How much of a file is read to tell what it is.
const SNIFF_BYTES: usize = 512;
/// Never listed: the Finder leaves these in every folder it opens.
const IGNORED_NAMES: &[&str] = &[".DS_Store"];

/// One workspace's folder.
#[derive(Debug, Clone)]
pub struct LocalDocStore {
    /// The folder the server keeps workspaces in (`dir`).
    root: PathBuf,
    /// The workspace's folder under `root`, segment by segment.
    prefix: Vec<String>,
}

impl LocalDocStore {
    /// The store for the workspace `config` names, under `root`. Only the
    /// workspace's ID is used: there is one folder, so there are no buckets
    /// to choose between.
    pub fn new(root: &Path, config: &GcsDocConfig) -> Result<Self, FormaError> {
        let id = config.workspace_id.trim_matches('/');
        let prefix = if id.is_empty() {
            Vec::new()
        } else {
            segments(id)?
        };
        Ok(Self {
            root: root.to_path_buf(),
            prefix,
        })
    }

    /// Where `path` lives, as segments under `root`.
    fn locate(&self, path: &str) -> Result<Vec<String>, FormaError> {
        let path = path.strip_prefix("./").unwrap_or(path);
        let path = path.trim_start_matches('/');
        let mut all = self.prefix.clone();
        all.extend(segments(path)?);
        Ok(all)
    }

    /// The file at `segments`, if it is there: every folder on the way a real
    /// folder and the file a real file. A link anywhere is refused.
    fn find(
        &self,
        segments: &[String],
    ) -> Result<Option<(PathBuf, std::fs::Metadata)>, FormaError> {
        let mut current = self.root.clone();
        for (i, segment) in segments.iter().enumerate() {
            current.push(segment);
            let meta = match std::fs::symlink_metadata(&current) {
                Ok(meta) => meta,
                Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(io_error(&current, e)),
            };
            if meta.file_type().is_symlink() {
                return Err(link_refused(segments));
            }
            let last = i + 1 == segments.len();
            match (last, meta.is_file(), meta.is_dir()) {
                (true, true, _) => return Ok(Some((current, meta))),
                (false, _, true) => {}
                _ => return Ok(None),
            }
        }
        Ok(None)
    }

    /// The workspace's own folder, if it exists and is a real one.
    fn folder(&self) -> Result<Option<PathBuf>, FormaError> {
        let mut current = self.root.clone();
        if !current.is_dir() {
            return Ok(None);
        }
        for segment in &self.prefix {
            current.push(segment);
            match std::fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(link_refused(&self.prefix));
                }
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => return Ok(None),
                Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(io_error(&current, e)),
            }
        }
        Ok(Some(current))
    }

    /// Makes the folders the file at `segments` goes in, without going through
    /// a link, and says where the file goes.
    fn prepare(&self, segments: &[String]) -> Result<PathBuf, FormaError> {
        std::fs::create_dir_all(&self.root).map_err(|e| io_error(&self.root, e))?;
        let (file, folders) = segments
            .split_last()
            .ok_or_else(|| FormaError::Input("a document path cannot be empty".to_string()))?;
        let mut current = self.root.clone();
        for segment in folders {
            current.push(segment);
            match std::fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => return Err(link_refused(segments)),
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    return Err(FormaError::Input(format!(
                        "'{segment}' is a document, not a folder"
                    )));
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    std::fs::create_dir(&current).map_err(|e| io_error(&current, e))?
                }
                Err(e) => return Err(io_error(&current, e)),
            }
        }
        current.push(file);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => Err(link_refused(segments)),
            Ok(meta) if meta.is_dir() => Err(FormaError::Input(format!("'{file}' is a folder"))),
            _ => Ok(current),
        }
    }

    /// The file for `path`, matching its name leniently when the exact one is
    /// not there — see `GcsDocStore::find_by_loose_name`, which this mirrors.
    fn resolve(&self, path: &str) -> Result<(String, PathBuf, std::fs::Metadata), FormaError> {
        if let Some((file, meta)) = self.find(&self.locate(path)?)? {
            return Ok((path.to_string(), file, meta));
        }
        let wanted = loose_name(path);
        let mut matches: Vec<String> = self
            .list_files("")?
            .into_iter()
            .map(|d| d.path)
            .filter(|candidate| loose_name(candidate) == wanted)
            .collect();
        matches.dedup();
        let actual = match matches.len() {
            0 => {
                return Err(FormaError::Input(format!(
                    "there is no document at '{path}' in this workspace; list it to see what is there"
                )));
            }
            1 => matches.remove(0),
            _ => {
                return Err(FormaError::Input(format!(
                    "'{path}' could be any of {}; ask for one by its exact name",
                    matches.join(", ")
                )));
            }
        };
        match self.find(&self.locate(&actual)?)? {
            Some((file, meta)) => Ok((actual, file, meta)),
            None => Err(FormaError::Input(format!(
                "there is no document at '{path}' any more"
            ))),
        }
    }

    /// Every document whose path starts with `prefix`, sorted.
    fn list_files(&self, prefix: &str) -> Result<Vec<DocMeta>, FormaError> {
        let Some(folder) = self.folder()? else {
            return Ok(Vec::new());
        };
        let mut docs = Vec::new();
        let mut pending = vec![(folder, String::new())];
        while let Some((dir, rel)) = pending.pop() {
            let entries = std::fs::read_dir(&dir).map_err(|e| io_error(&dir, e))?;
            for entry in entries {
                let entry = entry.map_err(|e| io_error(&dir, e))?;
                // Names that are not UTF-8 cannot be asked for by an agent.
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if IGNORED_NAMES.contains(&name.as_str()) {
                    continue;
                }
                let path = if rel.is_empty() {
                    name
                } else {
                    format!("{rel}/{name}")
                };
                // `file_type` does not follow links, so a link is neither a
                // folder to walk into nor a file to list.
                let kind = entry.file_type().map_err(|e| io_error(&entry.path(), e))?;
                if kind.is_dir() {
                    pending.push((entry.path(), path));
                } else if kind.is_file() && path.starts_with(prefix) {
                    let size = entry.metadata().map(|m| m.len()).ok();
                    docs.push(DocMeta {
                        media_type: Some(media_type_of(&path, &head_of(&entry.path()))),
                        path,
                        title: None,
                        size_bytes: size,
                    });
                }
            }
        }
        docs.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(docs)
    }

    fn read_file(&self, path: &str) -> Result<(String, Vec<u8>, String), FormaError> {
        let (actual, file, _) = self.resolve(path)?;
        let bytes = std::fs::read(&file).map_err(|e| io_error(&file, e))?;
        let media_type = media_type_of(&actual, &bytes[..bytes.len().min(SNIFF_BYTES)]);
        Ok((actual, bytes, media_type))
    }

    fn write_file(&self, path: &str, bytes: &[u8]) -> Result<u64, FormaError> {
        let file = self.prepare(&self.locate(path)?)?;
        std::fs::write(&file, bytes).map_err(|e| io_error(&file, e))?;
        Ok(bytes.len() as u64)
    }
}

/// Splits a relative path into segments, refusing what would mean something
/// to a file system: `..`, `.`, empty segments, backslashes, control
/// characters.
fn segments(path: &str) -> Result<Vec<String>, FormaError> {
    let bad = |why: &str| Err(FormaError::Input(format!("'{path}': {why}")));
    if path.is_empty() {
        return bad("a document path cannot be empty");
    }
    if path.chars().any(|c| c.is_control() || c == '\\') {
        return bad("the path contains characters that are not allowed");
    }
    let parts: Vec<String> = path.split('/').map(str::to_string).collect();
    if parts.iter().any(|s| s.is_empty() || s == "." || s == "..") {
        return bad("the path must be a plain relative path, without `.` or `..`");
    }
    Ok(parts)
}

fn link_refused(segments: &[String]) -> FormaError {
    FormaError::Input(format!(
        "'{}' goes through a link, which this store does not follow",
        segments.join("/")
    ))
}

/// I/O trouble is the runtime's, not the agent's; the message still names the
/// file, which is on the operator's own machine.
fn io_error(path: &Path, e: std::io::Error) -> FormaError {
    FormaError::Execution(format!("local workspace store: {}: {e}", path.display()))
}

/// The first bytes of a file, to tell what it is; empty when unreadable.
fn head_of(path: &Path) -> Vec<u8> {
    use std::io::Read;
    let mut head = Vec::with_capacity(SNIFF_BYTES);
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(SNIFF_BYTES as u64).read_to_end(&mut head);
    }
    head
}

/// What a document is, judged by its content first and its name last: the
/// name only refines text, it never turns bytes into something they are not.
/// A folder keeps no types of its own, so this stands in for what Cloud
/// Storage would have recorded.
pub(crate) fn media_type_of(name: &str, head: &[u8]) -> String {
    const MAGIC: &[(&[u8], &str)] = &[
        (b"\x89PNG\r\n\x1a\n", "image/png"),
        (b"\xff\xd8\xff", "image/jpeg"),
        (b"GIF87a", "image/gif"),
        (b"GIF89a", "image/gif"),
        (b"%PDF-", "application/pdf"),
    ];
    if let Some((_, kind)) = MAGIC.iter().find(|(magic, _)| head.starts_with(magic)) {
        return kind.to_string();
    }
    if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return "image/webp".to_string();
    }
    // A zip is only ever a workbook or a deck here, and only when it is
    // called one.
    if head.starts_with(b"PK\x03\x04") {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".xlsx") {
            return super::XLSX_TYPE.to_string();
        }
        if lower.ends_with(".pptx") {
            return super::PPTX_TYPE.to_string();
        }
    }
    // A head cut through a multi-byte character is still text.
    let utf8 = match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none(),
    };
    if !utf8 || head.contains(&0) {
        return "application/octet-stream".to_string();
    }
    let extension = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("md" | "markdown") => "text/markdown; charset=utf-8",
        Some("csv") => "text/csv; charset=utf-8",
        Some("json") => "application/json",
        Some("html" | "htm") => "text/html; charset=utf-8",
        _ => TEXT_TYPE,
    }
    .to_string()
}

/// Runs file-system work off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, FormaError> + Send + 'static,
) -> Result<T, FormaError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| FormaError::Execution(format!("local workspace store: {e}")))?
}

#[async_trait]
impl DocStore for LocalDocStore {
    async fn list(&self, prefix: Option<&str>) -> Result<Vec<DocMeta>, FormaError> {
        let store = self.clone();
        let prefix = prefix.unwrap_or("").to_string();
        blocking(move || store.list_files(&prefix)).await
    }

    async fn attach(&self, path: &str) -> Result<AttachedDoc, FormaError> {
        let store = self.clone();
        let wanted = path.to_string();
        let (_, bytes, media_type) = blocking(move || store.read_file(&wanted)).await?;
        Ok(AttachedDoc {
            path: path.to_string(),
            media_type,
            bytes,
        })
    }

    async fn write(&self, path: &str, content: &str) -> Result<DocMeta, FormaError> {
        self.write_bytes(path, content.as_bytes().to_vec(), TEXT_TYPE)
            .await
    }

    async fn stat(&self, path: &str) -> Result<Option<DocMeta>, FormaError> {
        let store = self.clone();
        let wanted = path.to_string();
        blocking(move || {
            let Some((file, meta)) = store.find(&store.locate(&wanted)?)? else {
                return Ok(None);
            };
            Ok(Some(DocMeta {
                media_type: Some(media_type_of(&wanted, &head_of(&file))),
                path: wanted,
                title: None,
                size_bytes: Some(meta.len()),
            }))
        })
        .await
    }

    async fn write_bytes(
        &self,
        path: &str,
        bytes: Vec<u8>,
        media_type: &str,
    ) -> Result<DocMeta, FormaError> {
        let store = self.clone();
        let wanted = path.to_string();
        let size = blocking(move || store.write_file(&wanted, &bytes)).await?;
        Ok(DocMeta {
            path: path.to_string(),
            title: None,
            size_bytes: Some(size),
            media_type: Some(media_type.to_string()),
        })
    }

    async fn delete(&self, path: &str) -> Result<(), FormaError> {
        let store = self.clone();
        let wanted = path.to_string();
        blocking(move || match store.find(&store.locate(&wanted)?)? {
            Some((file, _)) => std::fs::remove_file(&file).map_err(|e| io_error(&file, e)),
            None => Ok(()),
        })
        .await
    }

    async fn signed_url(
        &self,
        _path: &str,
        _access: SignedAccess,
        _expires: Duration,
    ) -> Result<SignedRequest, FormaError> {
        Err(cannot_sign())
    }

    async fn signed_upload(
        &self,
        _prefix: &str,
        _max_bytes: u64,
        _expires: Duration,
    ) -> Result<SignedUpload, FormaError> {
        Err(cannot_sign())
    }
}

fn cannot_sign() -> FormaError {
    FormaError::Input(
        "the local workspace store cannot sign URLs, which code execution needs \
         (keep workspaces in Cloud Storage: `workspaces: {store: gcs}` in runtime.yaml)"
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder for one test, removed when dropped.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "forma-local-store-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn store(&self, workspace_id: &str) -> LocalDocStore {
            LocalDocStore::new(
                &self.0,
                &GcsDocConfig {
                    bucket: "ignored-bucket".to_string(),
                    workspace_id: workspace_id.to_string(),
                },
            )
            .unwrap()
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn test_documents_round_trip_at_the_bucket_path() {
        let root = TempRoot::new();
        let store = root.store("kb-1");
        store.write("notes/plan.md", "# Plan").await.unwrap();
        store
            .write_bytes("chart.png", b"\x89PNG\r\n\x1a\nrest".to_vec(), "image/png")
            .await
            .unwrap();

        // Where the Cloud Storage store would put the objects: `{id}/{path}`.
        assert!(root.0.join("kb-1/notes/plan.md").is_file());

        let read = store.attach("notes/plan.md").await.unwrap();
        assert_eq!(read.bytes, b"# Plan");
        let listed = store.list(None).await.unwrap();
        let paths: Vec<&str> = listed.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths, vec!["chart.png", "notes/plan.md"]);
        assert_eq!(listed[0].media_type.as_deref(), Some("image/png"));
        assert_eq!(
            listed[1].media_type.as_deref(),
            Some("text/markdown; charset=utf-8")
        );
        assert_eq!(store.list(Some("notes/")).await.unwrap().len(), 1);

        assert_eq!(
            store.attach("chart.png").await.unwrap().media_type,
            "image/png"
        );

        let stat = store.stat("notes/plan.md").await.unwrap().unwrap();
        assert_eq!(stat.size_bytes, Some(6));
        assert!(store.stat("missing.md").await.unwrap().is_none());

        store.delete("notes/plan.md").await.unwrap();
        store.delete("notes/plan.md").await.unwrap();
        assert!(store.stat("notes/plan.md").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_workspaces_do_not_see_each_other() {
        let root = TempRoot::new();
        root.store("a").write("secret.md", "a").await.unwrap();
        let b = root.store("b");
        assert!(b.list(None).await.unwrap().is_empty());
        assert!(b.attach("secret.md").await.is_err());
        assert!(b.attach("../a/secret.md").await.is_err());
    }

    #[tokio::test]
    async fn test_paths_cannot_leave_the_workspace() {
        let root = TempRoot::new();
        std::fs::write(root.0.join("outside.txt"), "outside").unwrap();
        let store = root.store("kb");
        for bad in [
            "../outside.txt",
            "a/../../outside.txt",
            "a//b",
            "./a/./b",
            "a\\b",
            "a\nb",
        ] {
            assert!(store.attach(bad).await.is_err(), "read {bad:?}");
            assert!(store.write(bad, "x").await.is_err(), "write {bad:?}");
            assert!(store.delete(bad).await.is_err(), "delete {bad:?}");
        }
        // A leading slash means the workspace's root, as in Cloud Storage.
        store.write("/inside.txt", "x").await.unwrap();
        assert!(root.0.join("kb/inside.txt").is_file());
        assert_eq!(
            std::fs::read_to_string(root.0.join("outside.txt")).unwrap(),
            "outside"
        );

        // A workspace ID is held to the same rules.
        for bad in ["..", "../other", "a/../b"] {
            assert!(
                LocalDocStore::new(
                    &root.0,
                    &GcsDocConfig {
                        bucket: String::new(),
                        workspace_id: bad.to_string(),
                    }
                )
                .is_err(),
                "{bad}"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_links_are_never_followed() {
        let root = TempRoot::new();
        let outside = TempRoot::new();
        std::fs::write(outside.0.join("secret.txt"), "secret").unwrap();
        let store = root.store("kb");
        store.write("ok.txt", "fine").await.unwrap();
        let kb = root.0.join("kb");
        std::os::unix::fs::symlink(outside.0.join("secret.txt"), kb.join("file-link.txt")).unwrap();
        std::os::unix::fs::symlink(&outside.0, kb.join("dir-link")).unwrap();

        // Not listed, not read, not written or deleted through.
        let listed: Vec<String> = store
            .list(None)
            .await
            .unwrap()
            .into_iter()
            .map(|d| d.path)
            .collect();
        assert_eq!(listed, vec!["ok.txt"]);
        assert!(store.attach("file-link.txt").await.is_err());
        assert!(store.attach("dir-link/secret.txt").await.is_err());
        assert!(store.write("file-link.txt", "overwritten").await.is_err());
        assert!(store.write("dir-link/new.txt", "planted").await.is_err());
        assert!(store.delete("dir-link/secret.txt").await.is_err());
        assert_eq!(
            std::fs::read_to_string(outside.0.join("secret.txt")).unwrap(),
            "secret"
        );
        assert!(!outside.0.join("new.txt").exists());

        // A workspace whose own folder is a link is refused too.
        std::os::unix::fs::symlink(&outside.0, root.0.join("linked-kb")).unwrap();
        assert!(root.store("linked-kb").list(None).await.is_err());
    }

    #[tokio::test]
    async fn test_names_are_matched_leniently_on_a_miss() {
        let root = TempRoot::new();
        let store = root.store("kb");
        // A narrow no-break space, as macOS puts in screenshot names.
        store.write("Shot 3.02\u{202f}PM.md", "x").await.unwrap();
        let read = store.attach("shot 3.02 pm.md").await.unwrap();
        assert_eq!(read.bytes, b"x");
    }

    #[tokio::test]
    async fn test_it_cannot_sign() {
        let root = TempRoot::new();
        let err = root
            .store("kb")
            .signed_url("x", SignedAccess::Read, Duration::from_secs(60))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("store: gcs"));
    }

    #[test]
    fn test_media_type_is_judged_by_content() {
        assert_eq!(media_type_of("a.txt", b"%PDF-1.7"), "application/pdf");
        assert_eq!(media_type_of("a.png", b"hello"), TEXT_TYPE);
        assert_eq!(media_type_of("a.csv", b"a,b"), "text/csv; charset=utf-8");
        assert_eq!(
            media_type_of("a.bin", b"\x00\x01"),
            "application/octet-stream"
        );
        // Cut through "ñ": still text.
        assert_eq!(media_type_of("a.txt", &"añ".as_bytes()[..2]), TEXT_TYPE);
        assert_eq!(media_type_of("empty", b""), TEXT_TYPE);
    }
}

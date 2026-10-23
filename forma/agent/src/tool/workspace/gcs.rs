//! A [`DocStore`] backed by a Google Cloud Storage bucket.
//!
//! Documents are plain objects under an (optional) prefix that scopes one
//! knowledgebase within a bucket. Authentication uses Google Cloud
//! application-default credentials, injected at call time — exactly like the
//! Vertex AI Search tool — so no API key env var is required.
//!
//! GCS has no server-side content search, so [`GcsDocStore::search`] lists the
//! objects and scans a bounded number of them client-side. That is intentional:
//! it keeps the corpus a dumb bag of bytes with zero indexing infrastructure,
//! which is the right trade-off for the small, curated knowledgebases this is
//! aimed at. For large corpora, prefer a backend that can search server-side.

use std::time::Duration;

use async_trait::async_trait;
use protocols::{
    AttachedDoc, DocMeta, DocStore, FormaError, SignedAccess, SignedRequest, SignedUpload,
};
use serde::{Deserialize, Serialize};

use super::default_workspace_bucket;
use serde_json::Value;

/// OAuth scope needed to read objects from a bucket.
#[cfg(not(feature = "airtight"))]
const STORAGE_READ_SCOPE: &str = "https://www.googleapis.com/auth/devstorage.read_only";

/// OAuth scope needed to create/overwrite objects in a bucket.
#[cfg(not(feature = "airtight"))]
const STORAGE_WRITE_SCOPE: &str = "https://www.googleapis.com/auth/devstorage.read_write";

/// What an attached document is called when the store records no type of its
/// own for it.
const DEFAULT_MEDIA_TYPE: &str = "application/octet-stream";

/// What everything written through [`GcsDocStore::write`] is stored as: the
/// agent writes text, and only text.
const AGENT_TEXT_TYPE: &str = "text/plain; charset=utf-8";

/// Configuration for a Google Cloud Storage-backed knowledgebase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcsDocConfig {
    /// The bucket that holds the documents.
    #[serde(
        default = "default_workspace_bucket",
        skip_serializing_if = "String::is_empty"
    )]
    pub bucket: String,

    /// An optional object-name prefix scoping this knowledgebase within the
    /// bucket (e.g. `handbook/`). Paths reported to the agent are relative to
    /// this prefix.
    #[serde(default)]
    pub workspace_id: String,
}

impl GcsDocConfig {
    /// Validates the configuration.
    pub fn check(&self) -> Result<(), FormaError> {
        if self.bucket.trim().is_empty() {
            // no bucket? check env var
            protocols::envar::expect_env_var("WORKSPACES_BUCKET")?;
        }
        Ok(())
    }

    pub(crate) fn knowledgebase_id(&self) -> String {
        match (self.bucket.is_empty(), self.workspace_id().is_empty()) {
            (_, true) => self.bucket.clone(),
            // A local store has no bucket to name, and needs none.
            (true, false) => self.workspace_id(),
            (false, false) => format!("{}/{}", self.bucket, self.workspace_id()),
        }
    }

    /// Turns a knowledgebase-relative path into a full object name.
    pub(crate) fn object_name(&self, path: &str) -> String {
        // remove ./ or / prefixes
        let path = path.strip_prefix("./").unwrap_or(path);
        let path = path.strip_prefix("/").unwrap_or(path);
        format!("{}/{}", self.workspace_id(), path)
    }

    /// The prefix as a plain `&str` (empty when unset).
    fn workspace_id(&self) -> String {
        let ws_id = &self.workspace_id;
        let ws_id = ws_id.strip_suffix("/").unwrap_or(ws_id);
        ws_id.to_string()
    }
}

/// A live [`DocStore`] over a GCS bucket.
#[derive(Debug, Clone)]
pub struct GcsDocStore {
    config: GcsDocConfig,
}

impl GcsDocStore {
    /// Builds a store from its configuration. Credentials are resolved lazily,
    /// per request.
    pub fn new(config: GcsDocConfig) -> Self {
        Self { config }
    }

    /// Turns a knowledgebase-relative path into a full object name.
    fn object_name(&self, path: &str) -> String {
        self.config.object_name(path)
    }

    /// Describes one listed object, or `None` when it is not a document of
    /// this store's under `extra`.
    ///
    /// The media type is what the object was stored under, which is how an
    /// agent tells a note it can read from a picture it has to attach.
    fn doc_meta(&self, item: &Value, extra: &str) -> Option<DocMeta> {
        let name = item.get("name")?.as_str()?;
        // Skip "directory placeholder" objects.
        if name.ends_with('/') {
            return None;
        }
        let path = self.relative_path(name);
        if !path.starts_with(extra) {
            return None;
        }
        Some(DocMeta {
            path,
            title: None,
            size_bytes: item
                .get("size")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<u64>().ok()),
            media_type: item
                .get("contentType")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        })
    }

    /// Turns a full object name back into a knowledgebase-relative path.
    fn relative_path(&self, object: &str) -> String {
        let prefix = format!("{}/", self.config.workspace_id());
        object.strip_prefix(&prefix).unwrap_or(object).to_string()
    }
}

/// A document's name with the differences that are invisible on screen taken
/// out: every kind of space becomes an ordinary one, and case is ignored.
///
/// Used only to recover from a miss, never to store anything.
pub(super) fn loose_name(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect::<String>()
        .to_lowercase()
}

/// Percent-encodes a GCS object name for use as a single path segment. The
/// object name may contain `/`, which must be encoded as `%2F` because the
/// whole name is one path segment in the JSON API URL.
fn percent_encode_object(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Slices `content` to `[offset, offset+limit)` on character boundaries,
/// returning the slice and whether anything was cut off.
pub(super) fn slice_content(content: &str, offset: usize, limit: Option<usize>) -> (String, bool) {
    let chars: Vec<char> = content.chars().collect();
    let total = chars.len();
    let start = offset.min(total);
    let end = match limit {
        Some(l) => (start + l).min(total),
        None => total,
    };
    let slice: String = chars[start..end].iter().collect();
    (slice, end < total)
}

#[cfg(not(feature = "airtight"))]
impl GcsDocStore {
    /// Fetches application-default-credentials auth headers for the storage
    /// API, requesting the given OAuth `scopes`.
    async fn auth_headers(&self, scopes: &[&str]) -> Result<http::HeaderMap, FormaError> {
        crate::tool::google_auth::auth_headers(scopes).await
    }

    /// Lists raw object metadata under the configured prefix.
    async fn list_objects(&self) -> Result<Vec<Value>, FormaError> {
        let headers = self.auth_headers(&[STORAGE_READ_SCOPE]).await?;
        let client = crate::tool::http::client();
        let base = format!(
            "https://storage.googleapis.com/storage/v1/b/{}/o",
            self.config.bucket
        );

        let mut items: Vec<Value> = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut req = client
                .get(&base)
                .headers(headers.clone())
                .query(&[("prefix", self.config.workspace_id())]);
            if let Some(token) = &page_token {
                req = req.query(&[("pageToken", token)]);
            }
            let resp = req.send().await?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(FormaError::ApiCall {
                    url: Some(base),
                    msg: format!("HTTP {status}: {body}"),
                });
            }
            let body = resp.json::<Value>().await.unwrap_or(Value::Null);
            if let Some(arr) = body.get("items").and_then(|v| v.as_array()) {
                items.extend(arr.iter().cloned());
            }
            match body.get("nextPageToken").and_then(|v| v.as_str()) {
                Some(t) => page_token = Some(t.to_string()),
                None => break,
            }
        }
        Ok(items)
    }

    /// Downloads one object whole, with the media type the store has for it.
    ///
    /// The type comes from the response's own `Content-Type`, which is what
    /// the object was stored under, rather than from a second metadata call.
    ///
    /// A name that does not match exactly is looked up again, leniently — see
    /// [`GcsDocStore::find_by_loose_name`].
    async fn get_object_bytes(&self, path: &str) -> Result<(Vec<u8>, String), FormaError> {
        if let Some(found) = self.fetch_object(path).await? {
            return Ok(found);
        }
        let actual = self.find_by_loose_name(path).await?;
        match self.fetch_object(&actual).await? {
            Some(found) => Ok(found),
            // Between the listing and the fetch, someone deleted it.
            None => Err(FormaError::Input(format!(
                "there is no document at '{path}' any more"
            ))),
        }
    }

    /// Fetches one object, or `None` when there is nothing at that exact name.
    async fn fetch_object(&self, path: &str) -> Result<Option<(Vec<u8>, String)>, FormaError> {
        let headers = self.auth_headers(&[STORAGE_READ_SCOPE]).await?;
        let client = crate::tool::http::client();
        let object = percent_encode_object(&self.object_name(path));
        let url = format!(
            "https://storage.googleapis.com/storage/v1/b/{}/o/{}?alt=media",
            self.config.bucket, object
        );
        let resp = client.get(&url).headers(headers).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(FormaError::ApiCall {
                url: Some(url),
                msg: format!("HTTP {status}: {body}"),
            });
        }
        let media_type = resp
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(DEFAULT_MEDIA_TYPE)
            .to_string();
        let bytes = resp.bytes().await.map_err(|e| FormaError::ApiCall {
            url: Some(url),
            msg: format!("failed to read object body: {e}"),
        })?;
        Ok(Some((bytes.to_vec(), media_type)))
    }

    /// The document whose name matches `path` but for the kind of spaces or
    /// the case it is written in.
    ///
    /// A file called `Screenshot 2026-09-22 at 3.02.12 PM.png` on a Mac holds
    /// a narrow no-break space before `PM`, and a model that repeats that name
    /// back writes an ordinary one — the name looks identical and fetches
    /// nothing. Rather than make every caller reproduce invisible characters,
    /// a miss is looked up again against the listing.
    ///
    /// Only an unambiguous match is used: if two documents differ just in
    /// this, the caller is told to name one exactly.
    async fn find_by_loose_name(&self, path: &str) -> Result<String, FormaError> {
        let wanted = loose_name(path);
        let mut matches: Vec<String> = self
            .list_objects()
            .await?
            .iter()
            .filter_map(|item| item.get("name")?.as_str().map(|n| self.relative_path(n)))
            .filter(|candidate| !candidate.ends_with('/') && loose_name(candidate) == wanted)
            .collect();
        matches.sort();
        matches.dedup();

        match matches.len() {
            0 => Err(FormaError::Input(format!(
                "there is no document at '{path}' in this workspace; list it to see what is there"
            ))),
            1 => Ok(matches.remove(0)),
            _ => Err(FormaError::Input(format!(
                "'{path}' could be any of {}; ask for one by its exact name",
                matches.join(", ")
            ))),
        }
    }

    /// Uploads `content` as the object at `path` (by knowledgebase-relative
    /// path), creating it or overwriting it wholesale. Returns the stored
    /// object's size in bytes, when the API reports it.
    async fn put_object(&self, path: &str, content: &str) -> Result<Option<u64>, FormaError> {
        self.put_bytes(path, content.as_bytes().to_vec(), AGENT_TEXT_TYPE)
            .await
    }

    /// Uploads `bytes` as the object at `path`, stored as `media_type`.
    async fn put_bytes(
        &self,
        path: &str,
        bytes: Vec<u8>,
        media_type: &str,
    ) -> Result<Option<u64>, FormaError> {
        let headers = self.auth_headers(&[STORAGE_WRITE_SCOPE]).await?;
        let client = crate::tool::http::client();
        let object = percent_encode_object(&self.object_name(path));
        // `uploadType=media` uploads the raw body as the object's contents; the
        // object name goes in the `name` query parameter.
        let url = format!(
            "https://storage.googleapis.com/upload/storage/v1/b/{}/o?uploadType=media&name={}",
            self.config.bucket, object
        );
        let resp = client
            .post(&url)
            .headers(headers)
            .header(http::header::CONTENT_TYPE, media_type)
            .body(bytes)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(FormaError::ApiCall {
                url: Some(url),
                msg: format!("HTTP {status}: {body}"),
            });
        }
        let body = resp.json::<Value>().await.unwrap_or(Value::Null);
        let size = body
            .get("size")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok());
        Ok(size)
    }

    /// The metadata of the object at exactly `path`, or `None`.
    async fn stat_object(&self, path: &str) -> Result<Option<DocMeta>, FormaError> {
        let headers = self.auth_headers(&[STORAGE_READ_SCOPE]).await?;
        let object = percent_encode_object(&self.object_name(path));
        let url = format!(
            "https://storage.googleapis.com/storage/v1/b/{}/o/{}?fields=name,size,contentType",
            self.config.bucket, object
        );
        let resp = crate::tool::http::client()
            .get(&url)
            .headers(headers)
            .send()
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(FormaError::ApiCall {
                url: Some(url),
                msg: format!("HTTP {status}: {body}"),
            });
        }
        let item = resp.json::<Value>().await.unwrap_or(Value::Null);
        Ok(self.doc_meta(&item, ""))
    }

    /// Deletes the object at `path`; a missing one is fine.
    async fn delete_object(&self, path: &str) -> Result<(), FormaError> {
        let headers = self.auth_headers(&[STORAGE_WRITE_SCOPE]).await?;
        let object = percent_encode_object(&self.object_name(path));
        let url = format!(
            "https://storage.googleapis.com/storage/v1/b/{}/o/{}",
            self.config.bucket, object
        );
        let resp = crate::tool::http::client()
            .delete(&url)
            .headers(headers)
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        Err(FormaError::ApiCall {
            url: Some(url),
            msg: format!("HTTP {status}: {body}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signs a GET and a size-limited PUT through the store and uses them, as
    /// a sandbox would. Tells a signing mistake (`SignatureDoesNotMatch`) from
    /// a signer that cannot reach the bucket (`AccessDenied`). Needs
    /// `WORKSPACES_BUCKET`, application-default credentials, and
    /// `GCS_SIGNING_ACCOUNT` off Google Cloud. Deletes what it wrote. Run with:
    ///
    /// `cargo test -p agent --features integration_test signed_url_round_trip -- --ignored --nocapture`
    #[cfg(feature = "integration_test")]
    #[tokio::test]
    #[ignore = "needs a bucket and credentials"]
    async fn test_signed_url_round_trip() {
        let store = GcsDocStore::new(GcsDocConfig {
            bucket: default_workspace_bucket(),
            workspace_id: format!("runs/signing-check/{}", uuid::Uuid::new_v4().simple()),
        });
        let expires = Duration::from_secs(300);
        let client = reqwest::Client::new();

        store
            .write_bytes("probe.txt", b"signed".to_vec(), AGENT_TEXT_TYPE)
            .await
            .unwrap();
        let get = store
            .signed_url("probe.txt", SignedAccess::Read, expires)
            .await
            .unwrap();
        let resp = client.get(&get.url).send().await.unwrap();
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        let put = store
            .signed_url("put.txt", SignedAccess::Write { max_bytes: 10 }, expires)
            .await
            .unwrap();
        let send = |bytes: &'static [u8]| {
            let mut req = client.put(&put.url).body(bytes);
            for (k, v) in &put.headers {
                req = req.header(k, v);
            }
            req.send()
        };
        let small = send(b"fits").await.unwrap().status();
        let large = send(b"far too large for it").await.unwrap().status();

        for path in ["probe.txt", "put.txt"] {
            store.delete(path).await.unwrap();
        }
        assert!(status.is_success(), "signed GET: {status}: {body}");
        assert_eq!(body, "signed");
        assert!(small.is_success(), "signed PUT within the limit: {small}");
        assert!(
            !large.is_success(),
            "a PUT over the signed limit was accepted"
        );
    }

    #[test]
    fn test_object_name_and_relative_path_roundtrip() {
        let store = GcsDocStore::new(GcsDocConfig {
            bucket: "b".to_string(),
            workspace_id: "handbook/".to_string(),
        });
        assert_eq!(store.object_name("./a/b.md"), "handbook/a/b.md");
        assert_eq!(store.object_name("a/b.md"), "handbook/a/b.md");
        assert_eq!(store.relative_path("handbook/a/b.md"), "a/b.md");
        // Objects outside the prefix pass through unchanged
        assert_eq!(store.relative_path("other/x.md"), "other/x.md");

        let store = GcsDocStore::new(GcsDocConfig {
            bucket: "b".to_string(),
            workspace_id: "handbook".to_string(),
        });
        assert_eq!(store.object_name("./a/b.md"), "handbook/a/b.md");
        assert_eq!(store.object_name("a/b.md"), "handbook/a/b.md");
        assert_eq!(store.relative_path("handbook/a/b.md"), "a/b.md");
        // Objects outside the prefix pass through unchanged
        assert_eq!(store.relative_path("other/x.md"), "other/x.md");
    }

    /// What `list` tells the agent about each document: where it is, how big
    /// it is, and what it is — the last being what says whether it can be read
    /// as text at all.
    #[test]
    fn test_doc_meta_reports_size_and_media_type() {
        let store = GcsDocStore::new(GcsDocConfig {
            bucket: "b".to_string(),
            workspace_id: "handbook".to_string(),
        });
        let item = serde_json::json!({
            "name": "handbook/invoices/march.pdf",
            "size": "20481",
            "contentType": "application/pdf"
        });
        let meta = store.doc_meta(&item, "").expect("a document");
        assert_eq!(meta.path, "invoices/march.pdf");
        assert_eq!(meta.size_bytes, Some(20481));
        assert_eq!(meta.media_type.as_deref(), Some("application/pdf"));

        // A folder marker is not a document.
        let marker = serde_json::json!({ "name": "handbook/invoices/", "size": "0" });
        assert!(store.doc_meta(&marker, "").is_none());

        // Nor is anything outside the prefix being listed.
        assert!(store.doc_meta(&item, "notes/").is_none());

        // A store that records no type says so, rather than guessing one.
        let untyped = serde_json::json!({ "name": "handbook/notes.md" });
        let meta = store.doc_meta(&untyped, "").expect("a document");
        assert_eq!(meta.media_type, None);
        assert_eq!(meta.size_bytes, None);
    }

    /// The name a model repeats back is not always the name on the object: a
    /// Mac screenshot carries a narrow no-break space that comes back as an
    /// ordinary one.
    #[test]
    fn test_loose_name_ignores_the_kind_of_space_and_the_case() {
        let stored = "Screenshot 2026-09-22 at 3.02.12\u{202f}PM.png";
        let repeated = "Screenshot 2026-09-22 at 3.02.12 PM.png";
        assert_ne!(stored, repeated, "the two names differ byte for byte");
        assert_eq!(loose_name(stored), loose_name(repeated));

        assert_eq!(loose_name("Notes/Plan.MD"), loose_name("notes/plan.md"));
        // A tab or a non-breaking space reads as a space too.
        assert_eq!(loose_name("a\tb\u{a0}c"), "a b c");
        // Different documents stay different.
        assert_ne!(loose_name("plan.md"), loose_name("plans.md"));
    }

    #[test]
    fn test_slice_content() {
        let (s, truncated) = slice_content("hello world", 0, Some(5));
        assert_eq!(s, "hello");
        assert!(truncated);

        let (s, truncated) = slice_content("hello world", 6, None);
        assert_eq!(s, "world");
        assert!(!truncated);

        // reading past the end is not an error
        let (s, truncated) = slice_content("hi", 10, Some(5));
        assert_eq!(s, "");
        assert!(!truncated);
    }

    #[test]
    fn test_percent_encode_object() {
        assert_eq!(percent_encode_object("a/b c.md"), "a%2Fb%20c.md");
        assert_eq!(
            percent_encode_object("plain-name_1.txt"),
            "plain-name_1.txt"
        );
    }
}

#[async_trait]
impl DocStore for GcsDocStore {
    async fn list(&self, prefix: Option<&str>) -> Result<Vec<DocMeta>, FormaError> {
        #[cfg(feature = "airtight")]
        {
            let _ = prefix;
            return Ok(Vec::new());
        }
        #[cfg(not(feature = "airtight"))]
        {
            let objects = self.list_objects().await?;
            let extra = prefix.unwrap_or("");
            let metas = objects
                .iter()
                .filter_map(|item| self.doc_meta(item, extra))
                .collect();
            Ok(metas)
        }
    }

    async fn write(&self, path: &str, content: &str) -> Result<DocMeta, FormaError> {
        #[cfg(feature = "airtight")]
        {
            let _ = content;
            return Ok(DocMeta {
                path: path.to_string(),
                title: None,
                size_bytes: None,
                media_type: None,
            });
        }
        #[cfg(not(feature = "airtight"))]
        {
            let size_bytes = self.put_object(path, content).await?;
            Ok(DocMeta {
                path: path.to_string(),
                title: None,
                size_bytes,
                // put_object stores everything it writes as text.
                media_type: Some(AGENT_TEXT_TYPE.to_string()),
            })
        }
    }

    async fn attach(&self, path: &str) -> Result<AttachedDoc, FormaError> {
        #[cfg(feature = "airtight")]
        {
            return Ok(AttachedDoc {
                path: path.to_string(),
                media_type: DEFAULT_MEDIA_TYPE.to_string(),
                bytes: Vec::new(),
            });
        }
        #[cfg(not(feature = "airtight"))]
        {
            let (bytes, media_type) = self.get_object_bytes(path).await?;
            Ok(AttachedDoc {
                path: path.to_string(),
                media_type,
                bytes,
            })
        }
    }

    async fn stat(&self, path: &str) -> Result<Option<DocMeta>, FormaError> {
        #[cfg(feature = "airtight")]
        {
            let _ = path;
            return Ok(None);
        }
        #[cfg(not(feature = "airtight"))]
        self.stat_object(path).await
    }

    async fn write_bytes(
        &self,
        path: &str,
        bytes: Vec<u8>,
        media_type: &str,
    ) -> Result<DocMeta, FormaError> {
        #[cfg(feature = "airtight")]
        {
            let _ = bytes;
            return Ok(DocMeta {
                path: path.to_string(),
                title: None,
                size_bytes: None,
                media_type: Some(media_type.to_string()),
            });
        }
        #[cfg(not(feature = "airtight"))]
        {
            let size_bytes = self.put_bytes(path, bytes, media_type).await?;
            Ok(DocMeta {
                path: path.to_string(),
                title: None,
                size_bytes,
                media_type: Some(media_type.to_string()),
            })
        }
    }

    async fn delete(&self, path: &str) -> Result<(), FormaError> {
        #[cfg(feature = "airtight")]
        {
            let _ = path;
            return Ok(());
        }
        #[cfg(not(feature = "airtight"))]
        self.delete_object(path).await
    }

    async fn signed_url(
        &self,
        path: &str,
        access: SignedAccess,
        expires: Duration,
    ) -> Result<SignedRequest, FormaError> {
        let (method, headers) = match access {
            SignedAccess::Read => ("GET", std::collections::BTreeMap::new()),
            SignedAccess::Write { max_bytes } => {
                let mut headers = std::collections::BTreeMap::new();
                headers.insert(
                    super::gcs_signing::CONTENT_LENGTH_RANGE.to_string(),
                    format!("0,{max_bytes}"),
                );
                ("PUT", headers)
            }
        };
        let signer = super::gcs_signing::Signer::for_this_process().await?;
        signer
            .signed_url(
                method,
                &self.config.bucket,
                &self.object_name(path),
                headers,
                expires.as_secs(),
                chrono::Utc::now(),
            )
            .await
    }

    async fn signed_upload(
        &self,
        prefix: &str,
        max_bytes: u64,
        expires: Duration,
    ) -> Result<SignedUpload, FormaError> {
        let signer = super::gcs_signing::Signer::for_this_process().await?;
        signer
            .post_policy(
                &self.config.bucket,
                &self.object_name(prefix),
                max_bytes,
                expires.as_secs(),
                chrono::Utc::now(),
            )
            .await
    }
}

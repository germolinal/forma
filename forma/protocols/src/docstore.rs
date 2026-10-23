//! A backend-agnostic interface over a knowledgebase of documents.
//!
//! A [`DocStore`] exposes the three primitives an agent needs to explore a
//! corpus *agentically* — without embeddings or a vector database:
//!
//! - [`DocStore::list`] — enumerate the documents (optionally under a prefix),
//! - [`DocStore::read`] — fetch the text of one document (optionally a slice),
//! - [`DocStore::attach`] — fetch a document whole, as bytes, for the documents
//!   that are not text to read: a picture, a PDF.
//!
//! Beyond what an agent calls directly, a store can hand out *signed* access
//! ([`DocStore::signed_url`], [`DocStore::signed_upload`]): requests that whoever
//! holds them can make without credentials of their own, scoped to exact
//! documents and expiring. That is how something the runtime does not trust —
//! a code-execution sandbox — reads its inputs and writes its results without
//! ever holding a credential to the store. Each backend signs its own way.
//!
//! Keeping this interface in `protocols` — exactly like [`crate::FormaDB`] — is
//! what lets the framework stay independent of any particular storage: a
//! self-hoster can back it with MongoDB, Google Cloud Storage, the local file
//! system, or anything else, without the agent runtime changing.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::FormaError;
use serde::{Deserialize, Serialize};

/// Metadata about a single document in a [`DocStore`], as returned by
/// [`DocStore::list`]. The contents are fetched separately with
/// [`DocStore::read`] so that listing a large corpus stays cheap.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocMeta {
    /// The document's path/key within the store. This is the identifier passed
    /// back to [`DocStore::read`]. For hierarchical stores it looks like a file
    /// path (e.g. `handbook/onboarding.md`).
    pub path: String,

    /// A human-readable title, when the backend has one. Falls back to `None`
    /// when only a path is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// The size of the document in bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,

    /// What the document is, as the store records it (`text/markdown`,
    /// `application/pdf`, `image/png`…), when known.
    ///
    /// This tells an agent, before spending a call on it, what reading the
    /// document will give it: text, a file to look at, or a refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

/// The contents of one text document, as the read tool returns them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocContent {
    /// The path of the document that was read.
    pub path: String,

    /// The (possibly sliced) textual content of the document.
    pub content: String,

    /// The total length, in characters, of the full document, so the agent can
    /// tell whether it has seen everything or should read further.
    pub total_len: usize,

    /// `true` when `content` is only a slice of the document (i.e. it was
    /// truncated because it was longer than the requested limit).
    pub truncated: bool,
}

/// One document, whole, as [`DocStore::attach`] returns it.
///
/// Deliberately not serializable: these bytes belong in a message part, not in
/// a tool result. Serializing one would put a base64 copy of the document in
/// the conversation's history, which is the one thing attaching is meant to
/// avoid.
#[derive(Debug, Clone, PartialEq)]
pub struct AttachedDoc {
    /// The path of the document that was fetched.
    pub path: String,

    /// What the document is, as the store records it. Falls back to
    /// `application/octet-stream` when the store says nothing.
    pub media_type: String,

    /// The document itself.
    pub bytes: Vec<u8>,
}

impl AttachedDoc {
    /// The document's size in bytes, for the caps a caller applies.
    pub fn size_bytes(&self) -> u64 {
        self.bytes.len() as u64
    }
}

/// What a [`SignedRequest`] lets its holder do with one document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignedAccess {
    /// Download it.
    Read,
    /// Create or overwrite it.
    Write {
        /// The largest upload allowed. Part of the signature: an upload outside
        /// it is refused by the store itself.
        max_bytes: u64,
    },
}

/// A request whoever holds it may make, exactly as signed, until it expires —
/// with no credentials of their own.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SignedRequest {
    /// The HTTP method the signature is for (`GET`, `PUT`).
    pub method: String,
    /// The URL, signature included. Treat it as a credential: never log it.
    pub url: String,
    /// Headers the request must carry, as signed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

/// A grant to upload any number of documents under one prefix, each as an
/// HTML-form (`multipart/form-data`) POST, until it expires. It lets its holder
/// write there and nothing else: not read, not list, not write elsewhere.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SignedUpload {
    /// Where to POST the form.
    pub url: String,
    /// What the form's `key` — the uploaded document's full name in the store —
    /// must start with.
    pub key_prefix: String,
    /// Fields the form must carry, besides `key`, `Content-Type` and `file`
    /// (which goes last).
    pub fields: BTreeMap<String, String>,
}

/// The refusal a store gives for an operation it does not offer.
fn unsupported(what: &str) -> FormaError {
    FormaError::Input(format!("this kind of store cannot {what}"))
}

/// A backend-agnostic knowledgebase of documents that an agent can explore.
///
/// Implementations live outside `protocols` (e.g. a MongoDB-backed store, a
/// Google Cloud Storage-backed store) so that this crate keeps no heavy
/// dependencies. The interface is deliberately small and object-safe, so a tool
/// can hold a `Box<dyn DocStore>` chosen at runtime from configuration.
#[async_trait::async_trait]
pub trait DocStore: Send + Sync {
    /// Lists the documents in the store. When `prefix` is `Some`, only
    /// documents whose path starts with it are returned, letting the agent
    /// navigate a tree one level at a time.
    async fn list(&self, prefix: Option<&str>) -> Result<Vec<DocMeta>, FormaError>;

    /// Fetches the document at `path` whole, as it is stored.
    ///
    /// This is how an agent reads a document — the read tool decides, from
    /// what comes back, whether to hand it the text or attach the file — and
    /// how a document is handed to something other than a model, such as a
    /// sandbox that is about to run code over it. Callers cap
    /// the size themselves: a store cannot know what the thing receiving the
    /// document will accept.
    async fn attach(&self, path: &str) -> Result<AttachedDoc, FormaError>;

    /// Writes `content` to the document at `path`, creating it when it does not
    /// exist and overwriting it in full otherwise. Returns metadata about the
    /// stored document so the caller can confirm the path (and size) that was
    /// persisted.
    ///
    /// Backends that are read-only should return a [`FormaError`] rather than
    /// silently succeeding.
    async fn write(&self, path: &str, content: &str) -> Result<DocMeta, FormaError>;

    // The operations below are not exposed to agents directly; they are what
    // the runtime needs to move documents to and from a sandbox. A store that
    // does not offer one refuses it: every default fails closed.

    /// Describes the document at `path` — its size and type — or `None` when
    /// there is nothing there. Exact: no lenient matching of the name.
    async fn stat(&self, path: &str) -> Result<Option<DocMeta>, FormaError> {
        let _ = path;
        Err(unsupported("describe a single document"))
    }

    /// Writes `bytes` to the document at `path` as `media_type`, creating or
    /// overwriting it. Unlike [`DocStore::write`], the content need not be text.
    async fn write_bytes(
        &self,
        path: &str,
        bytes: Vec<u8>,
        media_type: &str,
    ) -> Result<DocMeta, FormaError> {
        let _ = (path, bytes, media_type);
        Err(unsupported("store binary documents"))
    }

    /// Deletes the document at `path`. Deleting what is not there is not an
    /// error.
    async fn delete(&self, path: &str) -> Result<(), FormaError> {
        let _ = path;
        Err(unsupported("delete documents"))
    }

    /// A request for exactly the document at `path`, granting `access`, valid
    /// for `expires`. See [`SignedRequest`].
    async fn signed_url(
        &self,
        path: &str,
        access: SignedAccess,
        expires: Duration,
    ) -> Result<SignedRequest, FormaError> {
        let _ = (path, access, expires);
        Err(unsupported("sign requests"))
    }

    /// A grant to upload documents under `prefix`, each at most `max_bytes`,
    /// valid for `expires`. See [`SignedUpload`].
    async fn signed_upload(
        &self,
        prefix: &str,
        max_bytes: u64,
        expires: Duration,
    ) -> Result<SignedUpload, FormaError> {
        let _ = (prefix, max_bytes, expires);
        Err(unsupported("sign uploads"))
    }
}

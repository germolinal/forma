//! Signed Cloud Storage URLs and POST policies, for [`super::gcs::GcsDocStore`]'s
//! [`DocStore::signed_url`](protocols::DocStore::signed_url) and
//! [`DocStore::signed_upload`](protocols::DocStore::signed_upload).
//!
//! A signature is scoped to one object (or, for an upload policy, one prefix),
//! can carry a size limit, and expires. Whoever holds it needs no credentials of
//! their own — which is the point: a code-execution sandbox runs as a service
//! account with no roles, and reaches the bucket only through these.
//!
//! Nothing here holds a private key. The runtime asks the IAM Credentials API
//! to sign with its own service account (`signBlob`), which needs that account
//! to hold `roles/iam.serviceAccountTokenCreator` on itself. The signatures are
//! the V4 scheme described in
//! <https://cloud.google.com/storage/docs/authentication/signatures>.

use std::collections::BTreeMap;

use base64::Engine;
use chrono::{DateTime, Utc};
use protocols::{FormaError, SignedRequest, SignedUpload, envar::get_env_var};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

use crate::tool::google_auth::{CLOUD_PLATFORM_SCOPE, auth_headers};

/// The service account Cloud Storage URLs are signed as. On Google Cloud it is
/// not needed: the runtime signs as its own account, read from the metadata
/// server. Locally, set it to an account your credentials may impersonate
/// (`roles/iam.serviceAccountTokenCreator` on it) and that can reach the bucket:
/// a signed URL carries the signer's permissions.
pub const SIGNER_ENV: &str = "GCS_SIGNING_ACCOUNT";

const HOST: &str = "storage.googleapis.com";
const ALGORITHM: &str = "GOOG4-RSA-SHA256";

/// Header that makes Cloud Storage refuse an upload outside a size range. Part
/// of the signature, so the holder of a URL cannot drop it.
pub const CONTENT_LENGTH_RANGE: &str = "x-goog-content-length-range";

/// Who signs, and how.
#[derive(Debug, Clone)]
pub struct Signer {
    email: String,
}

impl Signer {
    /// The signer for this process: `GCS_SIGNING_ACCOUNT` if set, otherwise
    /// the account the metadata server says this process runs as.
    pub async fn for_this_process() -> Result<Self, FormaError> {
        if let Some(email) = get_env_var(SIGNER_ENV)?.filter(|e| !e.trim().is_empty()) {
            return Ok(Self {
                email: email.trim().to_string(),
            });
        }
        static EMAIL: OnceCell<String> = OnceCell::const_new();
        let email = EMAIL
            .get_or_try_init(|| async {
                let resp = crate::tool::http::client()
                    .get("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/email")
                    .header("Metadata-Flavor", "Google")
                    .timeout(std::time::Duration::from_secs(5))
                    .send()
                    .await
                    .map_err(|e| FormaError::Input(format!(
                        "cannot tell which account signs Cloud Storage URLs (set {SIGNER_ENV} when not on Google Cloud): {e}"
                    )))?;
                if !resp.status().is_success() {
                    return Err(FormaError::Input(format!(
                        "the metadata server answered {} for the account that signs Cloud Storage URLs",
                        resp.status()
                    )));
                }
                Ok(resp.text().await.unwrap_or_default().trim().to_string())
            })
            .await?;
        Ok(Self {
            email: email.clone(),
        })
    }

    /// Signs `payload` with the account's Google-managed key.
    async fn sign(&self, payload: &[u8]) -> Result<Vec<u8>, FormaError> {
        let url = format!(
            "https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts/{}:signBlob",
            self.email
        );
        let headers = auth_headers(&[CLOUD_PLATFORM_SCOPE]).await?;
        let b64 = base64::engine::general_purpose::STANDARD;
        let resp = crate::tool::http::client()
            .post(&url)
            .headers(headers)
            .json(&json!({ "payload": b64.encode(payload) }))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(FormaError::ApiCall {
                url: Some(url),
                msg: format!("signBlob: HTTP {status}: {body}"),
            });
        }
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        let signed = body
            .get("signedBlob")
            .and_then(Value::as_str)
            .ok_or_else(|| FormaError::ApiCall {
                url: Some(url.clone()),
                msg: "signBlob: no signedBlob in the answer".to_string(),
            })?;
        b64.decode(signed).map_err(|e| FormaError::ApiCall {
            url: Some(url),
            msg: format!("signBlob: undecodable signature: {e}"),
        })
    }

    /// A URL for one `method` on one object, valid for `expires_secs`.
    ///
    /// Every header in `headers` becomes part of the signature, so the request
    /// has to carry it exactly — which is how a size limit is made binding.
    pub async fn signed_url(
        &self,
        method: &str,
        bucket: &str,
        object: &str,
        headers: BTreeMap<String, String>,
        expires_secs: u64,
        now: DateTime<Utc>,
    ) -> Result<SignedRequest, FormaError> {
        let parts = V4Request::new(
            &self.email,
            method,
            bucket,
            object,
            &headers,
            expires_secs,
            now,
        );
        let signature = hex(&self.sign(parts.string_to_sign.as_bytes()).await?);
        Ok(SignedRequest {
            method: method.to_string(),
            url: format!(
                "https://{HOST}{}?{}&X-Goog-Signature={signature}",
                parts.canonical_uri, parts.canonical_query
            ),
            headers,
        })
    }

    /// A POST policy for uploads under `key_prefix` in `bucket`, each at most
    /// `max_bytes`, valid for `expires_secs`.
    pub async fn post_policy(
        &self,
        bucket: &str,
        key_prefix: &str,
        max_bytes: u64,
        expires_secs: u64,
        now: DateTime<Utc>,
    ) -> Result<SignedUpload, FormaError> {
        let (policy_b64, mut fields) = policy_document(
            &self.email,
            bucket,
            key_prefix,
            max_bytes,
            expires_secs,
            now,
        );
        let signature = hex(&self.sign(policy_b64.as_bytes()).await?);
        fields.insert("policy".to_string(), policy_b64);
        fields.insert("x-goog-signature".to_string(), signature);
        Ok(SignedUpload {
            url: format!("https://{HOST}/{bucket}/"),
            key_prefix: key_prefix.to_string(),
            fields,
        })
    }
}

/// The pieces of a V4 signed URL, before the signature.
#[derive(Debug)]
struct V4Request {
    canonical_uri: String,
    canonical_query: String,
    string_to_sign: String,
}

impl V4Request {
    fn new(
        email: &str,
        method: &str,
        bucket: &str,
        object: &str,
        headers: &BTreeMap<String, String>,
        expires_secs: u64,
        now: DateTime<Utc>,
    ) -> Self {
        let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let scope = format!("{}/auto/storage/goog4_request", now.format("%Y%m%d"));
        let canonical_uri = format!("/{bucket}/{}", uri_encode(object, false));

        // Header names are lower-case and sorted (a BTreeMap of lower-case
        // keys), `host` included.
        let mut all_headers: BTreeMap<String, String> = headers
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.trim().to_string()))
            .collect();
        all_headers.insert("host".to_string(), HOST.to_string());
        let canonical_headers: String = all_headers
            .iter()
            .map(|(k, v)| format!("{k}:{v}\n"))
            .collect();
        let signed_headers = all_headers.keys().cloned().collect::<Vec<_>>().join(";");

        let mut query = BTreeMap::new();
        query.insert("X-Goog-Algorithm", ALGORITHM.to_string());
        query.insert("X-Goog-Credential", format!("{email}/{scope}"));
        query.insert("X-Goog-Date", timestamp.clone());
        query.insert("X-Goog-Expires", expires_secs.to_string());
        query.insert("X-Goog-SignedHeaders", signed_headers.clone());
        let canonical_query = query
            .iter()
            .map(|(k, v)| format!("{}={}", uri_encode(k, true), uri_encode(v, true)))
            .collect::<Vec<_>>()
            .join("&");

        let canonical_request = [
            method,
            &canonical_uri,
            &canonical_query,
            &canonical_headers,
            &signed_headers,
            "UNSIGNED-PAYLOAD",
        ]
        .join("\n");
        let string_to_sign = [
            ALGORITHM.to_string(),
            timestamp,
            scope,
            hex(&Sha256::digest(canonical_request.as_bytes())),
        ]
        .join("\n");

        Self {
            canonical_uri,
            canonical_query,
            string_to_sign,
        }
    }
}

/// The base64 policy document for a V4 POST upload, and the form fields that
/// go with it (all but the signature and the policy itself).
fn policy_document(
    email: &str,
    bucket: &str,
    key_prefix: &str,
    max_bytes: u64,
    expires_secs: u64,
    now: DateTime<Utc>,
) -> (String, BTreeMap<String, String>) {
    let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let credential = format!(
        "{email}/{}/auto/storage/goog4_request",
        now.format("%Y%m%d")
    );
    let expiration = (now + chrono::Duration::seconds(expires_secs as i64))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    let mut fields = BTreeMap::new();
    fields.insert("x-goog-algorithm".to_string(), ALGORITHM.to_string());
    fields.insert("x-goog-credential".to_string(), credential.clone());
    fields.insert("x-goog-date".to_string(), timestamp.clone());

    let policy = json!({
        "conditions": [
            { "bucket": bucket },
            ["starts-with", "$key", key_prefix],
            ["starts-with", "$Content-Type", ""],
            ["content-length-range", 0, max_bytes],
            { "x-goog-algorithm": ALGORITHM },
            { "x-goog-credential": credential },
            { "x-goog-date": timestamp },
        ],
        "expiration": expiration,
    });
    let b64 = base64::engine::general_purpose::STANDARD.encode(policy.to_string());
    (b64, fields)
}

/// Percent-encodes everything but RFC 3986 unreserved characters, and `/` too
/// unless `encode_slash` is false (object names keep their slashes in a path).
fn uri_encode(value: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b'/' if !encode_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 8, 16, 30, 5).unwrap()
    }

    #[test]
    fn test_uri_encode() {
        assert_eq!(uri_encode("runs/a b/ñ.csv", false), "runs/a%20b/%C3%B1.csv");
        assert_eq!(uri_encode("runs/a", true), "runs%2Fa");
        assert_eq!(uri_encode("sa@p.iam", true), "sa%40p.iam");
    }

    #[test]
    fn test_v4_request_signs_what_it_should() {
        let mut headers = BTreeMap::new();
        headers.insert(CONTENT_LENGTH_RANGE.to_string(), "0,1024".to_string());
        let r = V4Request::new(
            "sa@p.iam.gserviceaccount.com",
            "PUT",
            "bucket",
            "runs/o/u/r1/stdout.txt",
            &headers,
            900,
            now(),
        );
        assert_eq!(r.canonical_uri, "/bucket/runs/o/u/r1/stdout.txt");
        // Sorted, encoded, and naming the size header as signed.
        assert_eq!(
            r.canonical_query,
            "X-Goog-Algorithm=GOOG4-RSA-SHA256\
             &X-Goog-Credential=sa%40p.iam.gserviceaccount.com%2F20261008%2Fauto%2Fstorage%2Fgoog4_request\
             &X-Goog-Date=20261008T163005Z\
             &X-Goog-Expires=900\
             &X-Goog-SignedHeaders=host%3Bx-goog-content-length-range"
        );
        let lines: Vec<&str> = r.string_to_sign.lines().collect();
        assert_eq!(lines[0], "GOOG4-RSA-SHA256");
        assert_eq!(lines[1], "20261008T163005Z");
        assert_eq!(lines[2], "20261008/auto/storage/goog4_request");
        assert_eq!(lines[3].len(), 64);
    }

    #[test]
    fn test_v4_request_depends_on_every_part() {
        let h = BTreeMap::new();
        let base = V4Request::new("a@b", "GET", "bk", "o", &h, 60, now()).string_to_sign;
        assert_ne!(
            base,
            V4Request::new("a@b", "PUT", "bk", "o", &h, 60, now()).string_to_sign
        );
        assert_ne!(
            base,
            V4Request::new("a@b", "GET", "bk", "o2", &h, 60, now()).string_to_sign
        );
        assert_ne!(
            base,
            V4Request::new("a@b", "GET", "bk", "o", &h, 61, now()).string_to_sign
        );
        let mut sized = BTreeMap::new();
        sized.insert(CONTENT_LENGTH_RANGE.to_string(), "0,1".to_string());
        assert_ne!(
            base,
            V4Request::new("a@b", "GET", "bk", "o", &sized, 60, now()).string_to_sign
        );
    }

    #[test]
    fn test_policy_is_bound_to_the_prefix_and_the_size() {
        let (b64, fields) = policy_document("a@b", "bk", "runs/o/u/r/out/", 2048, 600, now());
        let policy: Value = serde_json::from_slice(
            &base64::engine::general_purpose::STANDARD
                .decode(b64)
                .unwrap(),
        )
        .unwrap();
        let conditions = policy["conditions"].as_array().unwrap();
        assert!(conditions.contains(&json!({ "bucket": "bk" })));
        assert!(conditions.contains(&json!(["starts-with", "$key", "runs/o/u/r/out/"])));
        assert!(conditions.contains(&json!(["content-length-range", 0, 2048])));
        assert_eq!(policy["expiration"], "2026-10-08T16:40:05Z");
        assert_eq!(fields["x-goog-date"], "20261008T163005Z");
        assert_eq!(
            fields["x-goog-credential"],
            "a@b/20261008/auto/storage/goog4_request"
        );
    }
}

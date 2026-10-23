//! Application-default credentials for the Google APIs the runtime calls itself.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};

use google_cloud_auth::credentials::{Builder, CacheableResource, Credentials};
use protocols::FormaError;

/// The scope for Google APIs that have no narrower one (IAM Credentials, Cloud
/// Run Admin).
pub(crate) const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// The credentials built so far, one per set of scopes. Credentials keep their
/// access token and renew it before it expires, so building them for every
/// request meant asking Google for a new token every time.
static CREDENTIALS: LazyLock<Mutex<HashMap<Vec<String>, Credentials>>> =
    LazyLock::new(Default::default);

/// The process's credentials for `scopes`, built the first time they are
/// asked for. A failure to build them is not kept: the next request tries
/// again.
fn credentials(scopes: &[&str]) -> Result<Credentials, FormaError> {
    let key: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
    let mut built = CREDENTIALS.lock().expect("credentials cache poisoned");
    if let Some(creds) = built.get(&key) {
        return Ok(creds.clone());
    }
    let creds = Builder::default()
        .with_scopes(scopes.iter().copied())
        .build()
        .map_err(|e| FormaError::ApiCall {
            url: None,
            msg: format!("failed to load Google application-default credentials: {e}"),
        })?;
    built.insert(key, creds.clone());
    Ok(creds)
}

/// Application-default-credentials headers for `scopes`: the runtime's service
/// account on Google Cloud, or `gcloud auth application-default login` locally.
pub(crate) async fn auth_headers(scopes: &[&str]) -> Result<http::HeaderMap, FormaError> {
    let creds = credentials(scopes)?;
    match creds.headers(http::Extensions::new()).await {
        Ok(CacheableResource::New { data, .. }) => Ok(data),
        Ok(CacheableResource::NotModified) => Ok(http::HeaderMap::new()),
        Err(e) => Err(FormaError::ApiCall {
            url: None,
            msg: format!("failed to fetch Google auth headers: {e}"),
        }),
    }
}

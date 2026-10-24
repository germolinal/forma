use crate::{FORMA_ENCRYPTION_KEY_ENV, FormaError};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use chrono::{DateTime, Duration, Utc};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json;
use std::{collections::HashMap, sync::OnceLock};

/// An OAuth2 access/refresh token pair for a single MCP server.
///
/// Keyed by the server URL when stored in the tokens collection.
/// Call [`is_expired`][OauthToken::is_expired] before use; if true, call
/// [`refresh`][OauthToken::refresh] to obtain a new token.
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OauthToken {
    /// The OAuth2 client ID used to obtain this token.
    pub client_id: String,
    /// The OAuth2 client secret used to refresh this token.
    pub client_secret: String,
    /// The bearer token sent in `Authorization` headers.
    pub access_token: String,
    /// The refresh token used to obtain a new access token when this one expires.
    pub refresh_token: String,
    /// The UTC timestamp after which this token must be refreshed.
    pub expires_at: DateTime<Utc>,
    /// The URL of the token endpoint used for refresh requests.
    pub token_endpoint: String,
}

impl OauthToken {
    /// Returns `true` if the token will expire within 30 seconds.
    pub fn is_expired(&self) -> bool {
        self.expires_at <= Utc::now() + Duration::seconds(30)
    }

    /// Returns a new `OauthToken` with an updated `access_token`, `refresh_token`,
    /// and `expires_at` by calling the token endpoint.
    pub async fn refresh(self) -> Result<Self, FormaError> {
        let http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| FormaError::OAuthError {
                url: self.token_endpoint.clone(),
                msg: format!("failed to build http client: {e}"),
            })?;

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: Option<i64>,
            refresh_token: Option<String>,
        }

        let response = http_client
            .post(&self.token_endpoint)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &self.refresh_token),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
            ])
            .send()
            .await
            .map_err(|e| FormaError::OAuthError {
                url: self.token_endpoint.clone(),
                msg: format!("refresh request failed: {e}"),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(FormaError::OAuthError {
                url: self.token_endpoint.clone(),
                msg: format!("refresh failed ({status}): {body}"),
            });
        }

        let data: TokenResponse = response.json().await.map_err(|e| FormaError::OAuthError {
            url: self.token_endpoint.clone(),
            msg: format!("refresh response parse failed: {e}"),
        })?;

        Ok(OauthToken {
            access_token: data.access_token,
            refresh_token: data
                .refresh_token
                .unwrap_or_else(|| self.refresh_token.clone()),
            expires_at: Utc::now() + Duration::seconds(data.expires_in.unwrap_or(3600)),
            token_endpoint: self.token_endpoint.clone(),
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
        })
    }
}

/// The key credentials are encrypted with, read from `FORMA_ENCRYPTION_KEY` the
/// first time it is needed. A key that is missing or malformed is kept as the
/// error it is, so that every use reports it rather than bringing the server
/// down.
static ENCRYPTION_KEY: OnceLock<Result<LessSafeKey, String>> = OnceLock::new();

fn load_encryption_key() -> Result<LessSafeKey, String> {
    let raw = std::env::var(FORMA_ENCRYPTION_KEY_ENV)
        .map_err(|_| format!("{FORMA_ENCRYPTION_KEY_ENV} must be set"))?;
    let key_bytes = B64
        .decode(raw.trim())
        .map_err(|_| format!("{FORMA_ENCRYPTION_KEY_ENV} must be valid base64"))?;
    let unbound = UnboundKey::new(&AES_256_GCM, &key_bytes)
        .map_err(|_| format!("{FORMA_ENCRYPTION_KEY_ENV} must be 32 bytes"))?;
    Ok(LessSafeKey::new(unbound))
}

fn encryption_key() -> Result<&'static LessSafeKey, String> {
    ENCRYPTION_KEY
        .get_or_init(load_encryption_key)
        .as_ref()
        .map_err(Clone::clone)
}

fn encrypt(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let key = encryption_key()?;
    let rng = SystemRandom::new();

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rng.fill(&mut nonce_bytes)
        .map_err(|_| "could not draw a nonce to encrypt with".to_string())?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);

    let mut ciphertext = plaintext.to_vec();
    key.seal_in_place_append_tag(nonce, Aad::empty(), &mut ciphertext)
        .map_err(|_| "could not encrypt".to_string())?;

    // Output: [nonce (12 bytes)] + [ciphertext + tag]
    let mut output = nonce_bytes.to_vec();
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

fn decrypt(data: &[u8]) -> Result<Vec<u8>, String> {
    let key = encryption_key()?;
    let bad = || "could not decrypt: the data, or the key, is not the one it was encrypted with";

    let Some((nonce_bytes, ciphertext)) = data.split_first_chunk::<NONCE_LEN>() else {
        return Err(bad().to_string());
    };
    let nonce = Nonce::assume_unique_for_key(*nonce_bytes);

    let mut buffer = ciphertext.to_vec();
    let plaintext = key
        .open_in_place(nonce, Aad::empty(), &mut buffer)
        .map_err(|_| bad().to_string())?;
    Ok(plaintext.to_vec())
}

/// A set of encrypted OAuth tokens for a single user
#[derive(Debug, Clone)]
pub struct EncryptedTokenSet<T: Serialize + for<'a> Deserialize<'a>>(pub HashMap<String, T>);

impl<T: Serialize + for<'a> Deserialize<'a>> From<HashMap<String, T>> for EncryptedTokenSet<T> {
    fn from(value: HashMap<String, T>) -> Self {
        EncryptedTokenSet(value)
    }
}

impl<T: Serialize + for<'a> Deserialize<'a>> Default for EncryptedTokenSet<T> {
    fn default() -> Self {
        EncryptedTokenSet(HashMap::default())
    }
}

impl<T: Serialize + for<'a> Deserialize<'a>> EncryptedTokenSet<T> {
    /// Returns the value associated with the given key, if one exists.
    pub fn get(&self, key: &str) -> Option<&T> {
        self.0.get(key)
    }

    /// Inserts a value into the token set under the given key.
    pub fn insert(&mut self, key: String, value: T) {
        self.0.insert(key, value);
    }

    /// Removes the value associated with the given key, if one exists.
    pub fn remove(&mut self, key: &str) -> Option<T> {
        self.0.remove(key)
    }
}

impl<'de, T: Serialize + for<'a> Deserialize<'a>> Deserialize<'de> for EncryptedTokenSet<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let encoded = String::deserialize(deserializer)?;

        let ciphertext = B64.decode(&encoded).map_err(serde::de::Error::custom)?;

        let plaintext = decrypt(&ciphertext).map_err(serde::de::Error::custom)?;

        let token_set = serde_json::from_slice(&plaintext).map_err(serde::de::Error::custom)?;

        Ok(EncryptedTokenSet(token_set))
    }
}

impl<T: Serialize + for<'a> Deserialize<'a>> Serialize for EncryptedTokenSet<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let json = serde_json::to_string(&self.0).map_err(serde::ser::Error::custom)?;

        let ciphertext = encrypt(json.as_bytes()).map_err(serde::ser::Error::custom)?;

        let encoded = B64.encode(&ciphertext);
        serializer.serialize_str(&encoded)
    }
}

/// The encrypted token set for a single user, stored as one MongoDB document.
pub type EncryptedTokens = EncryptedTokenSet<OauthToken>;

/// All OAuth tokens for a single user, stored as one document.
///
/// The `tokens` map is keyed by MCP server URL so that a single
/// `findOne({ user_id, agent_id })` returns all of a user's credentials.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserTokens {
    /// The user this document belongs to.
    pub user_id: String,
    /// Map from MCP server URL to the encrypted OAuth token for that server.
    pub tokens: EncryptedTokens,
}

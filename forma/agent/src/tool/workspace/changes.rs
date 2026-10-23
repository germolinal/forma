//! The changes an agent made to a workspace that nobody has reviewed yet.
//!
//! An agent's changes take effect at once — it and everybody else see the new
//! files straight away — but each one stays *pending* until a person approves
//! or reverts it. Reverting needs what the file was before, so the first time
//! an agent touches a file after an approval, the file as it was is kept. Later
//! changes leave that copy alone: what a person reviews, and what reverting
//! goes back to, is always the version they last approved, however many times
//! the agent went over it since.
//!
//! Everything lives inside the workspace itself, under [`HISTORY_ROOT`], so it
//! works the same in Cloud Storage and in a folder, and needs nothing else to
//! be deployed. Each changed file is one record, `.history/changes/<id>.json`,
//! with the approved bytes at `.history/base/<id>`. Applications built on
//! Forma may read the same records to show them and to approve or revert
//! them, so their shape is a contract: change it with care.
//!
//! A change nobody reviews is approved on its own [`KEEP_FOR`] after it was
//! last touched: its record and copy are dropped by whoever reads the records
//! next. Nothing has to run on a schedule for that.

use chrono::{DateTime, Duration, Utc};
use protocols::{DocStore, FormaError};
use serde::{Deserialize, Serialize};

/// Where a workspace keeps its pending changes. Hidden from everything an
/// agent can do, and from the listings people see.
pub const HISTORY_ROOT: &str = ".history";

const CHANGES_DIR: &str = ".history/changes/";
const BASE_DIR: &str = ".history/base/";

/// How long a change waits for a review before it is approved on its own.
pub const KEEP_FOR_DAYS: i64 = 3;

fn keep_for() -> Duration {
    Duration::days(KEEP_FOR_DAYS)
}

/// One file an agent changed since it was last approved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub id: String,
    /// Where the file was when it was last approved; `None` when the agent
    /// created it.
    pub original: Option<String>,
    /// Where it is now; `None` when the agent deleted it.
    pub current: Option<String>,
    /// What the approved version was stored as, to restore it as that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_type: Option<String>,
    pub first_at: DateTime<Utc>,
    /// The last time the agent touched it. The change is approved on its own
    /// [`KEEP_FOR_DAYS`] after this.
    pub last_at: DateTime<Utc>,
}

impl Change {
    fn record_path(&self) -> String {
        format!("{CHANGES_DIR}{}.json", self.id)
    }

    fn base_path(&self) -> String {
        format!("{BASE_DIR}{}", self.id)
    }

    fn expired(&self, now: DateTime<Utc>) -> bool {
        now - self.last_at > keep_for()
    }
}

/// Whether `path` is inside [`HISTORY_ROOT`], which no agent may read, write
/// or even see.
pub fn is_hidden(path: &str) -> bool {
    path.trim_start_matches('/').split('/').next() == Some(HISTORY_ROOT)
}

/// Refuses a path inside [`HISTORY_ROOT`].
pub fn check_visible(path: &str) -> Result<(), FormaError> {
    if is_hidden(path) {
        return Err(FormaError::Input(format!("'{path}' cannot be used")));
    }
    Ok(())
}

/// Every pending change in `store`. Those past [`KEEP_FOR_DAYS`] are approved
/// on the way — dropped — rather than returned.
pub async fn pending(store: &dyn DocStore) -> Result<Vec<Change>, FormaError> {
    let now = Utc::now();
    let mut changes = Vec::new();
    for meta in store.list(Some(CHANGES_DIR)).await? {
        if !meta.path.ends_with(".json") {
            continue;
        }
        let doc = store.attach(&meta.path).await?;
        let change: Change = match serde_json::from_slice(&doc.bytes) {
            Ok(change) => change,
            Err(e) => {
                tracing::warn!("unreadable change record {}: {e}", meta.path);
                continue;
            }
        };
        if change.expired(now) {
            approve(store, &change).await?;
        } else {
            changes.push(change);
        }
    }
    Ok(changes)
}

/// Forgets a change: what the file is now is what stays.
async fn approve(store: &dyn DocStore, change: &Change) -> Result<(), FormaError> {
    if change.original.is_some() {
        store.delete(&change.base_path()).await?;
    }
    store.delete(&change.record_path()).await
}

/// The pending change for the file at `path`, starting one — and keeping the
/// file as it is now — if there is none yet. Call it before changing the file,
/// then [`save`] it after.
pub async fn track(store: &dyn DocStore, path: &str) -> Result<Change, FormaError> {
    let pending = pending(store).await?;
    if let Some(change) = pending
        .into_iter()
        .find(|c| c.current.as_deref() == Some(path))
    {
        return Ok(change);
    }

    let now = Utc::now();
    let mut change = Change {
        id: uuid::Uuid::new_v4().simple().to_string(),
        original: None,
        current: Some(path.to_string()),
        base_type: None,
        first_at: now,
        last_at: now,
    };
    if store.stat(path).await?.is_some() {
        let doc = store.attach(path).await?;
        store
            .write_bytes(&change.base_path(), doc.bytes, &doc.media_type)
            .await?;
        change.original = Some(path.to_string());
        change.base_type = Some(doc.media_type);
    }
    Ok(change)
}

/// Records `change` as it now stands, after the file was changed.
///
/// A file the agent created and then deleted leaves nothing to review, so its
/// record goes.
pub async fn save(store: &dyn DocStore, mut change: Change) -> Result<(), FormaError> {
    if change.original.is_none() && change.current.is_none() {
        return store.delete(&change.record_path()).await;
    }
    change.last_at = Utc::now();
    let json = serde_json::to_vec(&change)?;
    store
        .write_bytes(&change.record_path(), json, "application/json")
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_only_the_history_root_is_hidden() {
        assert!(is_hidden(".history/changes/x.json"));
        assert!(is_hidden("/.history"));
        assert!(!is_hidden("notes/.history"));
        assert!(!is_hidden(".historical.md"));
        assert!(check_visible("notes/plan.md").is_ok());
        assert!(check_visible(".history/base/x").is_err());
    }

    #[test]
    fn test_a_change_expires_three_days_after_it_was_last_touched() {
        let now = Utc::now();
        let change = |last_at| Change {
            id: "x".into(),
            original: None,
            current: Some("a.md".into()),
            base_type: None,
            first_at: now - Duration::days(10),
            last_at,
        };
        assert!(!change(now - Duration::days(2)).expired(now));
        assert!(change(now - Duration::days(3) - Duration::minutes(1)).expired(now));
    }
}

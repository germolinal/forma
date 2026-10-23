//! Serialising for telemetry (span attributes and logs): JSON, like
//! `serde_json::to_string`, without what means nothing to a person reading a
//! trace. Today that is Gemini's thought signatures: opaque blobs that must be
//! stored and sent back to the model, but that serde writes as arrays of
//! numbers, kilobytes of them, into every span that carries a conversation.
//!
//! A field opts out with `#[serde(skip_serializing_if = ...)]`, pointing at
//! [`omitted`] (or [`none_or_omitted`] for an `Option` that is also skipped
//! when empty). Those check a thread-local flag that only [`to_json`] sets, so
//! everything else (the database, the providers) serialises the field as
//! before. Serialisation is synchronous, so the flag cannot outlive the call.

use serde::Serialize;
use std::cell::Cell;

thread_local! {
    static FOR_TELEMETRY: Cell<bool> = const { Cell::new(false) };
}

/// `value` as JSON, for a span attribute or a log, without the fields that
/// opt out of telemetry.
pub fn to_json<T: Serialize + ?Sized>(value: &T) -> String {
    // Restores the flag even if serialising panics, and nests.
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            FOR_TELEMETRY.with(|f| f.set(self.0));
        }
    }
    let _restore = Restore(FOR_TELEMETRY.with(|f| f.replace(true)));
    serde_json::to_string(value).unwrap_or_default()
}

/// For `skip_serializing_if`: skips the field while serialising for telemetry.
pub fn omitted<T>(_: &T) -> bool {
    FOR_TELEMETRY.with(Cell::get)
}

/// For `skip_serializing_if` on an `Option`: skips it when empty, and while
/// serialising for telemetry.
pub fn none_or_omitted<T>(value: &Option<T>) -> bool {
    value.is_none() || omitted(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Message {
        text: &'static str,
        #[serde(skip_serializing_if = "none_or_omitted")]
        signature: Option<Vec<u8>>,
    }

    #[test]
    fn telemetry_leaves_out_what_opts_out_and_nothing_else_does() {
        let m = Message {
            text: "hi",
            signature: Some(vec![1, 2, 3]),
        };
        assert_eq!(to_json(&m), r#"{"text":"hi"}"#);
        // Everything else still sees it, before and after.
        assert_eq!(
            serde_json::to_string(&m).unwrap(),
            r#"{"text":"hi","signature":[1,2,3]}"#
        );
        assert_eq!(to_json(&[&m, &m]), r#"[{"text":"hi"},{"text":"hi"}]"#);
        assert!(!omitted(&()));
    }
}

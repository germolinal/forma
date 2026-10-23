//! Files that are in a conversation — what the person attached, what a model
//! or a tool generated — named so that an agent can refer to one.
//!
//! A model cannot hand bytes to a tool: it would have to write the whole file
//! out as base64, in a tool call. So every such file gets a handle instead,
//! shown to the model beside the file itself (see [`label`]), and a tool given
//! a handle looks the bytes up in the conversation ([`find`]).
//!
//! The handle is derived from the file's content, so it is the same on every
//! turn, needs storing nowhere, and two different files cannot share one.

use base64::Engine;
use protocols::{FormaError, FormaMessage, FormaMessagePart, FormaTextPart};
use sha2::{Digest, Sha256};

/// What every handle starts with.
const PREFIX: &str = "file-";

/// How many hex digits of the hash a handle keeps: plenty to keep the files of
/// one conversation apart, and short enough to read.
const HASH_DIGITS: usize = 12;

/// The handle of the file carried as `data_url`.
pub fn handle_of(data_url: &str) -> String {
    let digest = Sha256::digest(data_url.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{PREFIX}{}", &hex[..HASH_DIGITS])
}

/// The file a part carries inline, as a data URL; `None` for anything else —
/// text, a link, a workspace file (which is already somewhere it can be named).
pub fn data_url_of(part: &FormaMessagePart) -> Option<&str> {
    let url = match part {
        FormaMessagePart::File(p) => p.file.file_data.as_deref()?,
        FormaMessagePart::ImageUrl(p) => p.image_url.url.as_str(),
        _ => return None,
    };
    url.starts_with("data:").then_some(url)
}

/// The type a data URL declares, for the label. Never trusted for anything
/// else: what a file is gets decided from its bytes.
fn declared_type(data_url: &str) -> &str {
    data_url
        .strip_prefix("data:")
        .and_then(|rest| rest.split([';', ',']).next())
        .filter(|t| !t.is_empty())
        .unwrap_or("application/octet-stream")
}

/// Puts each inline file's handle in front of it, as text the model reads:
/// `[file-3f2a9c01b7e4: image/png]`.
pub fn label(messages: &mut [FormaMessage]) {
    for message in messages {
        let content = message.content.content_mut();
        let mut i = 0;
        while i < content.len() {
            if let Some(url) = data_url_of(&content[i]) {
                let text = format!("[{}: {}]", handle_of(url), declared_type(url));
                content.insert(i, FormaMessagePart::Text(FormaTextPart { text }));
                i += 1;
            }
            i += 1;
        }
    }
}

/// The data URL of the file `handle` names, among `messages` and `produced` (the
/// files a tool made this turn, not yet in the history).
pub fn find<'a>(
    messages: &'a [FormaMessage],
    produced: &'a [FormaMessagePart],
    handle: &str,
) -> Option<&'a str> {
    messages
        .iter()
        .flat_map(|m| m.content.content())
        .chain(produced)
        .filter_map(data_url_of)
        .find(|url| handle_of(url) == handle)
}

/// The bytes a data URL carries.
pub fn decode(data_url: &str) -> Result<Vec<u8>, FormaError> {
    let payload = data_url
        .split_once(";base64,")
        .map(|(_, payload)| payload)
        .ok_or_else(|| {
            FormaError::Input("that file is not stored in a form that can be saved".into())
        })?;
    base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| FormaError::Input(format!("that file could not be read: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocols::{FormaFile, FormaFilePart, FormaImagePart, FormaImageUrl};

    fn file(url: &str) -> FormaMessagePart {
        FormaMessagePart::File(FormaFilePart {
            file: FormaFile {
                file_data: Some(url.to_string()),
                file_id: None,
                filename: None,
            },
        })
    }

    #[test]
    fn test_a_handle_is_stable_and_tells_files_apart() {
        let a = handle_of("data:image/png;base64,AAAA");
        assert_eq!(a, handle_of("data:image/png;base64,AAAA"));
        assert_ne!(a, handle_of("data:image/png;base64,AAAB"));
        assert!(a.starts_with("file-"));
        assert_eq!(a.len(), PREFIX.len() + HASH_DIGITS);
    }

    #[test]
    fn test_only_inline_files_are_named() {
        assert!(data_url_of(&file("data:application/pdf;base64,JVBERg==")).is_some());
        let link = FormaMessagePart::ImageUrl(FormaImagePart {
            image_url: FormaImageUrl {
                url: "https://example.com/cat.png".into(),
                detail: None,
            },
        });
        assert!(data_url_of(&link).is_none());
        assert!(
            data_url_of(&FormaMessagePart::Text(FormaTextPart { text: "hi".into() })).is_none()
        );
    }

    #[test]
    fn test_label_names_each_file_where_it_is() {
        let url = "data:image/png;base64,iVBORw0KGgo=";
        let mut messages = vec![FormaMessage::new_user_with_parts(vec![
            FormaMessagePart::Text(FormaTextPart {
                text: "look".into(),
            }),
            file(url),
        ])];
        label(&mut messages);
        let content = messages[0].content.content();
        assert_eq!(content.len(), 3);
        let FormaMessagePart::Text(t) = &content[1] else {
            panic!("the label goes right before the file");
        };
        assert_eq!(t.text, format!("[{}: image/png]", handle_of(url)));
        assert!(matches!(content[2], FormaMessagePart::File(_)));
    }

    #[test]
    fn test_find_looks_in_the_history_and_in_this_turn() {
        let old = "data:image/png;base64,AAAA";
        let new = "data:image/png;base64,BBBB";
        let messages = vec![FormaMessage::new_user_with_parts(vec![file(old)])];
        let produced = vec![file(new)];
        assert_eq!(find(&messages, &produced, &handle_of(old)), Some(old));
        assert_eq!(find(&messages, &produced, &handle_of(new)), Some(new));
        assert_eq!(find(&messages, &produced, "file-000000000000"), None);
    }

    #[test]
    fn test_decode() {
        assert_eq!(decode("data:text/plain;base64,aGk=").unwrap(), b"hi");
        assert!(decode("data:text/plain,hi").is_err());
    }
}

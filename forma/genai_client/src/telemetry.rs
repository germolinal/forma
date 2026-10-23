//! What goes into traces, following the OpenTelemetry GenAI semantic
//! conventions: whether message content is recorded at all, and the JSON
//! shape it is recorded in.
//!
//! Content (the messages, the system instructions, a tool's arguments and
//! result) is opt-in in the conventions, and switched by the standard
//! `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT` variable. When it is
//! recorded, it is recorded once per span, in the conventions' compact shape
//! (`{"role", "parts": [...]}`): no message ids, timestamps or thought
//! signatures, and media as its kind only, never its data.

use crate::{AssistantMessage, OpenAIMessagePart, OpenAIToolChoice};
use serde_json::{Value, json};
use std::sync::LazyLock;

/// The standard switch for recording message content in GenAI spans.
pub const CAPTURE_CONTENT_ENV: &str = "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT";

static CAPTURE_CONTENT: LazyLock<bool> =
    LazyLock::new(|| capture_content_from(std::env::var(CAPTURE_CONTENT_ENV).ok().as_deref()));

/// Whether spans record message content (read once, at the first call).
pub fn capture_content() -> bool {
    *CAPTURE_CONTENT
}

/// What the variable's value means. `true` and the newer `SPAN_ONLY` and
/// `SPAN_AND_EVENT` record content on spans; anything else, or nothing, does
/// not: content is opt-in.
fn capture_content_from(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("true" | "1" | "span_only" | "span_and_event")
    )
}

/// A text part.
pub fn text_part(text: &str) -> Value {
    json!({ "type": "text", "content": text })
}

/// A part the conventions have no type for (an image, an audio clip, a
/// file): its kind, without its data.
pub fn media_part(kind: &str) -> Value {
    json!({ "type": kind })
}

/// A tool call. `arguments` is the model's JSON, recorded as JSON when it
/// parses, and as the string it was otherwise.
pub fn tool_call_part(id: &str, name: &str, arguments: &str) -> Value {
    let arguments = serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| arguments.into());
    json!({ "type": "tool_call", "id": id, "name": name, "arguments": arguments })
}

/// A tool's result, answering the call `id`.
pub fn tool_call_response_part(id: &str, response: &str) -> Value {
    json!({ "type": "tool_call_response", "id": id, "response": response })
}

/// A message: its role and its parts.
pub fn message(role: &str, parts: Vec<Value>) -> Value {
    json!({ "role": role, "parts": parts })
}

/// The parts of an assistant's turn: its text, then its tool calls.
pub fn assistant_parts(
    content: impl IntoIterator<Item = Value>,
    tool_calls: &[OpenAIToolChoice],
) -> Vec<Value> {
    content
        .into_iter()
        .chain(
            tool_calls
                .iter()
                .map(|c| tool_call_part(&c.id(), &c.name(), &c.arguments())),
        )
        .collect()
}

/// A provider-side message part, as a part.
pub fn openai_part(part: &OpenAIMessagePart) -> Value {
    match part {
        OpenAIMessagePart::Text(t) => text_part(&t.text),
        OpenAIMessagePart::ImageUrl(_) => media_part("image"),
        OpenAIMessagePart::InputAudio(_) => media_part("audio"),
        OpenAIMessagePart::File(_) => media_part("file"),
    }
}

/// `gen_ai.output.messages`: the model's answer, with why it stopped.
pub fn output_messages(answer: &AssistantMessage, finish_reason: Option<&str>) -> String {
    let mut m = message(
        "assistant",
        assistant_parts(answer.content.iter().map(openai_part), &answer.tool_calls),
    );
    if let Some(reason) = finish_reason {
        m["finish_reason"] = reason.into();
    }
    Value::Array(vec![m]).to_string()
}

/// `gen_ai.system_instructions`: the system prompt, as a list of parts.
pub fn system_instructions(text: &str) -> String {
    Value::Array(vec![text_part(text)]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_is_opt_in() {
        assert!(!capture_content_from(None));
        assert!(!capture_content_from(Some("")));
        assert!(!capture_content_from(Some("false")));
        assert!(!capture_content_from(Some("NO_CONTENT")));
        assert!(!capture_content_from(Some("EVENT_ONLY")));
        assert!(capture_content_from(Some("true")));
        assert!(capture_content_from(Some("TRUE")));
        assert!(capture_content_from(Some("SPAN_ONLY")));
        assert!(capture_content_from(Some("span_and_event")));
    }

    #[test]
    fn parts_take_the_conventions_shape() {
        assert_eq!(text_part("hi"), json!({ "type": "text", "content": "hi" }));
        assert_eq!(
            tool_call_part("c1", "get_weather", r#"{"location":"Paris"}"#),
            json!({ "type": "tool_call", "id": "c1", "name": "get_weather",
                    "arguments": { "location": "Paris" } })
        );
        // Arguments that are not JSON stay a string.
        assert_eq!(
            tool_call_part("c1", "t", "not json")["arguments"],
            json!("not json")
        );
        assert_eq!(
            tool_call_response_part("c1", "rainy"),
            json!({ "type": "tool_call_response", "id": "c1", "response": "rainy" })
        );
        // Compared as JSON: key order depends on serde_json's features.
        assert_eq!(
            serde_json::from_str::<Value>(&system_instructions("Be brief")).unwrap(),
            json!([{ "type": "text", "content": "Be brief" }])
        );
    }

    #[test]
    fn an_answer_records_its_text_its_tool_calls_and_why_it_stopped() {
        let answer: AssistantMessage = serde_json::from_value(json!({
            "content": [{ "type": "text", "text": "Checking." }],
            "tool_calls": [{ "type": "function", "id": "c1",
                             "function": { "name": "lookup", "arguments": "{\"q\":1}" } }],
            "thought_signature": [1, 2, 3]
        }))
        .unwrap();
        let out: Value =
            serde_json::from_str(&output_messages(&answer, Some("tool_calls"))).unwrap();
        assert_eq!(
            out,
            json!([{
                "role": "assistant",
                "parts": [
                    { "type": "text", "content": "Checking." },
                    { "type": "tool_call", "id": "c1", "name": "lookup", "arguments": { "q": 1 } }
                ],
                "finish_reason": "tool_calls"
            }])
        );
    }
}

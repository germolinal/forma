use genai_client::{AudioFormat, SSEvent, openai::v1::OpenAIMessagePart};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};

use crate::server::runtime::adapters::ClientSSE;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AiSdkV5Event {
    Abort {
        reason: String,
    },
    Error {
        #[serde(rename = "errorText")]
        error_text: String,
    },
    Start {
        #[serde(rename = "messageId")]
        message_id: String,
    },
    TextStart {
        id: String,
    },
    TextDelta {
        id: String,
        delta: String,
    },
    TextEnd {
        id: String,
    },
    ReasoningStart {
        id: String,
    },
    ReasoningDelta {
        id: String,
        delta: String,
    },
    ReasoningEnd {
        id: String,
    },
    ToolInputStart {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
    },
    ToolInputAvailable {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        input: serde_json::Value,
    },
    ToolOutputAvailable {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        output: serde_json::Value,
    },

    Finish,

    /// The AI SDK's own file part, which takes a URL — a `data:` URL is what
    /// an inline document becomes — plus the media type it should render as.
    File {
        url: String,
        #[serde(rename = "mediaType")]
        media_type: String,
    },

    /// Serializes as `{"type":"data-<name>","data":{...}}`.
    Data {
        name: String,
        data: serde_json::Value,
    },
}

impl Serialize for AiSdkV5Event {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Abort { reason } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("type", "abort")?;
                m.serialize_entry("reason", reason)?;
                m.end()
            }
            Self::Error { error_text } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("type", "error")?;
                m.serialize_entry("errorText", error_text)?;
                m.end()
            }
            Self::Start { message_id } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("type", "start")?;
                m.serialize_entry("messageId", message_id)?;
                m.end()
            }
            Self::TextStart { id } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("type", "text-start")?;
                m.serialize_entry("id", id)?;
                m.end()
            }
            Self::TextDelta { id, delta } => {
                let mut m = s.serialize_map(Some(3))?;
                m.serialize_entry("type", "text-delta")?;
                m.serialize_entry("id", id)?;
                m.serialize_entry("delta", delta)?;
                m.end()
            }
            Self::TextEnd { id } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("type", "text-end")?;
                m.serialize_entry("id", id)?;
                m.end()
            }
            Self::ReasoningStart { id } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("type", "reasoning-start")?;
                m.serialize_entry("id", id)?;
                m.end()
            }
            Self::ReasoningDelta { id, delta } => {
                let mut m = s.serialize_map(Some(3))?;
                m.serialize_entry("type", "reasoning-delta")?;
                m.serialize_entry("id", id)?;
                m.serialize_entry("delta", delta)?;
                m.end()
            }
            Self::ReasoningEnd { id } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("type", "reasoning-end")?;
                m.serialize_entry("id", id)?;
                m.end()
            }
            Self::ToolInputStart {
                tool_call_id,
                tool_name,
            } => {
                let mut m = s.serialize_map(Some(3))?;
                m.serialize_entry("type", "tool-input-start")?;
                m.serialize_entry("toolCallId", tool_call_id)?;
                m.serialize_entry("toolName", tool_name)?;
                m.end()
            }
            Self::ToolInputAvailable {
                tool_call_id,
                tool_name,
                input,
            } => {
                let mut m = s.serialize_map(Some(4))?;
                m.serialize_entry("type", "tool-input-available")?;
                m.serialize_entry("toolCallId", tool_call_id)?;
                m.serialize_entry("toolName", tool_name)?;
                m.serialize_entry("input", input)?;
                m.end()
            }
            Self::ToolOutputAvailable {
                tool_call_id,
                output,
            } => {
                let mut m = s.serialize_map(Some(3))?;
                m.serialize_entry("type", "tool-output-available")?;
                m.serialize_entry("toolCallId", tool_call_id)?;
                m.serialize_entry("output", output)?;
                m.end()
            }
            Self::Finish => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("type", "finish")?;
                m.end()
            }
            Self::File { url, media_type } => {
                let mut m = s.serialize_map(Some(3))?;
                m.serialize_entry("type", "file")?;
                m.serialize_entry("url", url)?;
                m.serialize_entry("mediaType", media_type)?;
                m.end()
            }
            Self::Data { name, data } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("type", &format!("data-{name}"))?;
                m.serialize_entry("data", data)?;
                m.end()
            }
        }
    }
}

impl ClientSSE for AiSdkV5Event {
    fn translate(event: SSEvent) -> Option<Self> {
        let ret = match event {
            SSEvent::Error { error } => Self::Error { error_text: error },
            SSEvent::Start { message_id } => Self::Start { message_id },
            SSEvent::TextStart { id } => Self::TextStart { id },
            SSEvent::TextDelta { id, delta } => Self::TextDelta { id, delta },
            SSEvent::TextEnd { id } => Self::TextEnd { id },
            SSEvent::ReasoningStart { id } => Self::ReasoningStart { id },
            SSEvent::ReasoningDelta { id, delta } => Self::ReasoningDelta { id, delta },
            SSEvent::ReasoningEnd { id } => Self::ReasoningEnd { id },
            SSEvent::ToolInputStart {
                tool_call_id,
                tool_name,
                ..
            } => Self::ToolInputStart {
                tool_call_id,
                tool_name,
            },
            SSEvent::Usage { .. } | SSEvent::ToolUsage { .. } => None?,
            SSEvent::ToolInputAvailable {
                tool_call_id,
                tool_name,
                input,
                ..
            } => Self::ToolInputAvailable {
                tool_call_id,
                tool_name,
                input,
            },
            SSEvent::ToolOutputAvailable {
                tool_call_id,
                output,
            } => Self::ToolOutputAvailable {
                tool_call_id,
                output,
            },
            SSEvent::StartStep { .. } => None?,
            SSEvent::Finish => Self::Finish,
            SSEvent::RequestFeedback {
                tool_name,
                arguments,
                tool_call_id,
                ..
            } => Self::ToolInputAvailable {
                tool_call_id,
                tool_name,
                input: arguments,
            },
            SSEvent::StillSuspended => Self::Data {
                name: "still-suspended".to_string(),
                data: serde_json::json!({}),
            },
            SSEvent::GroundingScore { score } => Self::Data {
                name: "grounding-score".to_string(),
                data: serde_json::json!({
                    "score": score,
                }),
            },
            SSEvent::Reference {
                title,
                url,
                snippet,
                extractive_answer,
            } => Self::Data {
                name: "reference".to_string(),
                data: serde_json::json!({
                    "title": title,
                    "url": url,
                    "snippet": snippet,
                    "extractiveAnswer": extractive_answer,
                }),
            },
            SSEvent::FileAvailable { part, .. } => {
                let (url, media_type) = file_part_url(&part)?;
                Self::File { url, media_type }
            }
        };
        Some(ret)
    }
}

/// The URL and media type the AI SDK needs to render a content part.
///
/// Returns `None` for a text part, which has no business being announced as a
/// file, and for a document carrying nothing renderable.
fn file_part_url(part: &OpenAIMessagePart) -> Option<(String, String)> {
    match part {
        OpenAIMessagePart::Text(_) => None,
        OpenAIMessagePart::ImageUrl(p) => {
            let url = p.image_url.url.clone();
            let media_type = data_url_media_type(&url).unwrap_or("image/*").to_string();
            Some((url, media_type))
        }
        OpenAIMessagePart::File(p) => {
            let url = p.file.file_data.clone()?;
            let media_type = data_url_media_type(&url)
                .unwrap_or("application/octet-stream")
                .to_string();
            Some((url, media_type))
        }
        OpenAIMessagePart::InputAudio(p) => {
            // Audio arrives as bare base64 named by an enum, so the data URL
            // the SDK wants has to be rebuilt from the two.
            let media_type = match p.input_audio.format {
                AudioFormat::Mp3 => "audio/mpeg",
                AudioFormat::Wav => "audio/wav",
                AudioFormat::Flac => "audio/flac",
                AudioFormat::Opus => "audio/ogg",
                AudioFormat::Pcm16 => "audio/L16",
            };
            Some((
                format!("data:{};base64,{}", media_type, p.input_audio.data),
                media_type.to_string(),
            ))
        }
    }
}

/// The media type stated by a `data:` URL, if it is one.
fn data_url_media_type(url: &str) -> Option<&str> {
    let meta = url.strip_prefix("data:")?.split(',').next()?;
    meta.split(';').next().filter(|m| !m.is_empty())
}

#[cfg(test)]
mod testing {
    use super::AiSdkV5Event;

    #[test]
    fn file_event_becomes_an_sdk_file_part() {
        use crate::server::runtime::adapters::ClientSSE;
        use genai_client::{
            SSEvent,
            openai::v1::{OpenAIImagePart, OpenAIImageUrl, OpenAIMessagePart},
        };

        let event = SSEvent::FileAvailable {
            id: "m1".to_string(),
            part: OpenAIMessagePart::ImageUrl(OpenAIImagePart {
                image_url: OpenAIImageUrl {
                    url: "data:image/png;base64,iVBORw0KGgo=".to_string(),
                    detail: None,
                },
            }),
        };

        let translated = AiSdkV5Event::translate(event).expect("should translate");
        assert_eq!(
            serde_json::to_value(&translated).expect("serialises"),
            serde_json::json!({
                "type": "file",
                "url": "data:image/png;base64,iVBORw0KGgo=",
                "mediaType": "image/png",
            })
        );
    }

    #[test]
    fn serialise_abort() {
        let sse = AiSdkV5Event::Abort {
            reason: "aborted by user".to_string(),
        };
        assert_eq!(
            "{\"type\":\"abort\",\"reason\":\"aborted by user\"}",
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_start() {
        let sse = AiSdkV5Event::Start {
            message_id: "123123".to_string(),
        };
        assert_eq!(
            "{\"type\":\"start\",\"messageId\":\"123123\"}",
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }
    #[test]
    fn serialise_text_start() {
        let sse = AiSdkV5Event::TextStart {
            id: "msg_68679a454370819ca74c8eb3d04379630dd1afb72306ca5d".to_string(),
        };
        assert_eq!(
            r#"{"type":"text-start","id":"msg_68679a454370819ca74c8eb3d04379630dd1afb72306ca5d"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_text_delta() {
        let sse = AiSdkV5Event::TextDelta {
            id: "msg_68679a454370819ca74c8eb3d04379630dd1afb72306ca5d".to_string(),
            delta: "Hello".to_string(),
        };
        assert_eq!(
            r#"{"type":"text-delta","id":"msg_68679a454370819ca74c8eb3d04379630dd1afb72306ca5d","delta":"Hello"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_text_end() {
        let sse = AiSdkV5Event::TextEnd {
            id: "msg_68679a454370819ca74c8eb3d04379630dd1afb72306ca5d".to_string(),
        };
        assert_eq!(
            r#"{"type":"text-end","id":"msg_68679a454370819ca74c8eb3d04379630dd1afb72306ca5d"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_reasoning_start() {
        let sse = AiSdkV5Event::ReasoningStart {
            id: "reasoning_123".to_string(),
        };
        assert_eq!(
            r#"{"type":"reasoning-start","id":"reasoning_123"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_reasoning_delta() {
        let sse = AiSdkV5Event::ReasoningDelta {
            id: "reasoning_123".to_string(),
            delta: "This is some reasoning".to_string(),
        };
        assert_eq!(
            r#"{"type":"reasoning-delta","id":"reasoning_123","delta":"This is some reasoning"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_reasoning_end() {
        let sse = AiSdkV5Event::ReasoningEnd {
            id: "reasoning_123".to_string(),
        };
        assert_eq!(
            r#"{"type":"reasoning-end","id":"reasoning_123"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_tool_input_start() {
        let sse = AiSdkV5Event::ToolInputStart {
            tool_name: "getWeatherInformation".to_string(),
            tool_call_id: "call_fJdQDqnXeGxTmr4E3YPSR7Ar".to_string(),
        };
        assert_eq!(
            r#"{"type":"tool-input-start","toolCallId":"call_fJdQDqnXeGxTmr4E3YPSR7Ar","toolName":"getWeatherInformation"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_tool_input_available() {
        let sse = AiSdkV5Event::ToolInputAvailable {
            tool_name: "getWeatherInformation".to_string(),
            tool_call_id: "call_fJdQDqnXeGxTmr4E3YPSR7Ar".to_string(),
            input: serde_json::json!({"city": "San Francisco"}),
        };
        assert_eq!(
            r#"{"type":"tool-input-available","toolCallId":"call_fJdQDqnXeGxTmr4E3YPSR7Ar","toolName":"getWeatherInformation","input":{"city":"San Francisco"}}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }
    #[test]
    fn serialise_tool_output_available() {
        let sse = AiSdkV5Event::ToolOutputAvailable {
            tool_call_id: "call_fJdQDqnXeGxTmr4E3YPSR7Ar".to_string(),
            output: serde_json::json!({"city": "San Francisco","weather":"sunny"}),
        };
        assert_eq!(
            r#"{"type":"tool-output-available","toolCallId":"call_fJdQDqnXeGxTmr4E3YPSR7Ar","output":{"city":"San Francisco","weather":"sunny"}}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_finish() {
        let sse = AiSdkV5Event::Finish;
        assert_eq!(
            r#"{"type":"finish"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_data() {
        let sse = AiSdkV5Event::Data {
            name: "weather".to_string(),
            data: serde_json::json!({"location": "SF", "temperature": 100}),
        };
        assert_eq!(
            r#"{"type":"data-weather","data":{"location":"SF","temperature":100}}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }
}

use genai_client::{AudioFormat, SSEvent, openai::v1::OpenAIMessagePart};
use serde::{Deserialize, Serialize};

use crate::server::runtime::adapters::ClientSSE;

/// One event of the stream the `ai-sdk-v5` client sends, in the protocol of
/// Vercel's AI SDK (v5): each is the JSON of one `data:` line.
///
/// `type` says what happened. Text, reasoning and tool calls follow the AI
/// SDK's own UI message stream parts; what only Forma reports travels as the
/// SDK's custom `data-*` parts.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type")]
#[serde(rename_all = "kebab-case")]
pub enum AiSdkV5Event {
    /// The stream was aborted.
    Abort {
        /// Why.
        reason: String,
    },
    /// The agent failed; the stream ends here.
    Error {
        /// What went wrong.
        #[serde(rename = "errorText")]
        error_text: String,
    },
    /// The agent started working on the message. Always the first event.
    Start {
        /// The id of the job, which the usage report at the end repeats.
        #[serde(rename = "messageId")]
        message_id: String,
    },
    /// A text part begins.
    TextStart {
        /// The id shared by the part's `text-delta`s and `text-end`.
        id: String,
    },
    /// More text for the part `id` began.
    TextDelta {
        /// The part this text belongs to.
        id: String,
        /// The text, to append to what came before.
        delta: String,
    },
    /// The text part `id` is complete.
    TextEnd {
        /// The part that ended.
        id: String,
    },
    /// A reasoning part begins (models that expose their reasoning).
    ReasoningStart {
        /// The id shared by the part's deltas and end.
        id: String,
    },
    /// More reasoning for the part `id`.
    ReasoningDelta {
        /// The part this reasoning belongs to.
        id: String,
        /// The reasoning, to append to what came before.
        delta: String,
    },
    /// The reasoning part `id` is complete.
    ReasoningEnd {
        /// The part that ended.
        id: String,
    },
    /// The agent is about to call a tool.
    ToolInputStart {
        /// The call's id, which its input and output repeat.
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        /// The tool's name, as the agent defines it.
        #[serde(rename = "toolName")]
        tool_name: String,
    },
    /// The arguments of a tool call. Also how a call that waits for an answer
    /// (an approval, or a delegated tool's result) is announced: the stream
    /// then ends without its output, and the call is answered with
    /// `/v1/resume`.
    ToolInputAvailable {
        /// The call's id.
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        /// The tool's name.
        #[serde(rename = "toolName")]
        tool_name: String,
        /// The arguments, as the model wrote them.
        input: serde_json::Value,
    },
    /// What a tool call returned.
    ToolOutputAvailable {
        /// The call's id.
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        /// The tool's result.
        output: serde_json::Value,
    },
    /// The job is done, and its turn stored: the next message can be sent.
    Finish,
    /// A file produced during the turn: an image a tool generated, a file it
    /// fetched, or media the model returned. The AI SDK's own file part.
    File {
        /// A `data:` URL, or a URL the client can reach.
        url: String,
        /// What the file is (`image/png`, …).
        #[serde(rename = "mediaType")]
        media_type: String,
    },
    /// Answer to a `/v1/resume`: other calls of the same turn are still
    /// waiting for an answer.
    DataStillSuspended {
        /// Always empty.
        data: EmptyData,
    },
    /// How well the answer is grounded in the sources it used.
    DataGroundingScore {
        /// The score.
        data: GroundingScoreData,
    },
    /// A document used as a source for the answer.
    DataReference {
        /// The document.
        data: ReferenceData,
    },
}

/// The `data` of an event that carries nothing.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EmptyData {}

/// The `data` of a `data-grounding-score` event.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GroundingScoreData {
    /// How well the answer is supported by its sources.
    pub score: f64,
}

/// The `data` of a `data-reference` event.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReferenceData {
    /// The document's title.
    pub title: String,
    /// Where the document is.
    pub url: String,
    /// The passage that was used, if known.
    pub snippet: Option<String>,
    /// Passages that answer the question directly, if the source gave any.
    #[serde(rename = "extractiveAnswer")]
    pub extractive_answer: Option<Vec<String>>,
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
            SSEvent::StillSuspended => Self::DataStillSuspended { data: EmptyData {} },
            SSEvent::GroundingScore { score } => Self::DataGroundingScore {
                data: GroundingScoreData { score },
            },
            SSEvent::Reference {
                title,
                url,
                snippet,
                extractive_answer,
            } => Self::DataReference {
                data: ReferenceData {
                    title,
                    url,
                    snippet,
                    extractive_answer,
                },
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
pub(crate) fn file_part_url(part: &OpenAIMessagePart) -> Option<(String, String)> {
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
    use super::{AiSdkV5Event, EmptyData, GroundingScoreData, ReferenceData};

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
        // Forma's own events travel as the AI SDK's `data-*` parts.
        let sse = AiSdkV5Event::DataStillSuspended { data: EmptyData {} };
        assert_eq!(
            r#"{"type":"data-still-suspended","data":{}}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
        let sse = AiSdkV5Event::DataGroundingScore {
            data: GroundingScoreData { score: 0.5 },
        };
        assert_eq!(
            r#"{"type":"data-grounding-score","data":{"score":0.5}}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
        let sse = AiSdkV5Event::DataReference {
            data: ReferenceData {
                title: "Forma".to_string(),
                url: "https://example.com".to_string(),
                snippet: None,
                extractive_answer: Some(vec!["yes".to_string()]),
            },
        };
        assert_eq!(
            r#"{"type":"data-reference","data":{"title":"Forma","url":"https://example.com","snippet":null,"extractiveAnswer":["yes"]}}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }
}

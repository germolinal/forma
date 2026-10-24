use crate::{
    AssistantMessage,
    openai::v1::{OpenAIMessagePart, OpenAITextPart, OpenAIUsage},
};
use serde::{Deserialize, Serialize};

impl std::ops::AddAssign<SSEvent> for AssistantMessage {
    fn add_assign(&mut self, event: SSEvent) {
        match event {
            SSEvent::TextStart { .. } => {
                let empty_part = OpenAIMessagePart::Text(OpenAITextPart::default());
                self.content.push(empty_part);
            }
            SSEvent::TextDelta { delta, .. } => {
                let mut appended = false;
                for p in self.content.iter_mut().rev() {
                    if let OpenAIMessagePart::Text(OpenAITextPart { text }) = p {
                        text.push_str(&delta);
                        appended = true;
                        break;
                    }
                }
                if !appended {
                    panic!("trying to append text to a message that has no text parts")
                }
            }
            SSEvent::FileAvailable { part, .. } => {
                self.content.push(part);
            }
            _ => { /* Nothing to do about these */ }
        }
    }
}

/// What one LLM call consumed, in the shape a bill needs.
///
/// This is a *normalisation* of [`OpenAIUsage`], not a copy of it. Providers
/// disagree about what their totals include, and the disagreement shows up as
/// a wrong invoice rather than a parse error, so the convention is pinned here,
/// once, at the only layer that still knows which provider produced the numbers:
///
/// - `input_tokens` counts every prompt token, `cached_input_tokens` included.
/// - `output_tokens` counts every generated token, `reasoning_tokens` included.
///
/// `cached_input_tokens` and `reasoning_tokens` are therefore *subsets* of their
/// parents, carried separately because they are priced differently — cached
/// input is discounted, reasoning is billed at the output rate. Adding either to
/// its parent double-charges.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TokenUsage {
    /// Prompt tokens, cached ones included.
    pub input_tokens: usize,
    /// The subset of `input_tokens` the provider served from its prompt cache.
    /// Billed at a discount, so ignoring it over-charges.
    pub cached_input_tokens: usize,
    /// Generated tokens, reasoning included.
    pub output_tokens: usize,
    /// The subset of `output_tokens` spent on reasoning the client never sees.
    /// Billed like any other output token.
    pub reasoning_tokens: usize,
    /// Audio tokens in the prompt, a subset of `input_tokens`. Priced far above
    /// text tokens by every provider that supports them.
    pub input_audio_tokens: usize,
    /// Audio tokens generated, a subset of `output_tokens`.
    pub output_audio_tokens: usize,
}

impl From<&OpenAIUsage> for TokenUsage {
    /// Relies on [`OpenAIUsage`] carrying OpenAI's semantics whoever produced
    /// it — in particular `completion_tokens` including reasoning tokens. The
    /// Vertex conversion normalises to that before this runs.
    fn from(value: &OpenAIUsage) -> Self {
        let prompt = value.prompt_tokens_details.unwrap_or_default();
        let completion = value.completion_tokens_details.unwrap_or_default();
        Self {
            input_tokens: value.prompt_tokens,
            cached_input_tokens: prompt.cached_tokens,
            output_tokens: value.completion_tokens,
            reasoning_tokens: completion.reasoning_tokens,
            input_audio_tokens: prompt.audio_tokens,
            output_audio_tokens: completion.audio_tokens,
        }
    }
}

/// What one `(provider, server_address, model)` consumed across a whole run.
///
/// Provider is half the key, not decoration: the same model name reached
/// through two backends is two different prices, and through a local Ollama it
/// is free.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ModelUsage {
    /// The backend that served the calls, by its OpenTelemetry GenAI name
    /// (`ollama`, `openai`, `groq`, `gcp.vertex_ai`).
    pub provider: String,
    /// The host the calls went to (OpenTelemetry's `server.address`): with
    /// `provider`, who served them, and so who bills them. Every
    /// OpenAI-compatible server speaks `openai`; the host tells them apart.
    #[serde(default)]
    pub server_address: String,
    /// The model the provider reports having served.
    pub model: String,
    /// Provider round trips, which some plans bill per-request on top of tokens.
    pub calls: usize,
    #[serde(flatten)]
    pub usage: TokenUsage,
}

/// What a tool consumed beyond being called, in a unit it is priced by: the
/// seconds a code-execution sandbox ran, for instance. Summed per
/// `(tool_kind, unit)` across a run.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ToolUsage {
    /// The tool's kind (`code-execution`, …).
    pub tool_kind: String,
    /// What `quantity` counts (`second`, …).
    pub unit: String,
    /// How many `unit`s, over the whole job.
    pub quantity: u64,
}

/// Groups [`SSEvent::Usage`] events by `(provider, server_address, model)`, lists the kind of
/// every tool called ([`SSEvent::ToolInputAvailable`]), and sums
/// [`SSEvent::ToolUsage`] by kind and unit.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UsageTally {
    /// What each provider and model consumed, one entry per pair.
    ///
    /// Insertion-ordered so a run's first model leads the report, which makes
    /// logs and fixtures stable to read.
    pub tokens: Vec<ModelUsage>,
    /// The kind of every tool called, once per call.
    pub tools: Vec<String>,
    /// What tools consumed in units of their own (absent when nothing).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_usage: Vec<ToolUsage>,
    /// Contains the unique JobID
    pub job_id: String,
}

impl UsageTally {
    pub fn new_empty() -> Self {
        Self {
            tokens: Vec::with_capacity(0),
            tools: Vec::with_capacity(0),
            tool_usage: Vec::with_capacity(0),
            job_id: String::new(),
        }
    }

    /// True when there is nothing to bill: no provider was called — everything
    /// came from cache, or the run died before the first model replied — and no
    /// tool ran either. Tools count because some runners bill through to a paid
    /// third-party API, so a run can cost money without spending a token.
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty() && self.tools.is_empty() && self.tool_usage.is_empty()
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one event in. Anything that is not a [`SSEvent::Usage`] is ignored,
    /// so callers can hand it every event they see.
    pub fn add(&mut self, event: &SSEvent) {
        match event {
            SSEvent::Start { message_id } => {
                self.job_id = message_id.clone();
            }
            SSEvent::Usage {
                model,
                provider,
                server_address,
                usage,
                ..
            } => {
                let entry = match self.tokens.iter_mut().find(|e| {
                    e.model == *model
                        && e.provider == *provider
                        && e.server_address == *server_address
                }) {
                    Some(e) => e,
                    None => {
                        self.tokens.push(ModelUsage {
                            provider: provider.clone(),
                            server_address: server_address.clone(),
                            model: model.clone(),
                            ..Default::default()
                        });
                        self.tokens
                            .last_mut()
                            .expect("just pushed an entry, so there is a last one")
                    }
                };

                entry.calls += 1;
                entry.usage.input_tokens += usage.input_tokens;
                entry.usage.cached_input_tokens += usage.cached_input_tokens;
                entry.usage.output_tokens += usage.output_tokens;
                entry.usage.reasoning_tokens += usage.reasoning_tokens;
                entry.usage.input_audio_tokens += usage.input_audio_tokens;
                entry.usage.output_audio_tokens += usage.output_audio_tokens;
            }
            SSEvent::ToolInputAvailable { tool_kind, .. } => {
                self.tools.push(tool_kind.clone());
            }
            SSEvent::ToolUsage {
                tool_kind,
                unit,
                quantity,
            } => match self
                .tool_usage
                .iter_mut()
                .find(|u| u.tool_kind == *tool_kind && u.unit == *unit)
            {
                Some(u) => u.quantity += quantity,
                None => self.tool_usage.push(ToolUsage {
                    tool_kind: tool_kind.clone(),
                    unit: unit.clone(),
                    quantity: *quantity,
                }),
            },
            _ => {}
        };
    }

    /// Folds already-grouped entries in, so several runs can report as one.
    ///
    /// A single webhook delivery can carry several messages, each its own agent
    /// run: merging keeps one row per `(provider, server_address, model)` for the whole request
    /// rather than emitting duplicate rows a consumer would have to re-group.
    pub fn merge(&mut self, entries: &[ModelUsage]) {
        for incoming in entries {
            match self.tokens.iter_mut().find(|e| {
                e.model == incoming.model
                    && e.provider == incoming.provider
                    && e.server_address == incoming.server_address
            }) {
                Some(e) => {
                    e.calls += incoming.calls;
                    e.usage.input_tokens += incoming.usage.input_tokens;
                    e.usage.cached_input_tokens += incoming.usage.cached_input_tokens;
                    e.usage.output_tokens += incoming.usage.output_tokens;
                    e.usage.reasoning_tokens += incoming.usage.reasoning_tokens;
                    e.usage.input_audio_tokens += incoming.usage.input_audio_tokens;
                    e.usage.output_audio_tokens += incoming.usage.output_audio_tokens;
                }
                None => self.tokens.push(incoming.clone()),
            }
        }
    }
}

/// Something that happened while an agent ran: what a streaming client is
/// sent, one event per `data:` line, its `type` saying what happened.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum SSEvent {
    /// The agent failed. Nothing follows it.
    Error {
        /// What went wrong.
        error: String,
    },
    /// The agent started working on the message. Always the first event.
    Start {
        /// The id of the job, which its usage report repeats (`job_id`).
        message_id: String,
    },
    /// A text part begins.
    TextStart {
        /// The id its deltas and its end share.
        id: String,
    },
    /// More text for the part `id`.
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
        /// The id its deltas and its end share.
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
    /// A tool is about to be called.
    ToolInputStart {
        /// The call's id, which its input and output repeat.
        tool_call_id: String,
        /// The tool's name, as the agent defines it.
        tool_name: String,
        /// What the tool is (`tavily-web-search`, `workspace`,
        /// `code-execution`, …; see `Tool::kind`), which is what a client
        /// decides how to show the call by.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        tool_type: String,
        /// The group the tool belongs to: the MCP server or the definition
        /// that expanded into it.
        bundle: Option<String>,
    },
    /// The arguments of a tool call.
    ToolInputAvailable {
        /// The call's id.
        tool_call_id: String,
        /// The tool's name.
        tool_name: String,
        /// Which kind of tool this is, matching the `type` tag of `ToolRunner`
        /// (`mcp`, `api-call`, `tavily-web-search`, …).
        ///
        /// `tool_name` is the agent author's label and says nothing about cost;
        /// the kind is what a price list can key on, since some runners bill
        /// through to a paid third-party API. Optional so that an event decoded
        /// from an older runtime still parses.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        tool_kind: String,
        /// The arguments, as the model wrote them.
        input: serde_json::Value,
    },
    /// What a tool call returned.
    ToolOutputAvailable {
        /// The call's id.
        tool_call_id: String,
        /// The tool's result.
        output: serde_json::Value,
    },
    /// A node of the agent's workflow started working.
    StartStep {
        /// The node's id.
        id: String,
    },
    /// The job is done, and its turn stored: the next message can be sent.
    Finish,
    /// The agent stopped at a tool call that needs an answer: an approval
    /// (`confirm_before_calling`), or a delegated tool's result. Answer it
    /// with `/v1/resume`. `finish` follows.
    RequestFeedback {
        /// The call's id: what `/v1/resume` answers.
        tool_call_id: String,
        /// The node that made the call.
        node_id: String,
        /// The tool the agent wants to call.
        tool_name: String,
        /// The arguments the agent proposed.
        arguments: serde_json::Value,
        /// Whether the call was made by the agent's `start` workflow itself,
        /// rather than inside a workflow tool.
        from_root: bool,
        /// As in `ToolInputStart`: `delegated` for a tool the client runs.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        tool_type: String,
        /// As in `ToolInputStart`.
        bundle: Option<String>,
    },
    /// The answer to a `/v1/resume`: other calls of the same turn are still
    /// waiting for an answer.
    StillSuspended,
    /// Reports what a single LLM call consumed, so the proxy in front of the
    /// runtime can bill it without knowing anything about providers.
    ///
    /// Never streamed to a client: the runtime adds these up into the usage
    /// report that ends the stream.
    ///
    /// Emitted once per call that actually reached a provider — including calls
    /// made by non-root nodes, whose text never reaches the client. Everything
    /// else in this enum is gated on `is_root` because it exists to be
    /// displayed; this exists to be charged for, and an inner node's tokens cost
    /// exactly as much as the root's. A cached result emits nothing, because no
    /// provider call was made.
    ///
    /// One run therefore produces many of these, and a consumer is expected to
    /// group them by `(provider, server_address, model)` and sum.
    Usage {
        /// The node that made the call, so spend can be attributed within a
        /// graph rather than only to the run as a whole.
        node_id: String,
        /// The model the provider reports having *served*, which is not always
        /// the one configured: aliases resolve to dated snapshots, and a bill
        /// should name what ran.
        model: String,
        /// The backend that served the call, by its OpenTelemetry GenAI name
        /// (`ollama`, `openai`, `groq`, `gcp.vertex_ai`). The same model name
        /// costs different amounts through different providers, so this is
        /// half the price key, not decoration.
        provider: String,
        /// The host the call went to (OpenTelemetry's `server.address`): with
        /// `provider`, who served it. Every OpenAI-compatible server speaks
        /// `openai`, so it is the host that tells OpenAI from OpenRouter.
        /// Empty from runtimes that predate it.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        server_address: String,
        /// What the call consumed.
        usage: TokenUsage,
    },
    /// What a tool consumed in a unit it is priced by, beyond the call itself:
    /// the seconds a code-execution sandbox was billed for, say. Like
    /// [`SSEvent::Usage`], it is for the bill and never reaches a client.
    ToolUsage {
        /// The tool's kind, as in [`SSEvent::ToolInputAvailable`].
        tool_kind: String,
        /// What `quantity` counts (`second`, …). Part of the price key.
        unit: String,
        /// How many `unit`s.
        quantity: u64,
    },
    /// Contains a reference to a document that was used as context for the model.
    Reference {
        /// The document's title.
        title: String,
        /// Where the document is.
        url: String,
        /// The passage that was used, if known.
        snippet: Option<String>,
        /// Passages that answer the question directly, if the source gave
        /// any.
        extractive_answer: Option<Vec<String>>,
    },
    /// Contains the score of the grounding of the model's response.
    GroundingScore {
        /// How well the answer is supported by its sources.
        score: f64,
    },
    /// A document produced during the turn: an image a tool generated, a file
    /// it fetched, or media the model itself returned.
    ///
    /// The part is carried verbatim rather than as a URL or a blob of bytes,
    /// so that the one event serves both purposes — the client renders it, and
    /// folding it into the assistant's message is what lets the model refer
    /// back to what it produced on the next turn. A tool emits this the way
    /// [`SSEvent::Reference`] is emitted, returning only a summary to the LLM
    /// through its own JSON result.
    FileAvailable {
        /// Groups the file with the message being built, as `TextDelta` does.
        id: String,
        /// The file: an `image_url` or `file` part, usually a `data:` URL.
        part: OpenAIMessagePart,
    },
}

#[cfg(test)]
mod testing {
    use serde_json::Value;

    use crate::{
        AssistantMessage,
        events::{SSEvent, TokenUsage, ToolUsage, UsageTally},
        openai::v1::{
            OpenAICompletionTokenDetails, OpenAIMessagePart, OpenAIPromptTokenDetails, OpenAIUsage,
        },
    };

    #[test]
    fn file_event_becomes_a_content_part() {
        // The document has to land in the message, not just on the client, so
        // that the next turn's history still contains it.
        let mut message = AssistantMessage::default();
        message += SSEvent::TextStart { id: "t".into() };
        message += SSEvent::TextDelta {
            id: "t".into(),
            delta: "here you go".into(),
        };
        message += SSEvent::FileAvailable {
            id: "t".into(),
            part: OpenAIMessagePart::ImageUrl(crate::openai::v1::OpenAIImagePart {
                image_url: crate::openai::v1::OpenAIImageUrl {
                    url: "data:image/png;base64,iVBORw0KGgo=".into(),
                    detail: None,
                },
            }),
        };

        assert_eq!(message.content.len(), 2);
        assert!(matches!(message.content[0], OpenAIMessagePart::Text(_)));
        match &message.content[1] {
            OpenAIMessagePart::ImageUrl(p) => {
                assert_eq!(p.image_url.url, "data:image/png;base64,iVBORw0KGgo=");
            }
            other => panic!("expected an image part, got {other:?}"),
        }
    }

    #[test]
    fn file_event_round_trips_on_the_wire() {
        let event = SSEvent::FileAvailable {
            id: "msg-1".into(),
            part: OpenAIMessagePart::File(crate::openai::v1::OpenAIFilePart {
                file: crate::openai::v1::OpenAIFile {
                    file_data: Some("data:application/pdf;base64,JVBERi0=".into()),
                    file_id: None,
                    filename: Some("report.pdf".into()),
                },
            }),
        };

        let json: Value = serde_json::to_value(&event).expect("serialises");
        assert_eq!(json["type"], "file_available");
        // The nested part keeps its own tag, which is what lets a client
        // switch on the kind of document it just received.
        assert_eq!(json["part"]["type"], "file");
        assert_eq!(json["part"]["file"]["filename"], "report.pdf");

        let back: SSEvent = serde_json::from_value(json).expect("deserialises");
        assert!(matches!(back, SSEvent::FileAvailable { .. }));
    }

    #[test]
    fn append_start_text_in_empty() {
        let mut message = AssistantMessage::default();
        message += SSEvent::TextStart { id: "some".into() };
        assert_eq!(message.content.len(), 1); // Added one part.
        match &message.content[0] {
            OpenAIMessagePart::Text(p) => {
                assert!(p.text.is_empty());
            }
            _ => panic!("should have been text"),
        }
    }

    #[test]
    fn append_start_text_in_parts() {
        let mut message: AssistantMessage = "hi".to_string().into();

        message += SSEvent::TextStart { id: "some".into() };
        assert_eq!(message.content.len(), 2); // Added one part.
        match &message.content[0] {
            OpenAIMessagePart::Text(p) => {
                assert_eq!(p.text, "hi");
            }
            _ => panic!("should have been text"),
        }
        match &message.content[1] {
            OpenAIMessagePart::Text(p) => {
                assert!(p.text.is_empty());
            }
            _ => panic!("should have been text"),
        }
    }

    #[test]
    fn append_text() {
        let mut message = AssistantMessage::default();
        message += SSEvent::TextStart {
            id: "someid".into(),
        };
        message += SSEvent::ToolInputAvailable {
            tool_call_id: "".to_string(),
            tool_name: "".to_string(),
            tool_kind: Default::default(),
            input: Value::default(),
        };
        let msg = "some-text".to_string();
        message += SSEvent::TextDelta {
            id: "123".to_string(),
            delta: msg.clone(),
        };
        assert_eq!(message.content.len(), 1); // Added one part.
        match &message.content[0] {
            OpenAIMessagePart::Text(p) => {
                assert_eq!(p.text, msg);
            }
            _ => panic!("should have been text"),
        }
    }

    #[test]
    #[should_panic]
    fn append_events_without_starting1() {
        // Should fail because we did not inform this message that we would
        // start streaming text, and it therefore has no content
        let mut message = AssistantMessage::default();
        message += SSEvent::TextDelta {
            id: "123".to_string(),
            delta: "some-text".to_string(),
        };
    }

    #[test]
    fn serialise_start() {
        let sse = SSEvent::Start {
            message_id: "123123".to_string(),
        };
        assert_eq!(
            "{\"type\":\"start\",\"message_id\":\"123123\"}",
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }
    #[test]
    fn serialise_text_start() {
        let sse = SSEvent::TextStart {
            id: "msg_68679a454370819ca74c8eb3d04379630dd1afb72306ca5d".to_string(),
        };
        assert_eq!(
            r#"{"type":"text_start","id":"msg_68679a454370819ca74c8eb3d04379630dd1afb72306ca5d"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_text_delta() {
        let sse = SSEvent::TextDelta {
            id: "msg_68679a454370819ca74c8eb3d04379630dd1afb72306ca5d".to_string(),
            delta: "Hello".to_string(),
        };
        assert_eq!(
            r#"{"type":"text_delta","id":"msg_68679a454370819ca74c8eb3d04379630dd1afb72306ca5d","delta":"Hello"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_text_end() {
        let sse = SSEvent::TextEnd {
            id: "msg_68679a454370819ca74c8eb3d04379630dd1afb72306ca5d".to_string(),
        };
        assert_eq!(
            r#"{"type":"text_end","id":"msg_68679a454370819ca74c8eb3d04379630dd1afb72306ca5d"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_reasoning_start() {
        let sse = SSEvent::ReasoningStart {
            id: "reasoning_123".to_string(),
        };
        assert_eq!(
            r#"{"type":"reasoning_start","id":"reasoning_123"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_reasoning_delta() {
        let sse = SSEvent::ReasoningDelta {
            id: "reasoning_123".to_string(),
            delta: "This is some reasoning".to_string(),
        };
        assert_eq!(
            r#"{"type":"reasoning_delta","id":"reasoning_123","delta":"This is some reasoning"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_reasoning_end() {
        let sse = SSEvent::ReasoningEnd {
            id: "reasoning_123".to_string(),
        };
        assert_eq!(
            r#"{"type":"reasoning_end","id":"reasoning_123"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_tool_input_start() {
        let sse = SSEvent::ToolInputStart {
            tool_name: "getWeatherInformation".to_string(),
            tool_call_id: "call_fJdQDqnXeGxTmr4E3YPSR7Ar".to_string(),
            tool_type: "mcp".to_string(),
            bundle: Some("server".to_string()),
        };
        assert_eq!(
            r#"{"type":"tool_input_start","tool_call_id":"call_fJdQDqnXeGxTmr4E3YPSR7Ar","tool_name":"getWeatherInformation","tool_type":"mcp","bundle":"server"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_tool_input_available() {
        let sse = SSEvent::ToolInputAvailable {
            tool_name: "getWeatherInformation".to_string(),
            tool_call_id: "call_fJdQDqnXeGxTmr4E3YPSR7Ar".to_string(),
            tool_kind: "tavily-web-search".to_string(),
            input: serde_json::json!({"city": "San Francisco"}),
        };
        assert_eq!(
            r#"{"type":"tool_input_available","tool_call_id":"call_fJdQDqnXeGxTmr4E3YPSR7Ar","tool_name":"getWeatherInformation","tool_kind":"tavily-web-search","input":{"city":"San Francisco"}}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    /// An event from a runtime that predates `tool_kind` must still decode,
    /// since proxy and runtime are deployed separately and roll out in either
    /// order.
    #[test]
    fn deserialise_tool_input_available_without_kind() {
        let raw = r#"{"type":"tool_input_available","tool_call_id":"call_1","tool_name":"getWeather","input":{}}"#;
        let sse: SSEvent = serde_json::from_str(raw).expect("should decode without tool_kind");
        match sse {
            SSEvent::ToolInputAvailable { tool_kind, .. } => assert!(tool_kind.is_empty()),
            _ => panic!("should have been tool_input_available"),
        }
    }
    #[test]
    fn serialise_tool_output_available() {
        let sse = SSEvent::ToolOutputAvailable {
            tool_call_id: "call_fJdQDqnXeGxTmr4E3YPSR7Ar".to_string(),
            output: serde_json::json!({"city": "San Francisco","weather":"sunny"}),
        };
        assert_eq!(
            r#"{"type":"tool_output_available","tool_call_id":"call_fJdQDqnXeGxTmr4E3YPSR7Ar","output":{"city":"San Francisco","weather":"sunny"}}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    /// The TypeScript SDK mirrors these shapes by hand, so the wire format is
    /// part of the contract rather than an implementation detail.
    #[test]
    fn serialise_reference() {
        let sse = SSEvent::Reference {
            title: "GovS 011: Communication".to_string(),
            url: "gs://sample-rag-data/GovS_011.pdf".to_string(),
            snippet: Some("Teams cannot <b>communicate</b>&nbsp;...".to_string()),
            extractive_answer: Some(vec!["Communication resources".to_string()]),
        };
        assert_eq!(
            r#"{"type":"reference","title":"GovS 011: Communication","url":"gs://sample-rag-data/GovS_011.pdf","snippet":"Teams cannot <b>communicate</b>&nbsp;...","extractive_answer":["Communication resources"]}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    /// A reference without either is still sent, with both fields null — the
    /// SDK types them as nullable rather than optional for this reason.
    #[test]
    fn serialise_reference_without_snippet_or_answer() {
        let sse = SSEvent::Reference {
            title: "A title".to_string(),
            url: "https://example.com".to_string(),
            snippet: None,
            extractive_answer: None,
        };
        assert_eq!(
            r#"{"type":"reference","title":"A title","url":"https://example.com","snippet":null,"extractive_answer":null}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_grounding_score() {
        let sse = SSEvent::GroundingScore { score: 0.87 };
        assert_eq!(
            r#"{"type":"grounding_score","score":0.87}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_finish() {
        let sse = SSEvent::Finish;
        assert_eq!(
            r#"{"type":"finish"}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    #[test]
    fn serialise_usage() {
        let sse = SSEvent::Usage {
            node_id: "triage".to_string(),
            model: "gemini-2.5-flash-002".to_string(),
            provider: "vertex-ai".to_string(),
            server_address: "aiplatform.googleapis.com".to_string(),
            usage: TokenUsage {
                input_tokens: 1200,
                cached_input_tokens: 1024,
                output_tokens: 340,
                reasoning_tokens: 256,
                input_audio_tokens: 0,
                output_audio_tokens: 0,
            },
        };
        assert_eq!(
            r#"{"type":"usage","node_id":"triage","model":"gemini-2.5-flash-002","provider":"vertex-ai","server_address":"aiplatform.googleapis.com","usage":{"input_tokens":1200,"cached_input_tokens":1024,"output_tokens":340,"reasoning_tokens":256,"input_audio_tokens":0,"output_audio_tokens":0}}"#,
            serde_json::to_string(&sse).expect("Failed to serialize SSEvent")
        );
    }

    /// The conversion has to preserve the subset invariant documented on
    /// [`TokenUsage`]: cached input and reasoning tokens are already counted in
    /// their parents, so a bill that adds them again over-charges.
    #[test]
    fn token_usage_from_openai_usage() {
        let usage = OpenAIUsage {
            prompt_tokens: 1200,
            completion_tokens: 340,
            total_tokens: 1540,
            completion_tokens_details: Some(OpenAICompletionTokenDetails {
                reasoning_tokens: 256,
                audio_tokens: 12,
                ..Default::default()
            }),
            prompt_tokens_details: Some(OpenAIPromptTokenDetails {
                cached_tokens: 1024,
                audio_tokens: 8,
            }),
        };

        let billed: TokenUsage = (&usage).into();
        assert_eq!(billed.input_tokens, 1200);
        assert_eq!(billed.cached_input_tokens, 1024);
        assert_eq!(billed.output_tokens, 340);
        assert_eq!(billed.reasoning_tokens, 256);
        assert_eq!(billed.input_audio_tokens, 8);
        assert_eq!(billed.output_audio_tokens, 12);
        assert!(billed.cached_input_tokens <= billed.input_tokens);
        assert!(billed.reasoning_tokens <= billed.output_tokens);
    }

    #[test]
    fn usage_from_two_hosts_is_two_rows() {
        // Both speak `openai`, but OpenAI and OpenRouter bill differently.
        let event = |host: &str| SSEvent::Usage {
            node_id: "n".to_string(),
            model: "gpt-5".to_string(),
            provider: "openai".to_string(),
            server_address: host.to_string(),
            usage: TokenUsage {
                input_tokens: 10,
                ..Default::default()
            },
        };
        let mut tally = UsageTally::new();
        tally.add(&event("api.openai.com"));
        tally.add(&event("openrouter.ai"));
        tally.add(&event("api.openai.com"));
        let rows: Vec<(&str, usize)> = tally
            .tokens
            .iter()
            .map(|e| (e.server_address.as_str(), e.calls))
            .collect();
        assert_eq!(rows, [("api.openai.com", 2), ("openrouter.ai", 1)]);
    }

    fn usage_event(provider: &str, model: &str, input: usize, output: usize) -> SSEvent {
        SSEvent::Usage {
            node_id: "n".to_string(),
            model: model.to_string(),
            provider: provider.to_string(),
            server_address: String::new(),
            usage: TokenUsage {
                input_tokens: input,
                output_tokens: output,
                ..Default::default()
            },
        }
    }

    /// A run calls several models — a node's triage and summarisation clients
    /// differ, and inner nodes have their own — so the tally has to group rather
    /// than sum into one pair of counters. Provider is part of the key because
    /// the same model name is priced differently per backend.
    #[test]
    fn tally_groups_by_provider_and_model() {
        let mut tally = UsageTally::new();
        assert!(tally.is_empty());

        tally.add(&usage_event("vertex-ai", "gemini-2.5-flash", 100, 40));
        tally.add(&usage_event("vertex-ai", "gemini-2.5-flash", 10, 5));
        tally.add(&usage_event("openai-v1", "gemini-2.5-flash", 7, 3));
        // Non-usage events must be ignored, so callers can pass everything.
        tally.add(&SSEvent::Finish);

        let entries = &tally.tokens;
        assert_eq!(entries.len(), 2, "same model, two backends, two rows");

        assert_eq!(entries[0].provider, "vertex-ai", "insertion order is kept");
        assert_eq!(entries[0].calls, 2);
        assert_eq!(entries[0].usage.input_tokens, 110);
        assert_eq!(entries[0].usage.output_tokens, 45);

        assert_eq!(entries[1].provider, "openai-v1");
        assert_eq!(entries[1].calls, 1);
        assert_eq!(entries[1].usage.input_tokens, 7);
    }

    /// Tool usage is summed per (kind, unit), and does not count as a call:
    /// calls come from `ToolInputAvailable` alone.
    #[test]
    fn tally_sums_tool_usage() {
        let mut tally = UsageTally::new();
        let seconds = |n| SSEvent::ToolUsage {
            tool_kind: "code-execution".to_string(),
            unit: "second".to_string(),
            quantity: n,
        };
        tally.add(&seconds(12));
        tally.add(&seconds(30));
        assert!(!tally.is_empty());
        assert!(tally.tools.is_empty());
        assert_eq!(
            tally.tool_usage,
            vec![ToolUsage {
                tool_kind: "code-execution".to_string(),
                unit: "second".to_string(),
                quantity: 42,
            }]
        );
        let json = serde_json::to_value(&tally).unwrap();
        assert_eq!(json["tool_usage"][0]["quantity"], 42);
    }

    /// One webhook delivery can carry several messages, each its own run. The
    /// merged report must still have one row per (provider, model).
    #[test]
    fn tally_merges_separate_runs() {
        let mut first = UsageTally::new();
        first.add(&usage_event("groq", "llama-3.3-70b", 50, 20));

        let mut second = UsageTally::new();
        second.add(&usage_event("groq", "llama-3.3-70b", 30, 10));
        second.add(&usage_event("groq", "llama-3.1-8b", 5, 2));

        first.merge(&second.tokens);

        let entries = &first.tokens;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].model, "llama-3.3-70b");
        assert_eq!(entries[0].calls, 2);
        assert_eq!(entries[0].usage.input_tokens, 80);
        assert_eq!(entries[0].usage.output_tokens, 30);
        assert_eq!(entries[1].model, "llama-3.1-8b");
        assert_eq!(entries[1].calls, 1);
    }

    /// `ModelUsage` flattens `TokenUsage`, so the header payload stays one flat
    /// object per model rather than nesting a `usage` key.
    #[test]
    fn model_usage_flattens_tokens() {
        let mut tally = UsageTally::new();
        tally.add(&usage_event("vertex-ai", "gemini-2.5-flash", 100, 40));
        let json = serde_json::to_string(&tally.tokens).expect("should serialise");
        assert_eq!(
            json,
            r#"[{"provider":"vertex-ai","server_address":"","model":"gemini-2.5-flash","calls":1,"input_tokens":100,"cached_input_tokens":0,"output_tokens":40,"reasoning_tokens":0,"input_audio_tokens":0,"output_audio_tokens":0}]"#
        );
    }

    /// Providers that report no detail breakdown must yield zeroed subsets
    /// rather than a panic — the totals are still billable.
    #[test]
    fn token_usage_without_details() {
        let usage = OpenAIUsage {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
            completion_tokens_details: None,
            prompt_tokens_details: None,
        };

        let billed: TokenUsage = (&usage).into();
        assert_eq!(billed.input_tokens, 10);
        assert_eq!(billed.output_tokens, 20);
        assert_eq!(billed.cached_input_tokens, 0);
        assert_eq!(billed.reasoning_tokens, 0);
    }
}

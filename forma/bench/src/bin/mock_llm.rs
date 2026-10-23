//! A stand-in for an LLM provider, speaking OpenAI's chat completions API
//! (`POST /v1/chat/completions`, streamed or not), so Forma can be measured
//! without paying for tokens or waiting on a real model.
//!
//! It answers with a tool call when the request offers tools and the last
//! message is the user's, and with text otherwise, so every turn of an agent
//! with tools runs the whole loop: triage, tool, triage. How fast it answers
//! is set on the command line and can be changed while it runs:
//!
//! ```text
//! curl -X POST localhost:11500/config -d '{"ttft_ms": 400, "chunk_interval_ms": 20}'
//! ```
//!
//! Like a real provider, it refuses a conversation in which a tool result does
//! not follow the call it answers, or a call goes unanswered: OpenAI answers
//! those with a 400, and Gemini with an error of its own.
//!
//! `GET /stats` and `GET /timeline` report what it served; `POST /reset`
//! clears both.

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::Parser;
use forma_bench::{LlmCall, MockConfig, MockConfigPatch, MockStats, now_us};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// The words answers are made of.
const WORDS: &[&str] = &[
    "Your",
    "order",
    "left",
    "our",
    "warehouse",
    "yesterday",
    "and",
    "should",
    "arrive",
    "within",
    "two",
    "business",
    "days.",
    "You",
    "can",
    "follow",
    "it",
    "with",
    "the",
    "tracking",
    "number",
    "we",
    "sent",
    "by",
    "email.",
    "Anything",
    "else",
    "I",
    "can",
    "help",
    "you",
    "with?",
];

/// How many calls `/timeline` keeps; older ones are dropped.
const MAX_TIMELINE: usize = 1_000_000;

#[derive(Parser, Debug)]
#[command(about = "An OpenAI-compatible mock LLM, for benchmarking Forma")]
struct Args {
    /// Port to listen on (127.0.0.1 only).
    #[arg(long, env = "MOCK_LLM_PORT", default_value_t = 11500)]
    port: u16,
    /// Milliseconds before the first chunk (time to first token).
    #[arg(long, default_value_t = 0)]
    ttft_ms: u64,
    /// Text chunks per answer; each one is a word.
    #[arg(long, default_value_t = 32)]
    chunks: usize,
    /// Milliseconds between chunks.
    #[arg(long, default_value_t = 0)]
    chunk_interval_ms: u64,
    /// Never call tools: every answer is text.
    #[arg(long)]
    no_tool_calls: bool,
    /// Accept any conversation, even one a real provider would refuse.
    #[arg(long)]
    lenient: bool,
}

#[derive(Default)]
struct Mock {
    config: RwLock<MockConfig>,
    requests: AtomicU64,
    rejected: AtomicU64,
    streamed: AtomicU64,
    tool_calls: AtomicU64,
    bytes_in: AtomicU64,
    inflight: AtomicU64,
    max_inflight: AtomicU64,
    timeline: Mutex<Vec<LlmCall>>,
}

impl Mock {
    fn reset(&self) {
        for counter in [
            &self.requests,
            &self.rejected,
            &self.streamed,
            &self.tool_calls,
            &self.bytes_in,
            &self.max_inflight,
        ] {
            counter.store(0, Ordering::Relaxed);
        }
        self.timeline.lock().expect("poisoned").clear();
    }
}

/// Records a call when its answer has been written — or abandoned, when
/// Forma hangs up first.
struct CallRecord {
    mock: Arc<Mock>,
    call: LlmCall,
}

impl CallRecord {
    fn start(mock: &Arc<Mock>, bytes_in: usize, tool_call: bool) -> Self {
        let inflight = mock.inflight.fetch_add(1, Ordering::Relaxed) + 1;
        mock.max_inflight.fetch_max(inflight, Ordering::Relaxed);
        Self {
            mock: mock.clone(),
            call: LlmCall {
                start_us: now_us(),
                end_us: 0,
                bytes_in,
                tool_call,
            },
        }
    }
}

impl Drop for CallRecord {
    fn drop(&mut self) {
        self.mock.inflight.fetch_sub(1, Ordering::Relaxed);
        self.call.end_us = now_us();
        let mut timeline = self.mock.timeline.lock().expect("poisoned");
        if timeline.len() < MAX_TIMELINE {
            timeline.push(self.call);
        }
    }
}

/// What the mock decided to answer.
enum Answer {
    Text(Vec<String>),
    ToolCall { name: String, arguments: String },
}

/// A value for every property of a tool's parameters, of the right type, so
/// that the tool runs rather than failing on its input.
fn arguments_for(tool: &Value) -> String {
    let mut args = serde_json::Map::new();
    if let Some(properties) = tool["function"]["parameters"]["properties"].as_object() {
        for (name, schema) in properties {
            let value = match schema["type"].as_str() {
                Some("integer") | Some("number") => json!(1042),
                Some("boolean") => json!(true),
                Some("array") => json!([]),
                Some("object") => json!({}),
                _ => json!("B-1042"),
            };
            args.insert(name.clone(), value);
        }
    }
    Value::Object(args).to_string()
}

/// What OpenAI checks about tool calls before answering: each tool result
/// follows the assistant message that made the call, and every call is
/// answered before anything else is said.
fn check_conversation(messages: &[Value]) -> Result<(), String> {
    let mut unanswered: Vec<&str> = vec![];
    for (i, message) in messages.iter().enumerate() {
        if message["role"] == "tool" {
            let id = message["tool_call_id"].as_str().unwrap_or_default();
            match unanswered.iter().position(|call| *call == id) {
                Some(at) => {
                    unanswered.remove(at);
                }
                None => {
                    return Err(format!(
                        "messages[{i}]: a tool result for '{id}', which no assistant message right before it called"
                    ));
                }
            }
            continue;
        }
        if !unanswered.is_empty() {
            return Err(format!(
                "messages[{i}]: the tool calls {unanswered:?} have no result before it"
            ));
        }
        if message["role"] == "assistant" {
            unanswered = message["tool_calls"]
                .as_array()
                .map(|calls| calls.iter().filter_map(|c| c["id"].as_str()).collect())
                .unwrap_or_default();
        }
    }
    match unanswered.is_empty() {
        true => Ok(()),
        false => Err(format!("the tool calls {unanswered:?} have no result")),
    }
}

fn decide(request: &Value, config: &MockConfig) -> Answer {
    let first_tool = request["tools"].as_array().and_then(|t| t.first());
    let last_role = request["messages"]
        .as_array()
        .and_then(|m| m.last())
        .and_then(|m| m["role"].as_str());
    if let (true, Some(tool), Some("user")) = (config.tool_calls, first_tool, last_role) {
        return Answer::ToolCall {
            name: tool["function"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            arguments: arguments_for(tool),
        };
    }
    let words = (0..config.chunks)
        .map(|i| format!("{} ", WORDS[i % WORDS.len()]))
        .collect();
    Answer::Text(words)
}

/// What the answer cost, in the shape OpenAI reports it. Input tokens are
/// approximated from the size of the request.
fn usage(bytes_in: usize, completion_tokens: usize) -> Value {
    let prompt_tokens = bytes_in / 4;
    json!({
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "total_tokens": prompt_tokens + completion_tokens,
    })
}

fn chunk(id: &str, model: &str, delta: Value, finish_reason: Option<&str>) -> Bytes {
    let chunk = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": now_us() / 1_000_000,
        "model": model,
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish_reason }],
    });
    Bytes::from(format!("data: {chunk}\n\n"))
}

async fn chat(State(mock): State<Arc<Mock>>, body: Bytes) -> Response {
    let request: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    let config = mock.config.read().expect("poisoned").clone();
    let messages = request["messages"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if let (true, Err(e)) = (config.strict, check_conversation(messages)) {
        mock.rejected.fetch_add(1, Ordering::Relaxed);
        let error = json!({ "error": { "message": e, "type": "invalid_request_error" } });
        return (StatusCode::BAD_REQUEST, Json(error)).into_response();
    }
    let answer = decide(&request, &config);
    let is_tool_call = matches!(answer, Answer::ToolCall { .. });

    let n = mock.requests.fetch_add(1, Ordering::Relaxed);
    mock.bytes_in
        .fetch_add(body.len() as u64, Ordering::Relaxed);
    if is_tool_call {
        mock.tool_calls.fetch_add(1, Ordering::Relaxed);
    }
    let record = CallRecord::start(&mock, body.len(), is_tool_call);

    let id = format!("chatcmpl-{n}");
    let model = request["model"].as_str().unwrap_or("mock").to_string();
    let ttft = Duration::from_millis(config.ttft_ms);
    let interval = Duration::from_millis(config.chunk_interval_ms);

    if request["stream"].as_bool() != Some(true) {
        let (message, finish_reason, completion_tokens) = match answer {
            Answer::Text(words) => {
                tokio::time::sleep(ttft + interval * words.len().saturating_sub(1) as u32).await;
                let n = words.len();
                (
                    json!({ "role": "assistant", "content": words.concat() }),
                    "stop",
                    n,
                )
            }
            Answer::ToolCall { name, arguments } => {
                tokio::time::sleep(ttft).await;
                let call = json!({
                    "id": format!("call_{n}"),
                    "type": "function",
                    "function": { "name": name, "arguments": arguments },
                });
                (
                    json!({ "role": "assistant", "content": null, "tool_calls": [call] }),
                    "tool_calls",
                    1,
                )
            }
        };
        let response = json!({
            "id": id,
            "object": "chat.completion",
            "created": now_us() / 1_000_000,
            "model": model,
            "choices": [{ "index": 0, "message": message, "finish_reason": finish_reason }],
            "usage": usage(record.call.bytes_in, completion_tokens),
        });
        drop(record);
        return Json(response).into_response();
    }

    mock.streamed.fetch_add(1, Ordering::Relaxed);
    let include_usage = request["stream_options"]["include_usage"].as_bool() == Some(true);
    let stream = async_stream::stream! {
        // Dropped with the stream: when the last chunk has been handed over,
        // or when Forma hangs up.
        let record = record;
        tokio::time::sleep(ttft).await;
        let completion_tokens = match answer {
            Answer::Text(words) => {
                let n = words.len();
                for (i, word) in words.into_iter().enumerate() {
                    if i > 0 && !interval.is_zero() {
                        tokio::time::sleep(interval).await;
                    }
                    let delta = if i == 0 {
                        json!({ "role": "assistant", "content": word })
                    } else {
                        json!({ "content": word })
                    };
                    yield Ok::<_, std::convert::Infallible>(chunk(&id, &model, delta, None));
                }
                yield Ok(chunk(&id, &model, json!({}), Some("stop")));
                n
            }
            Answer::ToolCall { name, arguments } => {
                let delta = json!({
                    "role": "assistant",
                    "tool_calls": [{
                        "index": 0,
                        "id": format!("call_{n}"),
                        "type": "function",
                        "function": { "name": name, "arguments": arguments },
                    }],
                });
                yield Ok(chunk(&id, &model, delta, None));
                yield Ok(chunk(&id, &model, json!({}), Some("tool_calls")));
                1
            }
        };
        if include_usage {
            let last = json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": now_us() / 1_000_000,
                "model": model,
                "choices": [],
                "usage": usage(record.call.bytes_in, completion_tokens),
            });
            yield Ok(Bytes::from(format!("data: {last}\n\n")));
        }
        yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .expect("valid response")
}

async fn stats(State(mock): State<Arc<Mock>>) -> Json<MockStats> {
    Json(MockStats {
        requests: mock.requests.load(Ordering::Relaxed),
        rejected: mock.rejected.load(Ordering::Relaxed),
        streamed: mock.streamed.load(Ordering::Relaxed),
        tool_calls: mock.tool_calls.load(Ordering::Relaxed),
        bytes_in: mock.bytes_in.load(Ordering::Relaxed),
        max_inflight: mock.max_inflight.load(Ordering::Relaxed),
    })
}

async fn timeline(State(mock): State<Arc<Mock>>) -> Json<Vec<LlmCall>> {
    Json(mock.timeline.lock().expect("poisoned").clone())
}

async fn reset(State(mock): State<Arc<Mock>>) -> StatusCode {
    mock.reset();
    StatusCode::NO_CONTENT
}

async fn get_config(State(mock): State<Arc<Mock>>) -> Json<MockConfig> {
    Json(mock.config.read().expect("poisoned").clone())
}

async fn set_config(
    State(mock): State<Arc<Mock>>,
    Json(patch): Json<MockConfigPatch>,
) -> Json<MockConfig> {
    let mut config = mock.config.write().expect("poisoned");
    config.apply(patch);
    Json(config.clone())
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let mock = Arc::new(Mock {
        config: RwLock::new(MockConfig {
            ttft_ms: args.ttft_ms,
            chunks: args.chunks,
            chunk_interval_ms: args.chunk_interval_ms,
            tool_calls: !args.no_tool_calls,
            strict: !args.lenient,
        }),
        ..Default::default()
    });

    let app = Router::new()
        .route("/v1/chat/completions", post(chat))
        .route("/stats", get(stats))
        .route("/timeline", get(timeline))
        .route("/reset", post(reset))
        .route("/config", get(get_config).post(set_config))
        .route("/health", get(|| async { "ok" }))
        .with_state(mock);

    let addr = format!("127.0.0.1:{}", args.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("could not listen on {addr}: {e}"));
    eprintln!("mock LLM listening on http://{addr}");
    axum::serve(listener, app).await.expect("server failed");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(last_role: &str, with_tools: bool) -> Value {
        let tools = if with_tools {
            json!([{
                "type": "function",
                "function": {
                    "name": "lookup_order",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "order_id": { "type": "string" },
                            "count": { "type": "integer" },
                        },
                    },
                },
            }])
        } else {
            Value::Null
        };
        json!({
            "model": "mock",
            "messages": [{ "role": last_role, "content": "hi" }],
            "tools": tools,
        })
    }

    #[test]
    fn a_user_turn_with_tools_gets_a_tool_call() {
        let config = MockConfig::default();
        match decide(&request("user", true), &config) {
            Answer::ToolCall { name, arguments } => {
                assert_eq!(name, "lookup_order");
                let args: Value = serde_json::from_str(&arguments).unwrap();
                assert_eq!(args["order_id"], "B-1042");
                assert_eq!(args["count"], 1042);
            }
            Answer::Text(_) => panic!("expected a tool call"),
        }
    }

    #[test]
    fn a_tool_result_gets_text() {
        let config = MockConfig::default();
        match decide(&request("tool", true), &config) {
            Answer::Text(words) => assert_eq!(words.len(), config.chunks),
            Answer::ToolCall { .. } => panic!("expected text"),
        }
    }

    #[test]
    fn no_tools_means_text() {
        let config = MockConfig::default();
        assert!(matches!(
            decide(&request("user", false), &config),
            Answer::Text(_)
        ));
        let config = MockConfig {
            tool_calls: false,
            ..Default::default()
        };
        assert!(matches!(
            decide(&request("user", true), &config),
            Answer::Text(_)
        ));
    }

    #[test]
    fn a_tool_result_must_follow_its_call() {
        let call = json!({
            "role": "assistant",
            "tool_calls": [{ "id": "call_1", "type": "function",
                "function": { "name": "lookup_order", "arguments": "{}" } }],
        });
        let result = json!({ "role": "tool", "tool_call_id": "call_1", "content": "shipped" });
        let user = json!({ "role": "user", "content": "hi" });

        let fine = [user.clone(), call.clone(), result.clone(), user.clone()];
        assert!(check_conversation(&fine).is_ok());

        // A window that starts after the call: the result is an orphan.
        let orphan = [result.clone(), user.clone()];
        assert!(check_conversation(&orphan).is_err());

        // Something said between the call and its result.
        let interrupted = [call.clone(), user.clone(), result.clone()];
        assert!(check_conversation(&interrupted).is_err());

        // A call that never got its result.
        let unanswered = [user, call];
        assert!(check_conversation(&unanswered).is_err());
    }

    #[test]
    fn chunks_are_server_sent_events() {
        let c = chunk("id", "mock", json!({ "content": "hi " }), None);
        let s = std::str::from_utf8(&c).unwrap();
        assert!(s.starts_with("data: {"));
        assert!(s.ends_with("}\n\n"));
        let v: Value = serde_json::from_str(s.trim().strip_prefix("data: ").unwrap()).unwrap();
        assert_eq!(v["choices"][0]["delta"]["content"], "hi ");
    }
}

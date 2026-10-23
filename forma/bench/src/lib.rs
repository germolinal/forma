//! Shared pieces of the benchmark tools: what the mock LLM reports about the
//! calls it served, and the arithmetic both sides use to summarise timings.
//!
//! Every timestamp is wall-clock microseconds since the Unix epoch. The mock
//! and the load generator run on the same machine, so their clocks agree, and
//! that is what lets a request seen by the load generator be lined up with the
//! LLM calls Forma made for it.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Microseconds since the Unix epoch.
pub fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or_default()
}

/// How the mock answers. Changed at runtime through `POST /config`, so one
/// mock serves every scenario of a run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MockConfig {
    /// Milliseconds before the first chunk: the provider's time to first token.
    pub ttft_ms: u64,
    /// Text chunks in an answer; each one is a word.
    pub chunks: usize,
    /// Milliseconds between two chunks.
    pub chunk_interval_ms: u64,
    /// Whether to call a tool when the request offers some and the last
    /// message is the user's. Off, every answer is text.
    pub tool_calls: bool,
    /// Whether to refuse, as a real provider does, a conversation in which a
    /// tool result is not right after its call. Off, anything goes.
    pub strict: bool,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            ttft_ms: 0,
            chunks: 32,
            chunk_interval_ms: 0,
            tool_calls: true,
            strict: true,
        }
    }
}

/// A partial [`MockConfig`]: what `POST /config` changes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MockConfigPatch {
    pub ttft_ms: Option<u64>,
    pub chunks: Option<usize>,
    pub chunk_interval_ms: Option<u64>,
    pub tool_calls: Option<bool>,
    pub strict: Option<bool>,
}

impl MockConfig {
    pub fn apply(&mut self, patch: MockConfigPatch) {
        if let Some(v) = patch.ttft_ms {
            self.ttft_ms = v;
        }
        if let Some(v) = patch.chunks {
            self.chunks = v;
        }
        if let Some(v) = patch.chunk_interval_ms {
            self.chunk_interval_ms = v;
        }
        if let Some(v) = patch.tool_calls {
            self.tool_calls = v;
        }
        if let Some(v) = patch.strict {
            self.strict = v;
        }
    }
}

/// One LLM call as the mock saw it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LlmCall {
    /// When the request reached the mock.
    pub start_us: u64,
    /// When the mock wrote the last byte of its answer.
    pub end_us: u64,
    /// Size of the request body: the conversation Forma sent.
    pub bytes_in: usize,
    /// Whether the answer was a tool call (rather than text).
    pub tool_call: bool,
}

/// Counters the mock keeps, as `GET /stats` reports them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MockStats {
    pub requests: u64,
    /// Requests refused because their conversation was malformed (a tool
    /// result without its call, or a call without its result).
    pub rejected: u64,
    pub streamed: u64,
    pub tool_calls: u64,
    pub bytes_in: u64,
    pub max_inflight: u64,
}

/// Distribution of a set of samples, in milliseconds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Distribution {
    pub n: usize,
    pub mean: f64,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub max: f64,
}

impl Distribution {
    /// Summarises samples given in microseconds.
    pub fn from_us(samples: &[u64]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let at = |q: f64| {
            let i = ((sorted.len() as f64 - 1.0) * q).round() as usize;
            sorted[i] as f64 / 1000.0
        };
        let sum: u64 = sorted.iter().sum();
        Self {
            n: sorted.len(),
            mean: sum as f64 / sorted.len() as f64 / 1000.0,
            p50: at(0.50),
            p90: at(0.90),
            p99: at(0.99),
            max: *sorted.last().expect("not empty") as f64 / 1000.0,
        }
    }
}

/// One request as the load generator saw it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RequestTiming {
    pub start_us: u64,
    pub end_us: u64,
}

/// Where the time of a request went, from lining it up with the LLM calls
/// Forma made for it. Only meaningful when requests run one at a time:
/// otherwise calls cannot be told apart.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Phases {
    /// Requests that could be lined up with their LLM calls.
    pub matched: usize,
    /// LLM calls per request (median).
    pub llm_calls: f64,
    /// From the request leaving the client to Forma's first LLM call: reading
    /// the session and the agent, claiming the session, storing the message,
    /// building the LLM request.
    pub before_first_llm_call: Distribution,
    /// Time between the end of one LLM call and the start of the next: running
    /// the tools and building the next request.
    pub between_llm_calls: Distribution,
    /// From the last LLM call ending to the client receiving the last byte:
    /// storing the answer and closing the response.
    pub after_last_llm_call: Distribution,
    /// Time spent inside the mock, waiting for "the LLM".
    pub in_llm: Distribution,
    /// Everything that was not the LLM: Forma's own cost per request.
    pub forma_overhead: Distribution,
}

impl Phases {
    /// Lines each request up with the calls that started and ended within it.
    pub fn compute(requests: &[RequestTiming], calls: &[LlmCall]) -> Self {
        let mut calls = calls.to_vec();
        calls.sort_by_key(|c| c.start_us);
        let mut before = vec![];
        let mut between = vec![];
        let mut after = vec![];
        let mut in_llm = vec![];
        let mut overhead = vec![];
        let mut n_calls = vec![];
        for r in requests {
            let mine: Vec<&LlmCall> = calls
                .iter()
                .filter(|c| c.start_us >= r.start_us && c.end_us <= r.end_us)
                .collect();
            let (Some(first), Some(last)) = (mine.first(), mine.last()) else {
                continue;
            };
            before.push(first.start_us - r.start_us);
            after.push(r.end_us.saturating_sub(last.end_us));
            let gaps: u64 = mine
                .windows(2)
                .map(|w| w[1].start_us.saturating_sub(w[0].end_us))
                .sum();
            between.push(gaps);
            let llm: u64 = mine.iter().map(|c| c.end_us - c.start_us).sum();
            in_llm.push(llm);
            overhead.push((r.end_us - r.start_us).saturating_sub(llm));
            n_calls.push(mine.len() as u64);
        }
        n_calls.sort_unstable();
        Self {
            matched: before.len(),
            llm_calls: n_calls.get(n_calls.len() / 2).copied().unwrap_or(0) as f64,
            before_first_llm_call: Distribution::from_us(&before),
            between_llm_calls: Distribution::from_us(&between),
            after_last_llm_call: Distribution::from_us(&after),
            in_llm: Distribution::from_us(&in_llm),
            forma_overhead: Distribution::from_us(&overhead),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_of_nothing_is_empty() {
        let d = Distribution::from_us(&[]);
        assert_eq!(d.n, 0);
        assert_eq!(d.max, 0.0);
    }

    #[test]
    fn distribution_reads_milliseconds() {
        let samples: Vec<u64> = (1..=100).map(|ms| ms * 1000).collect();
        let d = Distribution::from_us(&samples);
        assert_eq!(d.n, 100);
        assert_eq!(d.p50, 51.0);
        assert_eq!(d.p99, 99.0);
        assert_eq!(d.max, 100.0);
        assert_eq!(d.mean, 50.5);
    }

    #[test]
    fn phases_split_a_request_around_its_calls() {
        let requests = [RequestTiming {
            start_us: 1_000,
            end_us: 10_000,
        }];
        let call = |start_us, end_us| LlmCall {
            start_us,
            end_us,
            bytes_in: 0,
            tool_call: false,
        };
        // Two calls, plus one belonging to another request.
        let calls = [call(2_000, 4_000), call(5_000, 8_000), call(20_000, 21_000)];
        let p = Phases::compute(&requests, &calls);
        assert_eq!(p.matched, 1);
        assert_eq!(p.llm_calls, 2.0);
        assert_eq!(p.before_first_llm_call.p50, 1.0);
        assert_eq!(p.between_llm_calls.p50, 1.0);
        assert_eq!(p.after_last_llm_call.p50, 2.0);
        assert_eq!(p.in_llm.p50, 5.0);
        assert_eq!(p.forma_overhead.p50, 4.0);
    }

    #[test]
    fn a_patch_changes_only_what_it_names() {
        let mut c = MockConfig::default();
        c.apply(MockConfigPatch {
            ttft_ms: Some(300),
            ..Default::default()
        });
        assert_eq!(c.ttft_ms, 300);
        assert_eq!(c.chunks, MockConfig::default().chunks);
    }
}

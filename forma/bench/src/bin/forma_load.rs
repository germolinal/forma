//! Sends chat messages to a running Forma server and reports how long they
//! took, how much CPU and memory Forma used doing it and — when requests run
//! one at a time and a mock LLM is given — where the time went.
//!
//! Each virtual user is one session: its messages are sent one after the
//! other, as a person would, so concurrency is the number of conversations
//! happening at once.

use clap::{Parser, Subcommand, ValueEnum};
use forma_bench::{Distribution, LlmCall, MockStats, Phases, RequestTiming, now_us};
use futures::StreamExt;
use mongodb::bson::{doc, oid::ObjectId};
use protocols::{
    AgentDeployment, CHAT_PATH, DEPLOYMENT_ID_HEADER, FORMA_AGENT_API_KEY, FORMA_DATABASE_ID,
    INIT_SESSION_PATH, SESSION_ID_HEADER, USER_ID_HEADER,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Parser, Debug)]
#[command(about = "Load generator for Forma's chat endpoint")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Sends messages and reports what they cost.
    Run(Box<RunArgs>),
    /// Stores a compiled agent as a deployment, for a multi-tenant server, and
    /// prints its id.
    Deploy(DeployArgs),
    /// Removes a deployment made by `deploy`.
    Undeploy(UndeployArgs),
    /// Turns the JSON files `run` wrote in a directory into Markdown tables.
    Summarise(SummariseArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    /// The `sse` runtime: the answer streams as server-sent events.
    Sse,
    /// The `sync` runtime: the answer comes back as one JSON.
    Sync,
}

#[derive(Parser, Debug)]
struct RunArgs {
    /// Forma's base URL.
    #[arg(long, default_value = "http://127.0.0.1:18080")]
    url: String,
    /// Which runtime the server runs.
    #[arg(long, value_enum, default_value_t = Mode::Sse)]
    mode: Mode,
    /// Conversations happening at once.
    #[arg(long, default_value_t = 1)]
    concurrency: usize,
    /// Messages to send, in total. Ignored when `--duration-s` is given.
    #[arg(long, default_value_t = 100)]
    requests: u64,
    /// Send messages for this many seconds instead of a fixed number.
    #[arg(long)]
    duration_s: Option<u64>,
    /// Messages sent first and left out of the results.
    #[arg(long, default_value_t = 10)]
    warmup: u64,
    /// Sent as `x-api-key`.
    #[arg(long, env = "FORMA_AGENT_KEY", default_value = "")]
    api_key: String,
    /// Sent as `x-deployment-id`, for a multi-tenant server.
    #[arg(long)]
    deployment_id: Option<String>,
    /// Open a session per user (`GET /v1/init`) and keep the conversation in
    /// it. Needed when the server stores sessions.
    #[arg(long)]
    persist: bool,
    /// The message every user sends.
    #[arg(long, default_value = "Hi! Where is my order B-1042?")]
    message: String,
    /// The mock LLM's base URL: its counters are reset before measuring and
    /// reported after, and with `--concurrency 1` each request is split into
    /// phases around its LLM calls.
    #[arg(long)]
    mock_url: Option<String>,
    /// Forma's process id, to report the CPU time, memory and threads it used.
    #[arg(long)]
    pid: Option<u32>,
    /// A name for this scenario, in the report.
    #[arg(long, default_value = "run")]
    label: String,
    /// Where to write the results as JSON.
    #[arg(long)]
    json_out: Option<String>,
    /// MongoDB to delete the sessions this run opened from, once it is done.
    #[arg(long)]
    cleanup_db: Option<String>,
    /// The MongoDB Forma uses, to count the operations each message costs.
    /// The counters are the server's, so it should serve nothing else.
    #[arg(long)]
    db_url: Option<String>,
}

#[derive(Parser, Debug)]
struct DeployArgs {
    /// The compiled agent (`forma expand -c --format json`).
    #[arg(long)]
    agent: String,
    /// The MongoDB holding deployments.
    #[arg(long, env = "SESSIONS_DB_URL")]
    mongo_url: String,
}

#[derive(Parser, Debug)]
struct UndeployArgs {
    /// The id `deploy` printed.
    #[arg(long)]
    id: String,
    /// The MongoDB holding deployments.
    #[arg(long, env = "SESSIONS_DB_URL")]
    mongo_url: String,
}

#[derive(Parser, Debug)]
struct SummariseArgs {
    /// The directory holding the results, one JSON per scenario.
    dir: String,
}

/// One virtual user: who it is and which session it talks in.
#[derive(Debug, Clone)]
struct User {
    user_id: String,
    session_id: Option<String>,
}

#[derive(Debug, Default)]
struct Outcome {
    /// When it started and ended, by the wall clock: what lines it up with
    /// the mock's own records.
    timing: Option<RequestTiming>,
    /// How long it took, by the monotonic clock, which no clock adjustment
    /// moves.
    latency_us: u64,
    /// Until the first byte of the answer.
    ttfb_us: Option<u64>,
    /// Until the first piece of text (streaming only).
    ttft_us: Option<u64>,
    error: Option<String>,
}

#[derive(Debug, Default)]
struct Collected {
    timings: Vec<RequestTiming>,
    latency_us: Vec<u64>,
    ttfb_us: Vec<u64>,
    ttft_us: Vec<u64>,
    errors: Vec<String>,
}

/// What the Forma process used during the measured part of the run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProcessUsage {
    /// CPU time (user + system), in milliseconds.
    cpu_ms: f64,
    /// CPU time per answered message, in milliseconds.
    cpu_ms_per_request: f64,
    /// Average cores busy: CPU time over wall time.
    cores_busy: f64,
    rss_mb_start: f64,
    rss_mb_peak: f64,
    threads_start: u64,
    threads_peak: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Report {
    label: String,
    mode: Option<Mode>,
    concurrency: usize,
    requests: usize,
    errors: usize,
    first_errors: Vec<String>,
    elapsed_s: f64,
    requests_per_s: f64,
    latency_ms: Distribution,
    ttfb_ms: Distribution,
    ttft_ms: Distribution,
    process: Option<ProcessUsage>,
    mock: Option<MockReport>,
    phases: Option<Phases>,
    db_ops_per_request: Option<DbOps>,
}

/// Commands MongoDB ran: each one a round trip from Forma. Counted from
/// `serverStatus.metrics.commands` rather than `opcounters`, which counts an
/// `insert_many` once per document it stores.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DbOps {
    insert: f64,
    find: f64,
    update: f64,
    delete: f64,
    other: f64,
}

/// The commands that read or write data. The rest — `hello`, `ping`,
/// `serverStatus` — come from drivers keeping an eye on the server.
const DATA_COMMANDS: [&str; 9] = [
    "insert",
    "find",
    "update",
    "delete",
    "getMore",
    "findAndModify",
    "aggregate",
    "count",
    "distinct",
];

impl DbOps {
    async fn read(client: &mongodb::Client) -> Result<Self, String> {
        let status = client
            .database("admin")
            .run_command(doc! { "serverStatus": 1 })
            .await
            .map_err(|e| format!("could not read the database's counters: {e}"))?;
        let commands = status
            .get_document("metrics")
            .and_then(|m| m.get_document("commands"))
            .map_err(|e| e.to_string())?;
        let read = |name: &str| {
            commands
                .get_document(name)
                .ok()
                .and_then(|c| c.get("total"))
                .and_then(|v| v.as_i64().or(v.as_i32().map(i64::from)))
                .unwrap_or_default() as f64
        };
        let other = DATA_COMMANDS[4..].iter().map(|name| read(name)).sum();
        Ok(Self {
            insert: read("insert"),
            find: read("find"),
            update: read("update"),
            delete: read("delete"),
            other,
        })
    }

    /// What happened between `before` and `self`, per request.
    fn per_request(&self, before: &Self, requests: usize) -> Self {
        let n = requests.max(1) as f64;
        Self {
            insert: (self.insert - before.insert) / n,
            find: (self.find - before.find) / n,
            update: (self.update - before.update) / n,
            delete: (self.delete - before.delete) / n,
            other: (self.other - before.other) / n,
        }
    }

    fn total(&self) -> f64 {
        self.insert + self.find + self.update + self.delete + self.other
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct MockReport {
    #[serde(flatten)]
    stats: MockStats,
    /// LLM calls per answered message.
    calls_per_request: f64,
    /// Average size of what Forma sent the LLM, in kB.
    request_kb_mean: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct ProcSample {
    cpu_ms: f64,
    rss_kb: u64,
    threads: u64,
}

/// Reads a process's CPU time, resident memory and thread count.
fn sample_process(pid: u32) -> Option<ProcSample> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The command name may hold spaces; fields start after its `)`.
        let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
        let ticks = 100.0; // CLK_TCK on every Linux this runs on.
        let utime: f64 = fields.get(11)?.parse().ok()?;
        let stime: f64 = fields.get(12)?.parse().ok()?;
        let threads: u64 = fields.get(17)?.parse().ok()?;
        let rss_pages: u64 = fields.get(21)?.parse().ok()?;
        Some(ProcSample {
            cpu_ms: (utime + stime) / ticks * 1000.0,
            rss_kb: rss_pages * 4,
            threads,
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "time=,rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut parts = text.split_whitespace();
        // `[[hh:]mm:]ss.cc`
        let cpu_ms = parts
            .next()?
            .rsplit(':')
            .zip([1.0, 60.0, 3600.0])
            .map(|(v, unit)| v.parse::<f64>().unwrap_or(0.0) * unit)
            .sum::<f64>()
            * 1000.0;
        let rss_kb = parts.next()?.parse().ok()?;
        let threads = std::process::Command::new("ps")
            .args(["-M", "-p", &pid.to_string()])
            .output()
            .ok()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .count()
                    .saturating_sub(1) as u64
            })
            .unwrap_or(0);
        Some(ProcSample {
            cpu_ms,
            rss_kb,
            threads,
        })
    }
}

/// Samples the process until told to stop, keeping the peaks.
fn watch_process(pid: u32, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<(u64, u64)> {
    std::thread::spawn(move || {
        let (mut rss, mut threads) = (0, 0);
        while !stop.load(Ordering::Relaxed) {
            if let Some(s) = sample_process(pid) {
                rss = rss.max(s.rss_kb);
                threads = threads.max(s.threads);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        (rss, threads)
    })
}

impl RunArgs {
    fn request(
        &self,
        client: &reqwest::Client,
        method: reqwest::Method,
        path: &str,
        user: &User,
    ) -> reqwest::RequestBuilder {
        let mut r = client
            .request(method, format!("{}{path}", self.url))
            .header(FORMA_AGENT_API_KEY, &self.api_key)
            .header(USER_ID_HEADER, &user.user_id);
        if let Some(id) = &self.deployment_id {
            r = r.header(DEPLOYMENT_ID_HEADER, id);
        }
        if let Some(id) = &user.session_id {
            r = r.header(SESSION_ID_HEADER, id);
        }
        r
    }

    async fn open_session(&self, client: &reqwest::Client, user: &mut User) -> Result<(), String> {
        let res = self
            .request(client, reqwest::Method::GET, INIT_SESSION_PATH, user)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = res.status();
        let body = res.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("{status} opening a session: {body}"));
        }
        let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        user.session_id = v["session_id"].as_str().map(str::to_string);
        Ok(())
    }

    async fn send(&self, client: &reqwest::Client, user: &User) -> Outcome {
        let start_us = now_us();
        let start = Instant::now();
        let mut outcome = Outcome::default();
        let body = json!({ "role": "user", "content": self.message });
        let res = match self
            .request(client, reqwest::Method::POST, CHAT_PATH, user)
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                outcome.error = Some(e.to_string());
                return outcome;
            }
        };
        let status = res.status();
        if !status.is_success() {
            let text = res.text().await.unwrap_or_default();
            outcome.error = Some(format!("{status}: {}", truncate(&text)));
            return outcome;
        }

        match self.mode {
            Mode::Sync => {
                let text = match res.text().await {
                    Ok(t) => t,
                    Err(e) => {
                        outcome.error = Some(e.to_string());
                        return outcome;
                    }
                };
                outcome.ttfb_us = Some(start.elapsed().as_micros() as u64);
                let v: Value = serde_json::from_str(&text).unwrap_or_default();
                if v["status"] != "completed" {
                    outcome.error = Some(format!("not completed: {}", truncate(&text)));
                }
            }
            Mode::Sse => {
                let mut stream = res.bytes_stream();
                let mut buffer = String::new();
                let mut finished = false;
                while let Some(chunk) = stream.next().await {
                    let chunk = match chunk {
                        Ok(c) => c,
                        Err(e) => {
                            outcome.error = Some(e.to_string());
                            return outcome;
                        }
                    };
                    let elapsed = start.elapsed().as_micros() as u64;
                    outcome.ttfb_us.get_or_insert(elapsed);
                    buffer.push_str(&String::from_utf8_lossy(&chunk));
                    while let Some(at) = buffer.find("\n\n") {
                        let event: String = buffer.drain(..at + 2).collect();
                        if event.contains(r#""type":"text_delta""#) {
                            outcome.ttft_us.get_or_insert(elapsed);
                        } else if event.contains(r#""type":"error""#) {
                            outcome.error = Some(truncate(event.trim()).to_string());
                        } else if event.contains(r#""type":"finish""#) {
                            finished = true;
                        }
                    }
                }
                if outcome.error.is_none() && !finished {
                    outcome.error = Some("the stream ended without a finish event".to_string());
                }
            }
        }
        outcome.latency_us = start.elapsed().as_micros() as u64;
        outcome.timing = Some(RequestTiming {
            start_us,
            end_us: now_us(),
        });
        outcome
    }

    async fn run(self) -> Result<(), String> {
        let args = Arc::new(self);
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(args.concurrency * 2)
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| e.to_string())?;

        // Users, and their sessions.
        let tag = now_us();
        let mut users = Vec::with_capacity(args.concurrency);
        for i in 0..args.concurrency {
            let mut user = User {
                user_id: format!("bench-{tag}-{i}"),
                session_id: None,
            };
            if args.persist {
                args.open_session(&client, &mut user).await?;
            }
            users.push(user);
        }

        // Warm up: connections, caches, lazily built things.
        let warmup_left = Arc::new(AtomicU64::new(args.warmup));
        let mut handles = vec![];
        for user in users.clone() {
            let (args, client, left) = (args.clone(), client.clone(), warmup_left.clone());
            handles.push(tokio::spawn(async move {
                while left
                    .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
                    .is_ok()
                {
                    if let Some(e) = args.send(&client, &user).await.error {
                        return Err(format!("warm-up failed: {e}"));
                    }
                }
                Ok(())
            }));
        }
        for h in handles {
            h.await.map_err(|e| e.to_string())??;
        }

        if let Some(mock) = &args.mock_url {
            client
                .post(format!("{mock}/reset"))
                .send()
                .await
                .map_err(|e| format!("could not reset the mock: {e}"))?;
        }

        // Measure.
        let db = match &args.db_url {
            Some(url) => Some(
                mongodb::Client::with_uri_str(url)
                    .await
                    .map_err(|e| e.to_string())?,
            ),
            None => None,
        };
        let db_before = match &db {
            Some(db) => Some(DbOps::read(db).await?),
            None => None,
        };
        let stop_watching = Arc::new(AtomicBool::new(false));
        let process_start = args.pid.and_then(sample_process);
        let watcher = args
            .pid
            .map(|pid| watch_process(pid, stop_watching.clone()));
        let started = Instant::now();
        let deadline = args.duration_s.map(|s| started + Duration::from_secs(s));
        let left = Arc::new(AtomicU64::new(args.requests));
        let collected = Arc::new(Mutex::new(Collected::default()));
        let mut handles = vec![];
        for user in users.clone() {
            let (args, client, left, collected) = (
                args.clone(),
                client.clone(),
                left.clone(),
                collected.clone(),
            );
            handles.push(tokio::spawn(async move {
                loop {
                    match deadline {
                        Some(d) if Instant::now() >= d => break,
                        Some(_) => {}
                        None => {
                            if left
                                .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                                    n.checked_sub(1)
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                    let outcome = args.send(&client, &user).await;
                    let mut c = collected.lock().expect("poisoned");
                    match (outcome.error, outcome.timing) {
                        (Some(e), _) => c.errors.push(e),
                        (None, Some(t)) => {
                            c.timings.push(t);
                            c.latency_us.push(outcome.latency_us);
                            c.ttfb_us.extend(outcome.ttfb_us);
                            c.ttft_us.extend(outcome.ttft_us);
                        }
                        (None, None) => c.errors.push("no timing recorded".to_string()),
                    }
                }
            }));
        }
        for h in handles {
            h.await.map_err(|e| e.to_string())?;
        }
        let elapsed = started.elapsed();
        stop_watching.store(true, Ordering::Relaxed);
        let process_end = args.pid.and_then(sample_process);
        let db_after = match &db {
            Some(db) => Some(DbOps::read(db).await?),
            None => None,
        };
        let peaks = watcher.and_then(|w| w.join().ok());

        let collected = std::mem::take(&mut *collected.lock().expect("poisoned"));
        let answered = collected.timings.len();

        let process = match (process_start, process_end) {
            (Some(start), Some(end)) => {
                let (rss_peak, threads_peak) = peaks.unwrap_or((end.rss_kb, end.threads));
                let cpu_ms = end.cpu_ms - start.cpu_ms;
                Some(ProcessUsage {
                    cpu_ms,
                    cpu_ms_per_request: cpu_ms / answered.max(1) as f64,
                    cores_busy: cpu_ms / 1000.0 / elapsed.as_secs_f64(),
                    rss_mb_start: start.rss_kb as f64 / 1024.0,
                    rss_mb_peak: rss_peak.max(end.rss_kb) as f64 / 1024.0,
                    threads_start: start.threads,
                    threads_peak: threads_peak.max(end.threads),
                })
            }
            _ => None,
        };

        let (mock, phases) = match &args.mock_url {
            Some(url) => {
                let stats: MockStats = fetch_json(&client, &format!("{url}/stats")).await?;
                let calls: Vec<LlmCall> = fetch_json(&client, &format!("{url}/timeline")).await?;
                let report = MockReport {
                    calls_per_request: stats.requests as f64 / answered.max(1) as f64,
                    request_kb_mean: stats.bytes_in as f64 / stats.requests.max(1) as f64 / 1024.0,
                    stats,
                };
                let phases =
                    (args.concurrency == 1).then(|| Phases::compute(&collected.timings, &calls));
                (Some(report), phases)
            }
            None => (None, None),
        };

        let report = Report {
            label: args.label.clone(),
            mode: Some(args.mode),
            concurrency: args.concurrency,
            requests: answered,
            errors: collected.errors.len(),
            first_errors: collected.errors.iter().take(5).cloned().collect(),
            elapsed_s: elapsed.as_secs_f64(),
            requests_per_s: answered as f64 / elapsed.as_secs_f64(),
            latency_ms: Distribution::from_us(&collected.latency_us),
            ttfb_ms: Distribution::from_us(&collected.ttfb_us),
            ttft_ms: Distribution::from_us(&collected.ttft_us),
            process,
            mock,
            phases,
            db_ops_per_request: db_before
                .zip(db_after)
                .map(|(before, after)| after.per_request(&before, answered)),
        };
        print_report(&report);
        if let Some(path) = &args.json_out {
            let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
            std::fs::write(path, json).map_err(|e| format!("could not write '{path}': {e}"))?;
        }

        if let Some(mongo_url) = &args.cleanup_db {
            let db = forma_db(mongo_url).await?;
            for session_id in users.iter().filter_map(|u| u.session_id.as_deref()) {
                let id = ObjectId::parse_str(session_id).map_err(|e| e.to_string())?;
                db.collection::<mongodb::bson::Document>("messages")
                    .delete_many(doc! { "session_id": session_id })
                    .await
                    .map_err(|e| e.to_string())?;
                db.collection::<mongodb::bson::Document>("sessions")
                    .delete_one(doc! { "_id": id })
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }

        if answered == 0 {
            return Err("no request succeeded".to_string());
        }
        Ok(())
    }
}

async fn fetch_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T, String> {
    client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("{url}: {e}"))?
        .json()
        .await
        .map_err(|e| format!("{url}: {e}"))
}

fn truncate(s: &str) -> &str {
    match s.char_indices().nth(300) {
        Some((at, _)) => &s[..at],
        None => s,
    }
}

fn print_report(r: &Report) {
    let d = |d: &Distribution| {
        format!(
            "p50 {:>8.2}  p90 {:>8.2}  p99 {:>8.2}  max {:>8.2}",
            d.p50, d.p90, d.p99, d.max
        )
    };
    eprintln!("── {} ── concurrency {}", r.label, r.concurrency);
    eprintln!(
        "  {} answered, {} failed, in {:.1}s: {:.1} req/s",
        r.requests, r.errors, r.elapsed_s, r.requests_per_s
    );
    for e in &r.first_errors {
        eprintln!("  error: {e}");
    }
    eprintln!("  latency ms  {}", d(&r.latency_ms));
    if r.ttft_ms.n > 0 {
        eprintln!("  first text  {}", d(&r.ttft_ms));
    }
    if let Some(p) = &r.process {
        eprintln!(
            "  forma: {:.2} ms CPU/request, {:.2} cores busy, RSS {:.0}→{:.0} MB, threads {}→{}",
            p.cpu_ms_per_request,
            p.cores_busy,
            p.rss_mb_start,
            p.rss_mb_peak,
            p.threads_start,
            p.threads_peak
        );
    }
    if let Some(m) = &r.mock {
        eprintln!(
            "  LLM: {:.1} calls/request, {:.1} kB/call, max {} at once, {} refused as malformed",
            m.calls_per_request, m.request_kb_mean, m.stats.max_inflight, m.stats.rejected
        );
    }
    if let Some(o) = &r.db_ops_per_request {
        eprintln!(
            "  database: {:.1} round trips/request ({:.1} insert, {:.1} find, {:.1} update, {:.1} delete, {:.1} other)",
            o.total(),
            o.insert,
            o.find,
            o.update,
            o.delete,
            o.other
        );
    }
    if let Some(p) = &r.phases {
        eprintln!("  where the time went ({} requests):", p.matched);
        eprintln!("    before 1st LLM call {}", d(&p.before_first_llm_call));
        eprintln!("    between LLM calls   {}", d(&p.between_llm_calls));
        eprintln!("    after last LLM call {}", d(&p.after_last_llm_call));
        eprintln!("    inside the LLM      {}", d(&p.in_llm));
        eprintln!("    Forma overhead      {}", d(&p.forma_overhead));
    }
}

/// Prints one table per scenario, with a row per target. Labels are
/// `target/scenario`, as `bench/run.sh` writes them.
fn summarise(args: SummariseArgs) -> Result<(), String> {
    let mut reports = vec![];
    let entries =
        std::fs::read_dir(&args.dir).map_err(|e| format!("could not read '{}': {e}", args.dir))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            if let Ok(report) = serde_json::from_str::<Report>(&text) {
                reports.push(report);
            }
        }
    }
    reports.sort_by(|a, b| a.label.cmp(&b.label));

    let mut scenarios: Vec<&str> = reports
        .iter()
        .map(|r| r.label.rsplit('/').next().unwrap_or_default())
        .collect();
    scenarios.sort();
    scenarios.dedup();

    let ms = |v: f64| format!("{v:.1}");
    for scenario in scenarios {
        let rows: Vec<&Report> = reports
            .iter()
            .filter(|r| r.label.rsplit('/').next() == Some(scenario))
            .collect();
        let concurrency = rows.first().map_or(0, |r| r.concurrency);
        println!("### {scenario} (concurrency {concurrency})\n");
        println!(
            "| target | req/s | p50 ms | p90 ms | p99 ms | first text p50 ms | CPU ms/req | cores busy | RSS peak MB | threads peak | errors | LLM refused |"
        );
        println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
        for r in &rows {
            let target = r.label.split('/').next().unwrap_or_default();
            let p = r.process.clone().unwrap_or_default();
            let refused = r.mock.as_ref().map_or(0, |m| m.stats.rejected);
            println!(
                "| {target} | {:.1} | {} | {} | {} | {} | {:.2} | {:.2} | {:.0} | {} | {} | {refused} |",
                r.requests_per_s,
                ms(r.latency_ms.p50),
                ms(r.latency_ms.p90),
                ms(r.latency_ms.p99),
                ms(r.ttft_ms.p50),
                p.cpu_ms_per_request,
                p.cores_busy,
                p.rss_mb_peak,
                p.threads_peak,
                r.errors
            );
        }
        let with_phases: Vec<(&&Report, &Phases)> = rows
            .iter()
            .filter_map(|r| r.phases.as_ref().map(|p| (r, p)))
            .collect();
        if !with_phases.is_empty() {
            println!("\nWhere the time went, per request (median ms):\n");
            println!(
                "| target | before 1st LLM call | between LLM calls | after last LLM call | inside the LLM | Forma overhead | p90 overhead | LLM calls |"
            );
            println!("|---|---:|---:|---:|---:|---:|---:|---:|");
            for (r, p) in with_phases {
                let target = r.label.split('/').next().unwrap_or_default();
                println!(
                    "| {target} | {} | {} | {} | {} | {} | {} | {} |",
                    ms(p.before_first_llm_call.p50),
                    ms(p.between_llm_calls.p50),
                    ms(p.after_last_llm_call.p50),
                    ms(p.in_llm.p50),
                    ms(p.forma_overhead.p50),
                    ms(p.forma_overhead.p90),
                    p.llm_calls
                );
            }
        }
        let with_db: Vec<(&&Report, &DbOps)> = rows
            .iter()
            .filter_map(|r| r.db_ops_per_request.as_ref().map(|o| (r, o)))
            .collect();
        if !with_db.is_empty() {
            println!("\nDatabase round trips per message:\n");
            println!("| target | total | insert | find | update | delete | other |");
            println!("|---|---:|---:|---:|---:|---:|---:|");
            for (r, o) in with_db {
                let target = r.label.split('/').next().unwrap_or_default();
                println!(
                    "| {target} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} |",
                    o.total(),
                    o.insert,
                    o.find,
                    o.update,
                    o.delete,
                    o.other
                );
            }
        }
        println!();
    }
    Ok(())
}

/// Forma's database. Reached with the driver rather than through the `db`
/// crate, whose backends are chosen by features: depending on it here would
/// switch one on for every `cargo check --workspace`.
async fn forma_db(mongo_url: &str) -> Result<mongodb::Database, String> {
    let client = mongodb::Client::with_uri_str(mongo_url)
        .await
        .map_err(|e| e.to_string())?;
    Ok(client.database(FORMA_DATABASE_ID))
}

/// Stores the agent as `db::mongo` stores a deployment: the agent itself as a
/// JSON string.
async fn deploy(args: DeployArgs) -> Result<(), String> {
    let agent = std::fs::read_to_string(&args.agent)
        .map_err(|e| format!("could not read '{}': {e}", args.agent))?;
    let agent: Value = serde_json::from_str(&agent).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now();
    let deployment = AgentDeployment {
        agent_id: agent["id"].as_str().unwrap_or("bench").to_string(),
        owner_id: "bench".to_string(),
        created_at: now,
        updated_at: now,
        version: 1,
        agent: Value::Null,
        credentials: None,
    };
    let mut document = mongodb::bson::to_document(&deployment).map_err(|e| e.to_string())?;
    document.insert("agent", agent.to_string());
    let db = forma_db(&args.mongo_url).await?;
    let inserted = db
        .collection::<mongodb::bson::Document>("deployments")
        .insert_one(document)
        .await
        .map_err(|e| e.to_string())?;
    let id = inserted
        .inserted_id
        .as_object_id()
        .ok_or("the deployment was not given an id")?;
    println!("{}", id.to_hex());
    Ok(())
}

async fn undeploy(args: UndeployArgs) -> Result<(), String> {
    let id = ObjectId::parse_str(&args.id).map_err(|e| e.to_string())?;
    forma_db(&args.mongo_url)
        .await?
        .collection::<mongodb::bson::Document>("deployments")
        .delete_one(doc! { "_id": id })
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tokio::main]
async fn main() {
    // Both rustls backends end up compiled in (see `forma::install_crypto_provider`).
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let result = match Cli::parse().command {
        Command::Run(args) => args.run().await,
        Command::Deploy(args) => deploy(args).await,
        Command::Undeploy(args) => undeploy(args).await,
        Command::Summarise(args) => summarise(args),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

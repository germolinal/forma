use std::{env, sync::LazyLock};

use genai_client::events::UsageTally;
use opentelemetry::{KeyValue, global, metrics::Counter};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, MetricExporter, SpanExporter};
use opentelemetry_resource_detectors::{OsResourceDetector, ProcessResourceDetector};
use opentelemetry_sdk::{
    Resource,
    logs::SdkLoggerProvider,
    metrics::{SdkMeterProvider, Temporality},
    propagation::TraceContextPropagator,
    resource::ResourceDetector,
    trace::{BatchConfigBuilder, BatchSpanProcessor, SdkTracerProvider},
};

use protocols::envar::get_env_var;
use serde::Serialize;
use tracing::{Level, info};
use tracing_subscriber::{
    EnvFilter,
    filter::{FilterExt, Targets},
    fmt::{self},
    prelude::*,
};

// Only the production console layer in `JustOtel` uses this.
#[cfg(not(feature = "cli"))]
use tracing_subscriber::filter::LevelFilter;

use opentelemetry::trace::TracerProvider as _;

/// What is logged: to the console, and as OpenTelemetry log records.
fn env_filter() -> EnvFilter {
    EnvFilter::try_from_env("FORMA_LOGS").unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Which spans are exported as traces. Separate from `FORMA_LOGS`, so that a
/// quiet log (`FORMA_LOGS=warn`) still exports every trace: with one filter
/// for both, `warn` dropped the (INFO) spans, and no trace was ever sent.
fn traces_filter() -> EnvFilter {
    EnvFilter::try_from_env("FORMA_TRACES").unwrap_or_else(|_| EnvFilter::new("info"))
}

fn crate_filter() -> Targets {
    // `Targets` only matches on `::` boundaries, so a `forma` directive does
    // not cover the `forma_manager` binary -- it needs its own entry, or the
    // manager's logs vanish in every mode that applies this filter.
    Targets::new()
        .with_target("tower_http", Level::DEBUG)
        .with_target("forma", Level::TRACE)
        .with_target("forma_manager", Level::TRACE)
        .with_target("agent", Level::TRACE)
        .with_target("genai_client", Level::TRACE)
        .with_target("protocols", Level::TRACE)
        .with_target("sessions", Level::TRACE)
        // OpenTelemetry's own warnings: a rejected or failed export. Without
        // them, lost telemetry is silent (and looks like a leak).
        .with_target("opentelemetry_sdk", Level::WARN)
        .with_target("opentelemetry-otlp", Level::WARN)
}

const TRACER_NAME: &str = "FormaTracer";

/// A flag that indicates whether to send telemetry to the
/// console, to an Opentelemetry collector, or both
#[derive(Default, Copy, Clone, PartialEq, Eq, Debug, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum TelemetryMode {
    /// Logs go to the console, nowhere else
    #[default]
    Console,
    /// Logs go to an OpenTelemetry collector, and the console
    Otel,

    /// Logs go only to an OpenTelemetry collector, not the console (useful for interactive REPL chat)
    JustOtel,
}

/// A telemeter that sends telemetry to the console or an OpenTelemetry collector
pub struct Telemeter {
    tracing_provider: Option<SdkTracerProvider>,
    logging_provider: Option<SdkLoggerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

impl Telemeter {
    /// Creates a new telemeter that sends telemetry to the console or an OpenTelemetry collector
    pub fn new(service_name: &str, mode: TelemetryMode) -> Self {
        match mode {
            TelemetryMode::Console => {
                init_local_logging();
                info!("Setting up console logging");
                Self {
                    tracing_provider: None,
                    logging_provider: None,
                    meter_provider: None,
                }
            }
            TelemetryMode::Otel => {
                let tracer = tracer(service_name);
                let logger = logger(service_name);
                let meter = meter(service_name);

                let tracing_layer =
                    tracing_opentelemetry::layer().with_tracer(tracer.tracer(TRACER_NAME));
                let logging_layer = OpenTelemetryTracingBridge::new(&logger);

                let ret = Self {
                    tracing_provider: Some(tracer),
                    logging_provider: Some(logger),
                    meter_provider: Some(meter),
                };

                #[cfg(not(feature = "cli"))]
                let with_ansi = false;
                #[cfg(feature = "cli")]
                let with_ansi = true;

                // A filter per layer: logs follow FORMA_LOGS, spans
                // FORMA_TRACES, and both only Forma's own crates.
                let subscriber = tracing_subscriber::registry()
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_ansi(with_ansi)
                            .with_filter(crate_filter().and(env_filter())),
                    )
                    .with(tracing_layer.with_filter(crate_filter().and(traces_filter())))
                    .with(logging_layer.with_filter(crate_filter().and(env_filter())));

                subscriber.init();

                ret
            }
            TelemetryMode::JustOtel => {
                let tracer = tracer(service_name);
                let logger = logger(service_name);
                let meter = meter(service_name);

                let tracing_layer =
                    tracing_opentelemetry::layer().with_tracer(tracer.tracer(TRACER_NAME));
                let logging_layer = OpenTelemetryTracingBridge::new(&logger);

                let ret = Self {
                    tracing_provider: Some(tracer),
                    logging_provider: Some(logger),
                    meter_provider: Some(meter),
                };

                // Production builds keep a WARN+ console layer. Cloud Run
                // ingests stdout into Cloud Logging on its own, so this is the
                // one path that still works when the OTLP route is broken --
                // without it, a collector that rejects logs is completely
                // silent. It is deliberately not the primary log path: app
                // logs go over OTLP so the collector can attach trace context.
                //
                // Interactive CLI builds get no console layer at all, which is
                // the whole point of this mode for REPL chat.
                // A filter per layer, as in `Otel`: logs follow FORMA_LOGS,
                // spans FORMA_TRACES.
                #[cfg(not(feature = "cli"))]
                let subscriber = tracing_subscriber::registry()
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_ansi(false)
                            .with_filter(LevelFilter::WARN.and(env_filter())),
                    )
                    .with(tracing_layer.with_filter(traces_filter()))
                    .with(logging_layer.with_filter(env_filter()));

                #[cfg(feature = "cli")]
                let subscriber = tracing_subscriber::registry()
                    .with(tracing_layer.with_filter(traces_filter()))
                    .with(logging_layer.with_filter(env_filter()));

                subscriber.init();

                ret
            }
        }
    }

    /// Shuts down the telemeter, flushing any pending telemetry data
    pub fn shutdown(self) {
        // We do not want these to panic if they fail... so we keep a Result<()> unchecked
        // This is why the let _a is there.
        if let Some(tp) = self.tracing_provider {
            let _a = tp.shutdown();
        }
        if let Some(lp) = self.logging_provider {
            let _a = lp.shutdown();
        }
        if let Some(mp) = self.meter_provider {
            let _a = mp.shutdown();
        }
    }
}

fn get_resource(service_name: &str) -> Resource {
    let detectors: Vec<Box<dyn ResourceDetector>> = vec![
        Box::new(OsResourceDetector),
        Box::new(ProcessResourceDetector),
    ];
    let mut res = Resource::builder()
        .with_detectors(&detectors)
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .with_attribute(KeyValue::new("service.name", service_name.to_string()));

    if let Some(project_id) = get_env_var("GOOGLE_CLOUD_PROJECT").expect("could not get ") {
        res = res.with_attribute(KeyValue::new("gcp.project_id", project_id));
    }
    res.with_service_name(service_name.to_string().clone())
        .build()
}

/// Endpoint is set through env OTEL_EXPORTER_OTLP_METRICS_ENDPOINT, defaults to `https://localhost:4317/v1/metrics`
fn meter(service_name: &str) -> SdkMeterProvider {
    // Cumulative, not Delta: these metrics are exported through
    // `googlemanagedprometheus`, and the Prometheus data model is cumulative.
    // The spec says a Prometheus exporter *should* convert monotonic delta sums
    // on the way through, but relying on that is how telemetry disappears.
    let exporter = MetricExporter::builder()
        .with_temporality(Temporality::Cumulative)
        .with_tonic()
        .build()
        .expect("Failed to create OTLP metric provider");
    let provider = SdkMeterProvider::builder()
        .with_resource(get_resource(service_name))
        .with_periodic_exporter(exporter)
        .build();
    global::set_meter_provider(provider.clone());
    provider
}

// endpoint set through 'OTEL_EXPORTER_OTLP_ENDPOINT'. defaults to  `https://localhost:4317`
fn tracer(service_name: &str) -> SdkTracerProvider {
    global::set_text_map_propagator(TraceContextPropagator::new());
    let exporter = SpanExporter::builder()
        .with_tonic()
        .build()
        .expect("Failed to create OTLP tracing provider");
    // Smaller batches than the SDK's 512 spans, unless the standard variable
    // says otherwise. An LLM span carries its conversation, so 512 of them
    // can exceed a collector's gRPC message limit (4 MiB by default): the
    // collector refuses the whole batch, every span in it is lost, and the
    // queue of waiting spans grows to hundreds of MB.
    let mut config = BatchConfigBuilder::default();
    if env::var_os("OTEL_BSP_MAX_EXPORT_BATCH_SIZE").is_none() {
        config = config.with_max_export_batch_size(SPAN_EXPORT_BATCH_SIZE);
    }
    let processor = BatchSpanProcessor::builder(exporter)
        .with_batch_config(config.build())
        .build();
    SdkTracerProvider::builder()
        .with_resource(get_resource(service_name))
        .with_span_processor(processor)
        .build()
}

/// Spans per export when `OTEL_BSP_MAX_EXPORT_BATCH_SIZE` is not set.
const SPAN_EXPORT_BATCH_SIZE: usize = 64;

fn logger(service_name: &str) -> SdkLoggerProvider {
    let exporter = LogExporter::builder()
        .with_tonic()
        .build()
        .expect("Failed to create OTLP log provider");
    SdkLoggerProvider::builder()
        .with_resource(get_resource(service_name))
        .with_batch_exporter(exporter)
        .build()
}

/// Name reported as the instrumentation scope for every usage instrument.
const METER_NAME: &str = "forma";

/// How an agent run ended.
///
/// Deliberately a closed set: this is a metric label, and every variant added
/// here multiplies the number of time series.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    /// The agent produced its answer.
    Completed,
    /// The run stopped waiting for human approval of a tool call.
    Suspended,
    /// The run failed part way through. It still reports whatever it spent
    /// before dying.
    Errored,
    /// The request went away before the run finished — the client left, or
    /// the platform's request timeout cut it off. Reports what was spent up
    /// to then.
    Dropped,
}

/// A run's tally that is recorded exactly once: by [`UsageRecorder::finish`]
/// when the run ends, or, if it never gets there, when it is dropped.
///
/// A streamed run lives inside its response; when the request goes away the
/// stream is dropped mid-loop and the code after the loop never runs. Without
/// this, everything such a run spent would be missing from the metrics.
pub struct UsageRecorder {
    /// What the run has spent so far; the run folds its events in here.
    pub tally: UsageTally,
    mode: &'static str,
    recorded: bool,
}

impl UsageRecorder {
    /// An empty tally for a run in `mode`, not yet recorded.
    pub fn new(mode: &'static str) -> Self {
        Self {
            tally: UsageTally::new(),
            mode,
            recorded: false,
        }
    }

    /// Records the tally under `outcome`. Later calls, and the drop, do nothing.
    pub fn finish(&mut self, outcome: RunOutcome) {
        if !self.recorded {
            self.recorded = true;
            record_usage(&self.tally, self.mode, outcome);
        }
    }
}

impl Drop for UsageRecorder {
    fn drop(&mut self) {
        self.finish(RunOutcome::Dropped);
    }
}

impl RunOutcome {
    fn as_str(self) -> &'static str {
        match self {
            RunOutcome::Completed => "completed",
            RunOutcome::Suspended => "suspended",
            RunOutcome::Errored => "errored",
            RunOutcome::Dropped => "dropped",
        }
    }
}

/// Built once and reused: creating an instrument per run would take the meter's
/// internal locks on every request for no benefit, since the SDK dedupes by
/// name anyway.
struct UsageInstruments {
    runs: Counter<u64>,
    llm_calls: Counter<u64>,
    tokens: Counter<u64>,
    token_subsets: Counter<u64>,
    tool_calls: Counter<u64>,
    tool_usage: Counter<u64>,
}

static USAGE_INSTRUMENTS: LazyLock<UsageInstruments> = LazyLock::new(|| {
    let meter = global::meter(METER_NAME);
    UsageInstruments {
        runs: meter
            .u64_counter("forma.agent.runs")
            .with_description("Agent runs handled, i.e. messages answered.")
            .with_unit("{run}")
            .build(),
        llm_calls: meter
            .u64_counter("forma.llm.calls")
            .with_description("Provider round trips.")
            .with_unit("{call}")
            .build(),
        tokens: meter
            .u64_counter("forma.llm.tokens")
            .with_description(
                "Tokens billed. `input` and `output` are disjoint, so summing \
                 across token_type gives the true total.",
            )
            .with_unit("{token}")
            .build(),
        token_subsets: meter
            .u64_counter("forma.llm.tokens.subset")
            .with_description(
                "Breakdown of tokens already counted by forma.llm.tokens -- cached \
                 input, reasoning and audio are subsets of input/output. Kept on a \
                 separate instrument so that summing forma.llm.tokens cannot double \
                 count.",
            )
            .with_unit("{token}")
            .build(),
        tool_calls: meter
            .u64_counter("forma.tool.calls")
            .with_description("Tool invocations, by the machinery that ran them.")
            .with_unit("{call}")
            .build(),
        tool_usage: meter
            .u64_counter("forma.tool.usage")
            .with_description(
                "What tools consumed beyond the call, in the unit they are priced \
                 by (e.g. code-execution seconds), labelled with that unit.",
            )
            .build(),
    }
});

/// Records one agent run's spend.
///
/// Takes no identifiers on purpose. `job_id`, `session_id` and `user_id` are
/// unbounded, and on a metrics backend every distinct label value is its own
/// time series -- one per run would be pathological. Those IDs belong on spans
/// and log records, where they cost nothing extra and are exactly what makes an
/// individual run findable. Everything labelled here is bounded by
/// construction: `tool_kind` is a fixed enum, `mode` and `outcome` are closed
/// sets, and provider/model come from deployed agent definitions.
pub fn record_usage(usage: &UsageTally, mode: &'static str, outcome: RunOutcome) {
    let instruments = &*USAGE_INSTRUMENTS;

    instruments.runs.add(
        1,
        &[
            KeyValue::new("mode", mode),
            KeyValue::new("outcome", outcome.as_str()),
        ],
    );

    for entry in &usage.tokens {
        let model = [
            KeyValue::new("provider", entry.provider.clone()),
            KeyValue::new("model", entry.model.clone()),
        ];
        let with_type = |token_type: &'static str| {
            [
                model[0].clone(),
                model[1].clone(),
                KeyValue::new("token_type", token_type),
            ]
        };

        instruments.llm_calls.add(entry.calls as u64, &model);

        let t = &entry.usage;
        for (token_type, value) in [("input", t.input_tokens), ("output", t.output_tokens)] {
            if value > 0 {
                instruments.tokens.add(value as u64, &with_type(token_type));
            }
        }
        for (token_type, value) in [
            ("cached_input", t.cached_input_tokens),
            ("reasoning", t.reasoning_tokens),
            ("input_audio", t.input_audio_tokens),
            ("output_audio", t.output_audio_tokens),
        ] {
            if value > 0 {
                instruments
                    .token_subsets
                    .add(value as u64, &with_type(token_type));
            }
        }
    }

    for kind in &usage.tools {
        // Runtimes that predate `tool_kind` send it empty; bucket those rather
        // than emitting a blank label value.
        let kind = if kind.is_empty() {
            "unknown".to_string()
        } else {
            kind.clone()
        };
        instruments
            .tool_calls
            .add(1, &[KeyValue::new("tool_kind", kind)]);
    }

    for u in &usage.tool_usage {
        instruments.tool_usage.add(
            u.quantity,
            &[
                KeyValue::new("tool_kind", u.tool_kind.clone()),
                KeyValue::new("unit", u.unit.clone()),
            ],
        );
    }
}

/// Resolves when the process is asked to stop.
///
/// Cloud Run sends SIGTERM and then waits before killing the instance, so this
/// is what gives [`Telemeter::shutdown`] a chance to run at all: without it,
/// `axum::serve` only returns on error and whatever the batch exporters have
/// queued dies with the process on every scale-down.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            // Nothing sensible to do if the handler cannot be installed; fall
            // back to ctrl_c rather than tearing the server down.
            Err(e) => {
                tracing::warn!(error = %e, "could not install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("shutdown signal received, flushing telemetry");
}

/// Start local logging
pub fn init_local_logging() {
    let format = fmt::format()
        .with_target(false)
        // .without_time()
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_line_number(false)
        .with_file(false)
        .pretty();
    let fmt_layer = fmt::layer().event_format(format);
    let crate_filter = crate_filter();
    let env_filter = env_filter();
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(crate_filter)
        .with(env_filter)
        .init();
}

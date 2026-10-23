#!/usr/bin/env bash
# Measures Forma against a mock LLM. Run it with `make bench` from forma/.
#
# What it does, and every knob below, is described under "Benchmarking" in
# forma/README.md. Results go to bench/results/<date>-<commit>/, with
# report.md as the summary.
set -euo pipefail

cd "$(dirname "$0")/.."

TARGETS=${BENCH_TARGETS:-"none mongo-mt"}
MOCK_PORT=${MOCK_LLM_PORT:-11500}
FORMA_PORT=${BENCH_FORMA_PORT:-18080}
MONGO_PORT=${BENCH_MONGO_PORT:-27018}
DB_URL=${BENCH_SESSIONS_DB_URL:-}
REQUESTS=${BENCH_REQUESTS:-300}
DURATION=${BENCH_DURATION_S:-15}
CONCURRENCY=${BENCH_CONCURRENCY:-32}
REALISTIC_CONCURRENCY=${BENCH_REALISTIC_CONCURRENCY:-64}
TELEMETRY=${BENCH_TELEMETRY:-console}
LOGS=${BENCH_FORMA_LOGS:-warn}
SEED_MESSAGES=${BENCH_DB_MESSAGES:-50000}
DB_INDEXES=${BENCH_DB_INDEXES:-0}
OTEL_PORT=${BENCH_OTEL_PORT:-14317}
OTEL_IMAGE=${BENCH_OTEL_IMAGE:-otel/opentelemetry-collector:0.115.1}
# 0 lets the mock accept conversations a real provider would refuse.
STRICT=$([ "${BENCH_MOCK_STRICT:-1}" = 0 ] && echo false || echo true)
API_KEY=bench-key

sha=$(git rev-parse --short HEAD)
git diff --quiet HEAD -- . || sha="$sha-dirty"
OUT=${BENCH_OUT:-bench/results/$(date +%Y%m%d-%H%M%S)-$sha}
# Prebuilt `forma-<target>` and `forma-cli` binaries, e.g. from another
# branch, to measure instead of building the working tree.
BIN=${BENCH_BIN:-target/bench}
LOAD=target/release/forma-load
mkdir -p "$OUT/agent" "$BIN"

# A target is a production build, and the storage its runtime.yaml names. The
# client is always `sse`.
features() {
	case $1 in
	none | mongo) echo "" ;;
	mongo-mt) echo "multi-tenant" ;;
	*)
		echo "unknown target '$1': use none, mongo or mongo-mt" >&2
		return 1
		;;
	esac
}

storage() {
	case $1 in
	none) echo "none" ;;
	mongo | mongo-mt) echo "mongo" ;;
	esac
}

MOCK_PID=
FORMA_PID=
MONGO_CONTAINER=
OTEL_CONTAINER=
cleanup() {
	if [ -n "$FORMA_PID" ]; then kill "$FORMA_PID" 2>/dev/null || true; fi
	if [ -n "$MOCK_PID" ]; then kill "$MOCK_PID" 2>/dev/null || true; fi
	if [ -n "$MONGO_CONTAINER" ]; then docker stop "$MONGO_CONTAINER" >/dev/null 2>&1 || true; fi
	if [ -n "$OTEL_CONTAINER" ]; then docker stop "$OTEL_CONTAINER" >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT

wait_for_port() {
	for _ in $(seq 1 300); do
		if curl -s -o /dev/null "http://127.0.0.1:$1/"; then return 0; fi
		sleep 0.1
	done
	echo "nothing is listening on port $1" >&2
	return 1
}

echo "==> building (release)"
cargo build --release -p forma-bench
if [ -z "${BENCH_BIN:-}" ]; then
	cargo build --release --bin forma
	cp target/release/forma "$BIN/forma-cli"
	for t in $TARGETS; do
		cargo build --release --bin forma --no-default-features --features "$(features "$t")"
		cp target/release/forma "$BIN/forma-$t"
	done
fi

echo "==> compiling the bench agent"
sed "s/__MOCK_LLM_PORT__/$MOCK_PORT/" bench/agent/src/agent.yaml >"$OUT/agent/agent.yaml"
cp -R bench/agent/src/prompts "$OUT/agent/"
OPENAI_API_KEY=bench "$BIN/forma-cli" expand -c --format json -p "$OUT/agent" -a agent >"$OUT/agent/agent.json"

echo "==> starting the mock LLM on port $MOCK_PORT"
target/release/mock-llm --port "$MOCK_PORT" >"$OUT/mock-llm.log" 2>&1 &
MOCK_PID=$!
wait_for_port "$MOCK_PORT"

# Telemetry that goes nowhere costs less than telemetry that is sent: unless
# OTEL_EXPORTER_OTLP_ENDPOINT names a collector of your own, the `otel` modes
# send to a throwaway one, so that what is measured includes exporting.
case $TELEMETRY in
otel | just-otel)
	if [ -z "${OTEL_EXPORTER_OTLP_ENDPOINT:-}" ]; then
		echo "==> starting a throwaway OpenTelemetry Collector on port $OTEL_PORT"
		OTEL_CONTAINER=forma-bench-otel
		docker rm -f "$OTEL_CONTAINER" >/dev/null 2>&1 || true
		# bench/otel-collector.yaml receives OTLP and drops it, counting what
		# arrived. (The image's own configuration prints every span in full,
		# which makes the collector the bottleneck.)
		docker run -d --rm --name "$OTEL_CONTAINER" -p "$OTEL_PORT:4317" \
			-v "$PWD/bench/otel-collector.yaml:/etc/otelcol/config.yaml:ro" "$OTEL_IMAGE" >/dev/null
		export OTEL_EXPORTER_OTLP_ENDPOINT="http://127.0.0.1:$OTEL_PORT"
		sleep 3
	fi
	;;
esac

needs_mongo=no
for t in $TARGETS; do
	case $t in mongo*) needs_mongo=yes ;; esac
done
CLEANUP_ARGS=()
if [ "$needs_mongo" = yes ]; then
	if [ -z "$DB_URL" ]; then
		echo "==> starting a throwaway MongoDB on port $MONGO_PORT"
		MONGO_CONTAINER=forma-bench-mongo
		docker rm -f "$MONGO_CONTAINER" >/dev/null 2>&1 || true
		docker run -d --rm --name "$MONGO_CONTAINER" -p "$MONGO_PORT:27017" mongo >/dev/null
		DB_URL="mongodb://127.0.0.1:$MONGO_PORT/?directConnection=true"
		until docker exec "$MONGO_CONTAINER" mongosh --quiet --eval 'db.runCommand({ ping: 1 }).ok' >/dev/null 2>&1; do
			sleep 0.5
		done
	else
		# Someone else's database: take away the sessions the run opens.
		CLEANUP_ARGS=(--cleanup-db "$DB_URL")
	fi
fi

# Empties the throwaway database and fills it with other people's
# conversations, as a database in use would be: how long a session takes to
# read can depend on everything else stored.
fresh_database() {
	docker exec "$MONGO_CONTAINER" mongosh --quiet --eval "
		const d = db.getSiblingDB('forma-db');
		d.dropDatabase();
		const n = $SEED_MESSAGES;
		const text = 'Part of an earlier conversation, about something else. '.repeat(4);
		for (let i = 0; i < n; i += 1000) {
			const batch = [];
			for (let j = i; j < Math.min(n, i + 1000); j++) {
				batch.push({
					id: 'seed-' + j,
					role: j % 2 ? 'assistant' : 'user',
					content: [{ type: 'text', text: text }],
					session_id: 'seed-session-' + (j % 5000),
					user_id: 'seed-user-' + (j % 5000),
					created_at: new Date(),
				});
			}
			d.messages.insertMany(batch, { ordered: false });
		}
		// The indexes recommended for the sessions database.
		if ($DB_INDEXES) {
			d.messages.createIndex({ session_id: 1, created_at: -1 });
			d.messages.createIndex({ id: 1, session_id: 1 });
			d.sessions.createIndex({ user_id: 1, agent_id: 1, created_at: -1 });
		}" >/dev/null
}

start_forma() {
	printf 'client: sse\nstorage: %s\n' "$(storage "$1")" >"$OUT/$1.runtime.yaml"
	local args=(serve -t "$TELEMETRY" -r "$OUT/$1.runtime.yaml")
	case $1 in *-mt) ;; *) args+=(-a "$OUT/agent/agent.json") ;; esac
	env -i PATH="$PATH" HOME="$HOME" \
		PORT="$FORMA_PORT" FORMA_AGENT_KEY="$API_KEY" OPENAI_API_KEY=bench \
		FORMA_LOGS="$LOGS" SESSIONS_DB_URL="$DB_URL" \
		${OTEL_EXPORTER_OTLP_ENDPOINT:+OTEL_EXPORTER_OTLP_ENDPOINT="$OTEL_EXPORTER_OTLP_ENDPOINT"} \
		${FORMA_TRACES:+FORMA_TRACES="$FORMA_TRACES"} \
		"$BIN/forma-$1" "${args[@]}" >"$OUT/$1.forma.log" 2>&1 &
	FORMA_PID=$!
	wait_for_port "$FORMA_PORT"
}

stop_forma() {
	kill "$FORMA_PID" 2>/dev/null || true
	wait "$FORMA_PID" 2>/dev/null || true
	FORMA_PID=
}

time_wait_sockets() {
	if command -v ss >/dev/null 2>&1; then
		ss -tan state time-wait 2>/dev/null | tail -n +2 | wc -l
	else
		netstat -an 2>/dev/null | grep -c TIME_WAIT || true
	fi
}

# Waits for the connections a scenario closed to leave TIME_WAIT. A Forma that
# opens a connection per LLM call can use up every local port in seconds, and
# then the next scenario (and this script) could not connect to anything.
settle() {
	for _ in $(seq 1 90); do
		if [ "$(time_wait_sockets)" -lt 1000 ]; then return 0; fi
		sleep 1
	done
}

mock() {
	settle
	curl -sf --retry 5 --retry-all-errors -X POST "http://127.0.0.1:$MOCK_PORT/config" \
		-H 'content-type: application/json' -d "$1" >/dev/null
}

# Samples Forma's stacks for a few seconds, once the load is going.
profile() {
	sleep 3
	if command -v sample >/dev/null 2>&1; then
		sample "$FORMA_PID" 5 -mayDie -file "$OUT/$1.profile.txt" >/dev/null 2>&1 || true
	elif command -v perf >/dev/null 2>&1; then
		perf record -g -o "$OUT/$1.perf.data" -p "$FORMA_PID" -- sleep 5 >/dev/null 2>&1 || true
	fi
}

SESSION_ARGS=()
load() {
	local target=$1 scenario=$2
	shift 2
	"$LOAD" run --url "http://127.0.0.1:$FORMA_PORT" --mode sse --api-key "$API_KEY" \
		--mock-url "http://127.0.0.1:$MOCK_PORT" --pid "$FORMA_PID" \
		--label "$target/$scenario" --json-out "$OUT/$target.$scenario.json" \
		${SESSION_ARGS[@]+"${SESSION_ARGS[@]}"} "$@"
}

for t in $TARGETS; do
	echo "==> $t (features: [$(features "$t")], storage: $(storage "$t"))"
	case $t in
	mongo*) if [ -n "$MONGO_CONTAINER" ]; then fresh_database; fi ;;
	esac
	start_forma "$t"
	SESSION_ARGS=()
	deployment=
	case $t in
	mongo*)
		SESSION_ARGS+=(--persist ${CLEANUP_ARGS[@]+"${CLEANUP_ARGS[@]}"})
		# Counting operations needs a database that serves nothing else.
		if [ -n "$MONGO_CONTAINER" ]; then SESSION_ARGS+=(--db-url "$DB_URL"); fi
		;;
	esac
	case $t in
	*-mt)
		deployment=$("$LOAD" deploy --agent "$OUT/agent/agent.json" --mongo-url "$DB_URL")
		SESSION_ARGS+=(--deployment-id "$deployment")
		;;
	esac

	# An LLM that answers at once: what is left is Forma.
	mock "{\"ttft_ms\": 0, \"chunk_interval_ms\": 0, \"chunks\": 32, \"tool_calls\": true, \"strict\": $STRICT}"
	load "$t" 1-latency --concurrency 1 --requests "$REQUESTS" --warmup 20

	settle
	profile "$t" &
	profiler=$!
	load "$t" 2-throughput --concurrency "$CONCURRENCY" --duration-s "$DURATION" --warmup 50
	wait "$profiler" || true

	# An LLM that takes as long as a real one: 400 ms to the first token,
	# then 40 tokens 20 ms apart.
	mock '{"ttft_ms": 400, "chunk_interval_ms": 20, "chunks": 40}'
	load "$t" 3-realistic --concurrency "$REALISTIC_CONCURRENCY" --duration-s "$DURATION" \
		--warmup "$REALISTIC_CONCURRENCY"

	if [ -n "$deployment" ]; then
		"$LOAD" undeploy --id "$deployment" --mongo-url "$DB_URL"
	fi
	stop_forma
	settle
done

{
	echo "# Forma benchmark: $sha"
	echo
	echo "$(date '+%Y-%m-%d %H:%M'), $(uname -sm), $(getconf _NPROCESSORS_ONLN) CPUs."
	echo "Targets: $TARGETS. Telemetry: \`$TELEMETRY\`, \`FORMA_LOGS=$LOGS\`."
	if [ "$STRICT" = false ]; then echo "The mock accepted conversations a real provider would refuse (\`BENCH_MOCK_STRICT=0\`)."; fi
	if [ -n "$OTEL_CONTAINER" ]; then echo "Telemetry was exported to a throwaway OpenTelemetry Collector (\`$OTEL_IMAGE\`)."; fi
	if [ -n "$MONGO_CONTAINER" ]; then
		echo "Each MongoDB target starts on a fresh database holding $SEED_MESSAGES messages of other conversations$([ "$DB_INDEXES" = 1 ] && echo ', with indexes on what Forma queries' || echo ', without indexes')."
	fi
	if [ -n "${BENCH_BIN:-}" ]; then echo "Binaries: \`$BIN\`."; fi
	echo
	echo "Each request is one user message, answered with the whole tool loop:"
	echo "an LLM call that asks for a tool, the tool, and an LLM call that answers."
	echo "\`1-latency\` and \`2-throughput\` use an LLM that answers at once;"
	echo "\`3-realistic\` one that takes 400 ms to the first token and streams 40"
	echo "tokens 20 ms apart."
	echo
	"$LOAD" summarise "$OUT"
	for t in $TARGETS; do
		if [ -f "$OUT/$t.profile.txt" ]; then
			echo "### Where the CPU went: $t, 2-throughput"
			echo
			echo "Top of the stack in 5 s of samples, idle threads left out (full profile in \`$t.profile.txt\`)."
			echo
			echo '```'
			awk '/Sort by top of stack/ { found = 1; next } found && NF == 0 { exit } found' "$OUT/$t.profile.txt" |
				grep -vE 'psynch_cvwait|kevent|workq_kernreturn|mach_msg|semwait_signal|__select|__wait4|nanosleep|__ulock_wait|start_wqthread' |
				head -30 || true
			echo '```'
			echo
		fi
	done
} >"$OUT/report.md"

echo
cat "$OUT/report.md"
echo "==> results in $OUT"

check_dependencies:
	cargo outdated --depth 2 --workspace

cov:
	cargo tarpaulin --ignore-tests --exclude derive --timeout 120 --features airtight --out Xml --workspace -- --skip write_docs  && python3 -m pycobertura show -f html -o cov.html cobertura.xml

docs:
	cargo test --package forma --lib --features cli -- print_docs::test::write_docs --exact --nocapture

integration_test:
	cargo test --features integration_test && \
 	cargo test --workspace --features integration_test

# Both builds: the CLI's (the default features) and production's, which have
# tests of their own.
test:
	$(MONGO_URL) cargo test --workspace --features airtight -- --skip write_docs
	$(MONGO_URL) cargo test --workspace --no-default-features --features airtight


MONGO_URL=SESSIONS_DB_URL=mongodb://root:example@localhost:27017/?directConnection=true
# The builds Forma ships: production servers (single- or multi-tenant, with or
# without hot reloading), the manager (`api`) and the CLI. Which client, storage,
# workspaces and sandbox a server uses is not a build: it is in runtime.yaml.
FEATURE_SETS := "" multi-tenant hot-reload multi-tenant,hot-reload api cli

.PHONY: check_dependencies cov docs integration_test test check-features pr_checks fix lint bench mock-llm

# Measures Forma against a mock LLM: latency, throughput, CPU, memory and where
# each request's time goes. Writes bench/results/<date>-<commit>/report.md.
# Knobs are environment variables (BENCH_TARGETS=none make bench); see
# "Benchmarking" in README.md.
bench:
	bench/run.sh

# Runs the mock LLM on its own (port 11500), to point an agent at by hand.
mock-llm:
	cargo run --release -p forma-bench --bin mock-llm -- $(MOCK_LLM_ARGS)

# Run by .github/workflows/ci.yml. `set -e` is load-bearing: a `for` loop reports the
# exit status of its *last* command, so without it a failing `cargo check` is
# swallowed and the loop still reports success.
check-features:
	@set -e; for features in $(FEATURE_SETS); do \
		echo "==> [$$features]"; \
		cargo check --workspace --tests --release --no-default-features --features "$$features"; \
		cargo clippy --workspace --release --no-default-features --features "$$features" -- -D warnings; \
	done

pr_checks: check-features lint test

fix:
	cargo clippy --workspace --fix && \
	cargo fmt

lint:
	cargo fmt -- --check && \
	cargo check --tests --workspace --features integration_test && \
	cargo clippy --workspace -- -D warnings

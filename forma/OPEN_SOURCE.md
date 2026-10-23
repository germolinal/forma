# Releasing Forma (possibly as open source)

Notes for a decision that is not made yet: whether, and how, to release Forma
on its own, outside Clonsult. Written 2026-10-21. Nothing here is done unless it
says so.

## Goals

1. **Base Docker images** for each runtime, the CLI and the manager, published
   so that anyone can build their agent's image on top of them.
2. **CLI binaries** that people download and run, without Rust.
3. **A CI/CD image** that checks and evaluates an agent, so that agents ship
   through ordinary pipelines (GitHub Actions, Cloud Build, GitLab...).
4. **Documentation** of how Forma works, and of every way to run it: one agent
   in a container, a multi-tenant platform like Clonsult's, and the CLI.
5. **The CLI as a production server** for small installations, not only for
   development.

## Where things stand

What already exists, and what each goal can start from:

| Piece | State |
| --- | --- |
| `Dockerfile.channel` | Builds `/app/forma` with `FEATURES` (empty, or `multi-tenant`), on distroless with the base pinned by digest. What Clonsult's channels and self-host releases use. The natural source of the runtime images |
| `Dockerfile.manager` | Builds `forma-manager` (`api`) |
| `Dockerfile.cli` | Builds the CLI with default features on `debian:trixie-slim`, `CMD forma --help`. A start for the CI/CD image |
| `build_cli.yaml` | Cloud Build config for an `enterprise-template` project. A Clonsult leftover: remove or replace (`Dockerfile.single_tenant` and `build_single_tenant.yaml` are gone) |
| `Dockerfile.tester` | Rust with clippy and rustfmt, for Forma's own CI |
| `cicd/` (repo root) | Clonsult's pipelines, including `release-binaries.sh`, which packages the `channel` binary for self-hosted Clonsult. Not Forma's release process, but it shows how to extract binaries from the images |
| `LICENSE` | MIT, "Copyright (c) 2025 Forma AI". Confirm the holder before publishing |
| `docs/` | mdBook, published with the website at `germolinal.github.io/forma/docs`. Covers single-tenant deployment (`deployment.md`, `cloud-deployment.md`), the HTTP API, the channels, evaluations. The reference pages are generated from doc comments (`make docs`) and git-ignored |
| `typescript-sdk/` | No `package.json`: consumed by path from `front/`. Not publishable as it is |
| `forma eval` | Already fails (non-zero exit) when a metric scores below its `required_score`, unless `--no-fail`. Good enough for a CI gate |

## 1. Base images

**Decided (2026-10-22): one production binary per tenancy.** Every client
(`sync`, `sse`, `ai-sdk-v5`, `whatsapp`, `telegram`) and every storage
(`mongo`, `file`, `none`) is compiled in, and a server reads which ones it uses
from `runtime.yaml`, as the CLI does, along with where workspaces are kept and
where code runs. Only tenancy (`multi-tenant`) and `hot-reload` remain features.
Measured before deciding: the binary with everything is the same size as one
with a single runtime (≈12 MB gzipped, either way), and choosing at start costs
a `match` per call. See `OPEN_SOURCE_DECISIONS.md`.

So the first set of images is:

- `forma` (single tenant), and `forma-multi-tenant`, for platforms;
- `forma-manager`;
- `forma-cli` (also the base of the CI/CD image, below).

TODO:

- [ ] Registry: GitHub Container Registry (`ghcr.io/ai-forma/...`), which needs
      nothing but the GitHub organisation.
- [ ] Tags: the version (`0.9.18`), `0.9`, `latest`; always also by digest. Record
      the Forma commit in a label (`org.opencontainers.image.revision`).
- [ ] Multi-arch (`linux/amd64`, `linux/arm64`) with `docker buildx`. Cloud Run
      only needs amd64, but people build on Apple Silicon, where emulated Rust
      builds take far too long (`cloud-deployment.md` warns about it).
- [ ] Build in CI (GitHub Actions, if the repository moves to GitHub), not from
      someone's laptop. Cache the cargo build: a cold build is long.
- [ ] Sign images (cosign) and attach an SBOM. Cheap, and expected of
      infrastructure people run with their secrets.
- [ ] Keep the distroless base pinned by digest, and bump it on purpose (as
      `Dockerfile.channel` does now).
- [ ] Document the image contract: `/app/forma serve -a /app/agent.yaml`, which
      variables each runtime needs, the port, what `-t` does.

## 2. CLI binaries

TODO:

- [ ] Targets: Linux `x86_64` and `aarch64` (glibc; check whether musl is needed
      for people on Alpine), macOS `aarch64` and `x86_64` (or universal),
      Windows `x86_64` if the CLI works there (paths, file watching and the
      FileDB have never been tried on Windows).
- [ ] A release tool: [cargo-dist](https://opensource.axo.dev/cargo-dist/) builds
      the archives, checksums, an install script and a Homebrew formula from
      the tag. Cheaper than writing it.
- [ ] macOS signing and notarisation, or Gatekeeper blocks the binary.
- [ ] `forma --version` from the release tag (it says `0.2.0` from `Cargo.toml`
      today, while releases are `v0.9.x`): one version for everything.
- [x] The getting-started page (`docs/src/how-to/getting-started.md`)
      installs from the download now, with `cargo install` as the alternative.
- [ ] Check what the CLI needs at runtime besides itself: TLS is rustls, but
      the binary is linked against glibc; Ollama for the starter agent; Docker
      for `forma init`'s Phoenix and for the `local-docker` sandbox.

## 3. CI/CD image

An image that runs in any pipeline, checks an agent, evaluates it, and fails
the job if it is not good enough. Then deploys it, or hands the compiled agent
to whatever deploys.

What it runs, in order:

1. `forma expand -c` — the agent compiles: unknown fields, broken `$` references
   and tools that cannot be built fail here, without spending anything.
2. `forma eval` — the datasets and metrics in `evals/`. Fails below
   `required_score` already.
3. The output: the compiled `agent.yaml` (what a production image needs), and
   the evaluation report.

TODO:

- [ ] Base it on `forma-cli`. Decide how evals reach their LLMs from CI: API keys
      as CI secrets, or workload identity for Vertex AI. The judge's model too.
- [ ] Reports a CI system can show: a Markdown summary (GitHub Actions job
      summary), and JUnit XML so each failed metric shows as a failed test.
- [ ] Make evaluation cost visible, and capped: an eval run on every push can
      cost real money. Maybe a `--max-cost` or a sample size.
- [ ] Datasets: where they live in CI (the repository, or `forma dataset-*` from
      Phoenix), and whether they are versioned with the agent.
- [ ] Ship a GitHub Action (`ai-forma/forma-action`) wrapping the image, and a
      ready workflow: check and eval on every pull request; build the agent's
      image on top of `forma-<runtime>` and deploy on merge. `cloud-deployment.md`
      is the deploy half, written for people.
- [ ] Non-determinism: the same agent scores differently run to run. Document
      how to set `required_score` with that in mind (repetitions, margins).

## 4. Documentation

`docs` is for people who build agents in depth, and who run Forma
themselves. Clonsult's product is documented by its own UI, not here.

Covered today: single-tenant deployment (binary and container), the cloud walk-
through (Cloud Run), the HTTP API, WhatsApp and Telegram, evaluations,
observability, human in the loop.

TODO:

- [ ] **Running a multi-tenant platform**, Clonsult's architecture without
      Clonsult: the manager, the runtimes, the database they share, and the
      reverse proxy in front, which is the part people have to build
      themselves. Its contract has to be written down exactly:
      - which headers the runtime trusts (`X-Deployment-ID`, `X-User-ID`,
        `X-User-Roles`, `X-Time-Zone`): the proxy must set them from what it
        authenticated, and strip whatever the client sent;
      - webhooks: one URL per deployment on the proxy, forwarded to
        `/v1/chat` with the deployment's id, the body unchanged and the
        platform's signature headers kept;
      - per-deployment credentials (`whatsapp_api`, `telegram_api`,
        `workspaces`), and `FORMA_ENCRYPTION_KEY`, shared by manager and
        runtimes;
      - who registers webhooks (Clonsult's backend calls Telegram's
        `setWebhook` at deploy time);
      - usage reporting for billing (the tallies at the end of a stream, or in
        a webhook's response body).
      `back/` is the reference implementation; a minimal proxy example (a few
      hundred lines, any language) would be worth more than prose.
- [ ] **The manager's API**, complete (`deployment.md` has a table, without the
      bodies).
- [ ] **The security model** for operators: what each component trusts, what
      must never be exposed (the manager), how secrets flow, and why webhooks
      are signed.
- [ ] **Self-hosting the workspace stores and the code-execution sandbox**:
      `WORKSPACES_BACKEND` (`gcs`, `local`), `CODE_EXECUTION_BACKEND`
      (`cloud-run-job`, `local-docker`), and what each needs.
- [ ] **Versioning and compatibility**: what a version promises (agent YAML,
      HTTP API, images, the database's shape), and how upgrades work.
- [x] Generate the reference pages in the docs' own CI: `website/build.sh`
      runs `make docs` before `mdbook build` (`docs.yml` on pull requests,
      `pages.yml` when publishing). See `OPEN_SOURCE_DECISIONS.md`.
- [ ] Contributing guide, code of conduct, issue templates, a security policy
      (where to report a vulnerability).

## 5. The CLI in production

The CLI already serves agents (`forma serve`), with the runtime from
`runtime.yaml`, conversations in files, and hot reloading. For a small
installation — one machine, a handful of agents — it could be all that is
needed. What stands in the way:

- [ ] **The address it listens on.** CLI builds bind `127.0.0.1` (compile-time,
      `get_host_url` in `src/server/runtime/mod.rs`), production builds
      `0.0.0.0`. Make it a flag or variable (e.g., `HOST`), defaulting to
      `localhost`. Note that `localhost` may resolve to `::1` only, which some
      tools (and Docker port forwarding) do not expect: listen on both, or say
      which.
- [ ] **Local workspaces and code execution.** The `local` workspace store
      cannot sign URLs, so code execution — which hands the sandbox signed
      URLs — does not work with it, even though a `local-docker` sandbox
      exists. Options: the CLI serves the local store itself, with URLs signed
      by an HMAC key it holds, reachable from the sandbox's container; or the
      `local-docker` sandbox mounts the files instead. The first keeps "any
      backend with any store" true.
- [ ] **FileDB under load.** One process, files on disk: check concurrent
      requests (two runs, the claim on a session, indexes rewritten at once),
      and document its limits. (`forma serve` can use `storage: mongo` since
      2026-10-22.)
- [ ] **API keys.** Since 2026-10-22, production servers read `runtime.yaml`
      too, so `no_api_key: true` turns the check off in both. That is explicit,
      not by accident; but consider refusing it in production builds unless
      something says the server sits behind an authenticating proxy.
- [ ] **Hot reloading** stays a development feature: a production mode that
      turns it off (and stops watching files).
- [ ] **Converging `forma serve` and the production binary.** Two ways to serve
      one agent, with different defaults, is confusing. Since 2026-10-22 both
      read the same `runtime.yaml` (client, storage, workspaces, sandbox), so
      what is left is the address and hot reloading. Ideally the CLI's `serve`
      *is* the production server, with development extras behind `forma dev`.
- [ ] A service file (systemd) and a `docker compose` example for "one machine".

## Before publishing anything

Clonsult and Forma live in one repository. Before Forma is public:

- [ ] **Extract it** with its history (`git filter-repo --path forma/`), or start
      a new history. Either way, **scan the history for secrets** first (e.g.,
      `gitleaks`): `.env` files, keys in Makefiles, test credentials. A key
      ever committed is a key to rotate, not to delete.
- [ ] **Remove Clonsult from Forma**:
      - `src/cli/init_files/agent-dev-guidelines.md` has a section on
        Clonsult's internal practices (3.a), pointing at
        `knowledge/ai-agent-hax/`, which ships with every `forma init`;
      - `agent/src/tool/code_execution/sandbox.rs` uses a `clonsult-dev` job
        path in a test;
      - `build_cli.yaml` (an `enterprise-template` project);
      - grep for `clonsult` again before publishing.
- [x] **Fix the README**: its "Building runtimes" section is now "Production
      servers", with `runtime.yaml`; the documentation is at
      `germolinal.github.io/forma/docs` everywhere.
- [ ] **Licences of dependencies**: `cargo deny check licenses`, so MIT is not
      contradicted by something copyleft.
- [ ] **Google-specific tools.** `google-knowledgebase`, `google-workspace`,
      `google-agent-search`, `google-gen-image`, the GCS workspace store and the
      Cloud Run sandbox are fine to ship, but an open-source user without
      Google Cloud needs an alternative for each that matters (S3-compatible
      storage, a local sandbox that works).
- [ ] **The TypeScript SDK**: a `package.json`, a build, and npm, if it is meant
      for others.
- [x] **CI on GitHub Actions** for the public repository: tests (with a MongoDB
      service container, as `make test` expects), clippy, fmt, the docs build.
      Written in `forma-repo-root/.github/workflows/` (the future repository's
      root); not run yet. A website (`website/`) publishes the docs to GitHub
      Pages. Choices made along the way: `OPEN_SOURCE_DECISIONS.md`.

## Open questions

- Open source at all, or source-available (e.g., BSL), given that Clonsult is a
  hosted Forma? MIT allows anyone to run a competing platform.
- Does Clonsult build on the public images once they exist (and so test them
  every day), or keep its own pipeline?
- Where datasets and eval results live for open-source users: files (the
  default) and Phoenix are supported; anything else?
- What is supported, and what is "it works on our machines": Windows, ARM
  servers, databases other than MongoDB and Firestore's MongoDB API.

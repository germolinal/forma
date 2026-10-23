# Forma as open source

Forma is released on its own, outside Clonsult: `germolinal/forma` is public,
MIT-licensed, and v0.2.1 is out. Started 2026-10-21; tidied 2026-10-23. This
file tracks the goals and what is left; how things were set up, and why, is in
`OPEN_SOURCE_DECISIONS.md`.

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

## Done

- **Repository**: extracted from Clonsult's, history scanned for secrets,
  public. Nothing in it mentions Clonsult but these notes.
- **Licence**: MIT, "Copyright (c) 2025-2026 German Molina", in `LICENSE` at
  the root; the name and logo in `TRADEMARKS.md`.
- **One production binary per tenancy** (2026-10-22): every client and storage
  is compiled in, and `runtime.yaml` chooses. Only `multi-tenant` and
  `hot-reload` remain features.
- **Releases** (`release.yml`, on `v*` tags): the CLI for Linux and macOS, x86_64
  and aarch64, on a GitHub Release with `SHA256SUMS`; five multi-arch images on
  GHCR (`forma`, `forma-multi-tenant`, `forma-manager`, `forma-cli`,
  `forma-sandbox`), tagged `X.Y.Z`, `X.Y` and `latest`, with SBOM and
  provenance, signed with cosign. The tag must match `forma --version`.
- **The image contract** is documented in `docs/src/how-to/deployment.md`
  ("The images"), with the two-step `Dockerfile` that `forma init` creates. The
  server images' default command is `-t otel`.
- **Install from the release**: the getting-started page, the README, the
  deployment guide and the website; `cargo install` is the alternative.
- **CI on GitHub Actions**: fmt, clippy, tests (MongoDB and Phoenix as service
  containers), the six builds, the docs; `cargo deny` and `cargo audit`.
- **The docs' reference pages** are generated in CI (`website/build.sh` runs
  `make docs` before `mdbook build`).

## Left to do

### Releases

- [ ] **The `forma` image was not published for v0.2.1**: its "Publish" job
      failed (`ghcr.io/germolinal/forma@sha256:b551…: not found`) while the other
      four succeeded. Re-run it, and find out why before the next tag.
- [ ] macOS signing and notarisation (needs an Apple Developer account; until
      then a browser-downloaded binary needs
      `xattr -d com.apple.quarantine`).
- [ ] An install script, or cargo-dist and a Homebrew formula.
- [ ] Check what the CLI needs besides itself: it links against glibc ≥ 2.35
      (musl for Alpine?); Ollama for the starter agent; Docker for
      `forma init`'s Phoenix and for the `local-docker` sandbox.
- [ ] Docker build caching in CI (Rust layers would fill the 10 GB cache).

### CI/CD image

An image that runs in any pipeline, checks an agent, evaluates it, and fails
the job if it is not good enough: `forma expand -c` (the agent compiles), then
`forma eval` (already fails below `required_score`, unless `--no-fail`), and
out come the compiled `agent.yaml` and the evaluation report.

- [ ] Base it on `forma-cli`. Decide how evals reach their LLMs from CI: API
      keys as CI secrets, or workload identity for Vertex AI. The judge's model
      too.
- [ ] Reports a CI system can show: a Markdown summary (GitHub Actions job
      summary), and JUnit XML so each failed metric shows as a failed test.
- [ ] Make evaluation cost visible, and capped: an eval run on every push can
      cost real money. Maybe a `--max-cost` or a sample size.
- [ ] Datasets: where they live in CI (the repository, or `forma dataset-*` from
      Phoenix), and whether they are versioned with the agent.
- [ ] A GitHub Action (`germolinal/forma-action`?) wrapping the image, and a
      ready workflow: check and eval on every pull request; build the agent's
      image on top of `forma` and deploy on merge. `cloud-deployment.md` is the
      deploy half, written for people.
- [ ] Non-determinism: the same agent scores differently run to run. Document
      how to set `required_score` with that in mind (repetitions, margins).

### Documentation

`docs` is for people who build agents in depth, and who run Forma themselves.
Covered today: single-tenant deployment (binary and container), the cloud
walk-through (Cloud Run), the HTTP API, WhatsApp and Telegram, evaluations,
observability, human in the loop.

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
      Clonsult's `back/` is the reference implementation; a minimal proxy
      example (a few hundred lines, any language) would be worth more than
      prose.
- [ ] **The manager's API**, complete (`deployment.md` has a table, without the
      bodies).
- [ ] **The security model** for operators: what each component trusts, what
      must never be exposed (the manager), how secrets flow, and why webhooks
      are signed.
- [ ] **Self-hosting the workspace stores and the code-execution sandbox**:
      `workspaces` (`gcs`, `local`) and `code_execution` (`cloud-run-job`,
      `local-docker`) in `runtime.yaml`, and what each needs.
- [ ] **Versioning and compatibility**: what a version promises (agent YAML,
      HTTP API, images, the database's shape), and how upgrades work.
- [ ] **Project files**: `SECURITY.md` (where to report a vulnerability),
      a contributing guide (contributions come in under MIT), a code of
      conduct, issue templates.
- [ ] Examples: `forma/README.md` links `github.com/ai-forma/forma-examples`,
      which does not exist. Move the examples here, or drop the link.

### The CLI in production

The CLI already serves agents (`forma serve`), with the runtime from
`runtime.yaml`, conversations in files or MongoDB, and hot reloading. For one
machine and a handful of agents it could be all that is needed. What stands in
the way:

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
      and document its limits.
- [ ] **API keys.** `no_api_key: true` turns the check off in production too.
      Explicit, not by accident; but consider refusing it in production builds
      unless something says the server sits behind an authenticating proxy.
- [ ] **Hot reloading** stays a development feature: a production mode that
      turns it off (and stops watching files).
- [ ] **Converging `forma serve` and the production binary.** Both read the
      same `runtime.yaml`, so what is left is the address and hot reloading.
      Ideally the CLI's `serve` *is* the production server, with development
      extras behind `forma dev`.
- [ ] A service file (systemd) and a `docker compose` example for "one machine".

### Beyond Google Cloud

- [ ] `google-knowledgebase`, `google-workspace`, `google-agent-search`,
      `google-gen-image`, the GCS workspace store and the Cloud Run sandbox
      are fine to ship, but a user without Google Cloud needs an alternative
      for each that matters (S3-compatible storage, a local sandbox that
      works: see "Local workspaces and code execution" above).
- [ ] **The TypeScript SDK**: a `package.json`, a build, and npm, if it is meant
      for others. Today it has none, and Clonsult's `front/` uses it by path.

## Open questions

- Does Clonsult build on the public images (and so test them every day), or
  keep its own pipeline? If it moves, `Dockerfile.tester` and Clonsult's Cloud
  Build tests of Forma can go.
- Where datasets and eval results live for open-source users: files (the
  default) and Phoenix are supported; anything else?
- What is supported, and what is "it works on our machines": Windows (paths,
  file watching and the FileDB have never been tried there), musl, databases
  other than MongoDB and Firestore's MongoDB API.

# Forma's own repository: decisions, made and pending

Started 2026-10-22, while setting up CI, the website and the logo for Forma's
own repository (see `OPEN_SOURCE.md`), and while moving the runtime's choices
from Cargo features into `runtime.yaml`. What is still to do is in
`OPEN_SOURCE.md`.

## Decided

- **Owner**: Germán Molina, GitHub user `germolinal`. Repository
  `germolinal/forma`; website <https://germolinal.github.io/forma/>, docs at
  <https://germolinal.github.io/forma/docs/>.
- **Layout**:

  ```
  <repo root>/
    .github/            workflows/ (ci, docs, pages, release, dependencies), dependabot.yml
    README.md, LICENSE, TRADEMARKS.md
    forma/              as it is today (the Cargo workspace)
    docs/         the mdBook
    website/         the website (Astro, Tailwind)
  ```

  `forma/` stays a subdirectory: `make docs` writes to `../docs/src`, and
  the three directories keep the paths they have today.
- **Licence (2026-10-23): MIT**, "Copyright (c) 2025-2026 German Molina", in
  `LICENSE` at the root. The release copies it into the CLI's archives, the
  images carry `org.opencontainers.image.licenses`, and every crate says
  `license = "MIT"`. Considered: AGPL-3.0, Apache-2.0, source-available
  (FSL, BSL). Abuse is handled through the name, not the licence:
  `TRADEMARKS.md` (forks rename; the name never on anything harmful), as Deno
  and Node.js do. Contributions come in under MIT; no CLA.
- **Releases are `v*` tags.** A tag publishes the website and the docs, both
  from the tag (`pages.yml`), and the binaries and images (`release.yml`; see
  "Releases", below).
- **Website**: static Astro with Tailwind 4, the same stack as Clonsult's
  `website/`. shadcn's token names and button recipe, no shadcn library (the
  page needs no interactive component yet).
- **Coverage** (tarpaulin, Codecov) is out for now. `codecov.yaml` and the
  `cov` target in the Makefile are still there; the README badge is gone.
- **Builds are few; the rest is `runtime.yaml`** (2026-10-22). See
  [Builds and the runtime file](#builds-and-the-runtime-file).

## Builds and the runtime file

Cargo features now decide only what a build *is*:

| Feature        | Kept because                                                          |
| -------------- | --------------------------------------------------------------------- |
| `cli`          | The development commands, the chat UI, and their defaults (`127.0.0.1`) |
| `multi-tenant` | It changes what the server trusts (`X-Deployment-ID`, the user headers from the proxy): a misread setting must not turn it on |
| `hot-reload`   | Unused in production today; kept for agents that update themselves    |
| `api`          | The definitions and the manager                                       |

Gone: `run-*` (the clients) and `db-*` (the storages). Every client and
storage is compiled into every build: the binary with everything is the same
size as one with a single runtime (≈12 MB gzipped either way, measured), and
choosing at start costs a `match` per call.

`runtime.yaml` (the `FormaRuntime` struct) is read by `forma serve`,
`forma dev`, `forma eval` and production servers (`-r`, default
`runtime.yaml`):

```yaml
client: sse                  # sync, sse, ai-sdk-v5, whatsapp, telegram
max_history_messages: 20     # the client's own options sit next to it
storage: mongo               # mongo, file, none
workspaces:                  # optional
  store: gcs                 # or `local`, with `dir`
code_execution:              # optional
  sandbox: cloud-run-job     # or `local-docker`, with `image`
```

- **`storage` is required**, with no default: a production server must not
  quietly keep conversations in a container's disk, nor drop them. A
  multi-tenant server refuses `none` at start (it reads its agents from
  storage).
- **`workspaces` and `code_execution` are optional.** Without them, the tools
  that need them fail when called, with a message naming `runtime.yaml` (the
  existing rule: "an agent loads anywhere"). The implicit defaults are gone
  (no `WORKSPACES_BACKEND` meant `gcs`; no `CODE_EXECUTION_BACKEND` meant
  `cloud-run-job`): a server that wants them says so.
- **Checked at start**: a runtime file that sets code execution with local
  workspaces (which cannot sign URLs), or with none, is refused.
- **Choices in the file, secrets and per-environment names in the
  environment.** `SESSIONS_DB_URL`, `WORKSPACES_BUCKET`, `CODE_EXECUTION_JOB`,
  `GOOGLE_CLOUD_*` and the clients' tokens stay environment variables: an image
  promoted from dev to prod must not carry them. `WORKSPACES_BACKEND`,
  `WORKSPACES_DIR`, `CODE_EXECUTION_BACKEND` and `CODE_EXECUTION_IMAGE` are
  gone. Environment-variable alternatives to the file are left for later, as
  the settings will grow.
- **How tools learn the choices**: `agent::tool::host`, a process-wide value
  installed once at start (`FormaRuntime::install`), which replaced the
  environment variables the tools read before (also process-wide). Where
  nothing is installed (`forma-manager`, `forma expand`), the checks that
  depend on the store are skipped, and left to the server that runs the agent.
- **`forma dev`** reads the file but its `client` (it serves SSE to its chat
  UI). **`forma eval`** reads it for workspaces and code execution only; evals
  keep conversations nowhere, as before. **`forma-manager`** uses MongoDB.
- **`no_api_key: true` now works in production too**, since production reads
  the file. Explicit, so not "open by accident"; `OPEN_SOURCE.md` (section 5)
  asks whether to refuse it there.
- **YAML shape**: the client is flattened into the top level, so existing
  files only gained `storage`. Typos are still refused (the client's options
  through its `deny_unknown_fields`). Variants with nothing to set
  (`store: gcs`, `sandbox: cloud-run-job`) are empty structs rather than unit
  variants, because serde lets a unit variant of a tagged enum accept, and
  ignore, any key (e.g., a `job:` that does nothing).
- **Names**: the old `FormaRuntime` enum is now `Client`; the code-execution
  setting's type is `Sandbox`, as the `code` tool's reference page is already
  called `CodeExecution`.
- **Docs generator** (`derive/`): documents `#[serde(flatten)]` fields ("the
  fields of `Client`, at this level"), reads every argument of a
  `#[serde(...)]` attribute (it read only the first, so
  `tag = "...", rename_all = "..."` lost its renaming), and leaves the
  "Full Specification" out of types with nothing to set.
- Found along the way, and fixed: `multi-tenant` with `hot-reload` failed
  clippy (an unused import; the old matrix never built that pair); 11
  `#[allow(dead_code)]` that only existed because some builds left adapters
  out; an unused test import; `fetch_update` (deprecated in Rust 1.99) in the
  bench.
- `featured-agents/*/runtime.yaml` (Clonsult) now say `storage: file`, and
  `workspaces: gcs` / `code_execution: cloud-run-job` where their agents use
  them: what they got implicitly before.

### Clonsult's side

Done (2026-10-22):

- **One image for every channel.** `cicd/deploy-channel.yaml` builds
  `forma-channel` (`Dockerfile.channel`, `FEATURES=multi-tenant`) once per
  push, and deploys it to every channel's service: one trigger per build
  target (`dep-channels-<target>-trigger`), no longer one per channel.
  Promotion keeps a trigger per channel, each deploying the shared image
  (`_IMAGE_NAME` in `promote-image.yaml`).
- **Each channel's runtime file** is `infra/channels/<channel>.yaml` (client
  `sse`, `whatsapp` or `telegram`; `storage: mongo`; `workspaces: gcs`;
  `code_execution: cloud-run-job`). Terraform puts it in a secret,
  `<channel>-channel-runtime`, mounted at `/config` (Cloud Run mounts files
  only from secrets). `Dockerfile.channel`'s command reads
  `/config/runtime.yaml`: the path is the image's contract, so Terraform only
  mounts the file, and the self-hosted images (which copy the command) need
  nothing else. The service mounts the version Terraform wrote, so editing a
  file and applying is a new revision, with no build. The root `Makefile`
  runs the channels locally with the same files.
- **Self-hosted releases** publish one `channel` artifact instead of
  `channel-web`, `channel-whatsapp` and `channel-telegram` (they would have held
  the same binary). The self-host pipeline deploys it to every channel's
  service, and its module gives each its runtime file, inline (only its `.tf`
  files ship), without code execution. A licence can only cover what a release
  publishes (`releaseArtifactNames` in `back/selfhost.go`): `backend`,
  `forma-manager`, `channel`, `collector`, `terraform`. No licence existed yet,
  so none needed migrating.
- `Dockerfile.single_tenant` (identical to `Dockerfile.channel` without
  features) and its `build_single_tenant.yaml` are gone, with the root
  `Makefile`'s `build_enterprise_runtime`.

To know before applying:

- **Order**: apply Terraform first (secrets, the new trigger, the volume on
  each service): the old per-channel images ignore the mounted file. Then the
  next push to `main` builds `forma-channel`, whose command reads it, and
  deploys it to all three. A build that lands before the apply fails to start
  (no file), and Cloud Run keeps the previous revision serving; re-run it after
  applying. A release tag promotes the shared image only once dev runs it:
  `resolve-image.sh`'s fallback looks for `forma-channel@` in dev's revisions.
- Applying destroys the three `dep-<channel>-channel-dev-trigger` and creates
  `dep-channels-dev-trigger`; the old per-channel images stay in the registry.
- **Self-host**: a release built from this commit expects its module, which
  mounts the runtime file the released command reads. An install has to apply
  the module of the version it upgrades to (which the release carries).
- Pending: `forma-local` (branch `swift-client`) used `FormaDBClient` as
  `FileDB` through `db-file`: it is `MultiDB` now (`MultiDB::new_file()`).

## Extraction

Done: the repository was extracted from Clonsult's with `git filter-repo`
(`forma/`, `docs/`, `website/`, and `forma-repo-root/` renamed to the root),
its history scanned for secrets, and made public.

- **Dependabot**: version updates come from `dependabot.yml`; alerts and
  security updates are turned on in Settings → Code security. Grouped weekly
  updates keep its pull requests to a few a week.

## CI (`.github/workflows/`)

- `ci.yml` mirrors Cloud Build's `cicd/test-forma.yaml` and `make pr_checks`:
  fmt, clippy, integration tests compile, `make test` (the CLI's build and
  production's, with MongoDB and Phoenix as service containers) and
  `make check-features` (check and clippy of the six builds Forma ships:
  production with and without `multi-tenant` and `hot-reload`, the manager,
  the CLI). Runs after fmt and clippy pass.
- `dependencies.yml`: `cargo deny check bans licenses sources` when
  dependencies change, and `cargo audit` (RustSec) on those changes and daily.
  Vulnerabilities are left to `cargo audit` so the two don't report the same
  advisory twice.
- `forma/deny.toml` allows permissive licences plus MPL-2.0, all of which can
  ship inside MIT-licensed Forma.
  Duplicate crate versions warn, they don't fail. CC0-1.0 is allowed too
  (`notify`, `tiny-keccak`).
- Every crate in the workspace has `publish = false` and `license = "MIT"`:
  none is on crates.io, so cargo deny skips them and their path dependencies.
  Publishing a crate later means removing `publish = false`.
- `cargo audit` passes since 2026-10-22, when `Cargo.lock` was updated to fix
  10 advisories (`rustls`, `rustls-webpki`, `webpki-roots`,
  `webpki-root-certs`, `quinn-proto`). Two "unmaintained" warnings remain
  (`proc-macro-error`, `unsafe-libyaml`); they don't fail the check.
- Rust is pinned to 1.99.1 in each workflow (`RUST_TOOLCHAIN`), like
  `Dockerfile.tester`. Alternative: a `rust-toolchain.toml` in `forma/`, which
  also pins it on people's machines. `Dockerfile.tester` is not needed by
  GitHub Actions: keep it only while Clonsult's Cloud Build tests Forma.
- Node is pinned by `website/.nvmrc` (24; Astro 7 needs ≥ 22.23).
- Caches are saved from `main` only (the repository's cache quota is 10 GB).
- `arizephoenix/phoenix:latest` and `mongo:8` are not pinned to digests.
- Path filters: CI runs only when `forma/**` changes. If CI becomes a required
  check in branch protection, path-filtered workflows block docs-only pull
  requests; then drop the filters or add a job that always passes.

## Website and documentation

- `pages.yml` runs on `v*` tags, and by hand (it publishes the latest tag
  again). Changes on `main` reach the site at the next release. If the site
  ever needs to change between releases (a typo on the landing page), add a
  `main` trigger that builds the site from `main` and the docs from the latest
  tag.
- The site is served under `/forma/`. `SITE_URL` and `BASE_PATH` come from
  `actions/configure-pages`, so a custom domain needs only the Pages setting
  (and a `website/public/CNAME`).
- Fonts are self-hosted (`@fontsource-variable`): Inter, Space Grotesk,
  JetBrains Mono. No third-party requests.
- The "Get started" section installs from the latest release.
- `docs/src/intro.md` embeds a YouTube video that shows the old logo.

## Logo

- An "F" of rounded bars on a dark tile, its middle arm ending in an amber dot
  (one definition branching out to channels). In `website/public/`
  (`mark.svg` = `favicon.svg`, `logo.svg`, `apple-touch-icon.png`) and
  `website/src/components/brand/Mark.astro`; copies in
  `docs/src/forma-logo.svg`, `docs/theme/favicon.{svg,png}` and
  `forma/img/forma-logo.svg`.
- The wordmark in `logo.svg` is live text (Space Grotesk, falling back to the
  system's sans-serif), so it renders slightly differently where the font is
  missing (e.g. GitHub's README). Convert it to paths once the design is
  settled.
- Colours: ink `#141922`, paper `#F4F1EA`, amber `#F2A93B`.

## Releases (`.github/workflows/release.yml`, written 2026-10-22, first run v0.2.1)

- A `v*` tag publishes the CLI's binaries to a GitHub Release
  (`forma-<target>.tar.gz`, no version in the name so
  `releases/latest/download/...` is stable, plus `SHA256SUMS`), and five
  multi-arch images to GHCR (`ghcr.io/germolinal/`): `forma`,
  `forma-multi-tenant`, `forma-manager`, `forma-cli`, `forma-sandbox`, tagged
  `X.Y.Z`, `X.Y` and `latest` (not for prereleases), with SBOM and provenance.
- The tag must equal `version` in `forma/Cargo.toml` (now `0.2.1`), which is
  what `forma --version` reports: bump it in the release commit.
- Targets: Linux x86_64/aarch64 (built on Ubuntu 22.04, glibc ≥ 2.35), macOS
  aarch64/x86_64. Native runners only (`ubuntu-22.04-arm`, `ubuntu-24.04-arm`,
  `macos-15-intel`): emulating arm64 makes a Rust build take hours.
- Images are signed with cosign (keyless), only from a public repository:
  Rekor would publish a private repository's name.
- The getting-started page, the README, the deployment guide and the site
  install from the release (download, or the images); `cargo install` stays
  as the alternative. Every workflow passes `actionlint` 1.7.12.
- v0.2.1 published everything but the `forma` image; what is pending is in
  `OPEN_SOURCE.md` ("Releases").

## OpenAPI (2026-10-23)

The HTTP APIs are described in OpenAPI 3.1, generated from the code, so that
people (and their coding agents) can build on them: `forma openapi <api>`
prints one, and `make docs` writes all six to `docs/src/api/openapi/`
(git-ignored, like the reference pages), which `docs/src/api/explorer.html`
shows with Swagger UI (`swagger-ui-dist`, pinned). The prose is in
`docs/src/api/`.

- **`utoipa` 6 and `utoipa-axum`**, behind an `openapi` feature (in `forma`,
  `protocols` and `genai_client`), which `api` turns on: the CLI and the
  manager have it, production servers do not. `aide` was the alternative (it
  infers from axum's types), but it is on `schemars` 0.9 and Forma on 1.2.
  The types that travel derive `ToSchema` under the feature; their doc
  comments are the schemas' descriptions, so `missing_docs` keeps them
  described.
- **One description per client** (`sync`, `sse`, `ai-sdk-v5`, `whatsapp`,
  `telegram`), plus the manager's: a runtime serves the same paths to every
  client, but they take and return different things.
- **The manager's routes and description are one declaration**
  (`OpenApiRouter`, `src/manager.rs`, where the manager moved from
  `bin/forma-manager.rs`). The runtime's handlers are generic methods, which
  `utoipa` cannot describe in place, so `src/server/openapi.rs` describes each
  client's operations, on the paths the router uses; a test sends every
  operation described to the client's real router, and fails on a 404 or 405.
- **The schemas are tested against what serde sends**: real events, answers,
  sessions and usage are checked against the generated schemas (`jsonschema`),
  and malformed ones must be refused, so a schema that drifts from the type
  fails the tests.
- **OpenAPI 3.1, not 3.2**: 3.2's `itemSchema` describes the items of a
  stream, but viewers and generators do not all read 3.2 yet. A stream is
  `text/event-stream` with the schema of one `data:` line (`SseLine`,
  `AiSdkV5Line`), and the docs explain the rest.
- `AiSdkV5Event` lost its hand-written `Serialize`: its one `Data { name, data }`
  variant (serialized as `data-<name>`) is now a variant per event
  (`data-still-suspended`, `data-grounding-score`, `data-reference`), which
  serde derives, and a schema can describe. The JSON sent is unchanged.
- `GenericFormaMessage<DATE>` and `GenericSessionData<ID, DATE>` describe their
  dates as strings; the message is named `FormaMessage` in the description
  (`utoipa` would call it `FormaMessage_String`).
- Agent definitions are plain objects in the manager's description: their
  schema is the reference documentation's (`FormaDocs`). Deriving `ToSchema`
  over every definition type would put it in the description too.
- Found along the way, and fixed: `McpToolDefinition::auth` documented two
  strategies (`oauth-discovery`, `client-credentials`) that do not exist; the
  deployment guide's ids were UUIDs, which the manager refuses (they are
  MongoDB ids).

## Failures, ownership and usage (2026-10-23)

- **A panic is reported, never answered as a success.** A run's task is
  unwound (`catch_unwind` in `call_agent`) into an `error` event, and a run
  whose channel closes with neither `finish` nor `error` is an error too,
  both streamed and in JSON answers. Before, a panic (a missing
  `OPENAI_API_KEY`) ended `sse` streams with no `error`, and `sync` answered
  `completed` with an empty message. The panics that input, configuration or
  providers can trigger now return errors: a missing API key, an answer the
  provider sends that does not parse, Vertex AI parts Forma does not use
  (kept as text) or a candidate without content (a blocked answer), and
  `FORMA_ENCRYPTION_KEY` (missing, malformed; and two first uses at once,
  which panicked on "already initialized"). `GenaiClientTrait::stream`
  returns `String` errors, as `generate` does.
- **Image generation reports its provider as OpenTelemetry names it**
  (`gcp.vertex_ai`), as LLM calls do: it was `vertex-ai`, a second price key
  for the same provider. `provider` names the API spoken, not who bills: all
  OpenAI-compatible endpoints are `openai` (`docs/src/api/usage-and-billing.md`).
- **`DELETE /deployments` checks ownership** (`X-User-ID`), as `PUT` does:
  someone else's deployment is a `404`. `FormaDB::remove_agent_deployment`
  takes the user. Callers must now send `X-User-ID`.
- **JSON answers carry `usage`** (`sync`, and `ai-sdk-v5`'s `/v1/resume`), next
  to `status` and `data`: additive.
- **The AI SDK's own `file` parts are read** (`{type: file, mediaType, url}`),
  as the AI SDK sends attachments, and files go back as such in
  `/v1/resume`'s answer. Pictures may be links; other files must be `data:`
  URLs (Forma does not fetch them), or the message is refused with a `400`.
- **Usage names the host as well** (`server_address`, OpenTelemetry's
  `server.address`), and is grouped by `(provider, server_address, model)`:
  `provider` is the API spoken, so every OpenAI-compatible server is
  `openai`, and the host is what tells OpenAI from OpenRouter for a bill.
  Additive (empty from older runtimes). The LLM metrics carry it as a label,
  and LLM spans now record the host as `server.address`, as the convention
  says (they recorded the full URL, through an `unwrap` that an empty
  endpoint would have panicked on).

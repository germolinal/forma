# Forma's own repository: decisions, made and pending

Started 2026-10-22, while setting up CI, the website and the logo for Forma's
own repository (see `OPEN_SOURCE.md`), and while moving the runtime's choices
from Cargo features into `runtime.yaml`. Licences, binaries and Docker images
are left for their own discussions.

## Decided

- **Owner**: Germán Molina, GitHub user `germolinal`. Repository
  `germolinal/forma`; website <https://germolinal.github.io/forma/>, docs at
  <https://germolinal.github.io/forma/docs/>.
- **Layout**:

  ```
  <repo root>/
    .github/            workflows/ (ci, docs, pages, dependencies), dependabot.yml
    README.md
    forma/              as it is today (the Cargo workspace)
    docs/         the mdBook
    website/         the website (Astro, Tailwind)
  ```

  `forma/` stays a subdirectory: `make docs` writes to `../docs/src`, and
  the three directories keep the paths they have today.
- **Releases are `v*` tags.** A tag publishes the website and the docs, both
  from the tag (`pages.yml`); binaries and images will follow the same tag.
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

The root files live in `forma-repo-root/` in Clonsult's repository, where
GitHub ignores them (workflows only run from the root's `.github/`, and
Clonsult's repository must not run them). On extraction:

```sh
git filter-repo --path forma/ --path docs/ --path website/ \
  --path forma-repo-root/ --path-rename forma-repo-root/:
```

Scan the history for secrets first (`OPEN_SOURCE.md`, "Before publishing").

## While the repository is private

- **GitHub Pages needs a paid plan for a private repository** (Pro, Team or
  Enterprise; Free publishes Pages from public repositories only). Either
  GitHub Pro, or no website until the repository is public: `pages.yml` then
  fails at `configure-pages`, nothing else breaks.
- **Actions minutes**: private repositories get 2,000 minutes a month on
  Free (3,000 on Pro); public ones are free. A pull request that touches
  `forma/` runs four Rust jobs (fmt, clippy, tests, builds); the 20-job
  feature matrix is gone with the features.
- **Dependabot** is free for private repositories too: alerts, security
  updates and version updates. In a private repository, turn on alerts and
  security updates in Settings → Code security. Version updates
  (`dependabot.yml`) work as soon as the file is there. Its pull requests run
  the workflows like any other, so they spend minutes too: grouped weekly
  updates keep that to a few pull requests a week.

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
- `forma/deny.toml` allows permissive licences plus MPL-2.0. **Part of the
  licence discussion**: if Forma's licence changes, this list changes with it.
  Duplicate crate versions warn, they don't fail. CC0-1.0 is allowed too
  (`notify`, `tiny-keccak`).
- Every crate in the workspace has `publish = false`: none is on crates.io,
  and cargo deny then skips their (missing) `license` field and their path
  dependencies. Publishing a crate later means removing it, and giving that
  crate a `license`.
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
- The "Try it" section installs from source (`cargo install`), until binaries
  exist.
- `docs/src/intro.md` embeds a YouTube video that shows the old logo.
- `forma/README.md` links `github.com/ai-forma/forma-examples`, which won't
  exist under `germolinal`. Move the examples into this repository, or drop
  the link.
- `forma/LICENSE` says "Copyright (c) 2025 Forma AI": part of the licence
  discussion.

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

## Releases (`.github/workflows/release.yml`, written 2026-10-22, not run yet)

- A `v*` tag publishes the CLI's binaries to a GitHub Release
  (`forma-<target>.tar.gz`, no version in the name so
  `releases/latest/download/...` is stable, plus `SHA256SUMS`), and four
  multi-arch images to GHCR: `forma`, `forma-multi-tenant`, `forma-manager`,
  `forma-cli`, tagged `X.Y.Z`, `X.Y` and `latest` (not for prereleases), with
  SBOM and provenance.
- The tag must equal `version` in `forma/Cargo.toml` (now `0.2.0`): bump it in
  the release commit.
- Targets: Linux x86_64/aarch64 (built on Ubuntu 22.04, glibc ≥ 2.35), macOS
  aarch64/x86_64. Native runners only (`ubuntu-22.04-arm`, `ubuntu-24.04-arm`,
  `macos-15-intel`): check they are available to a private repository.
- Images are signed with cosign (keyless) only once the repository is public:
  Rekor would publish a private repository's name.
- The getting-started page, the README, the deployment guide and the site
  now install from the release (download, or the images); `cargo install`
  stays as the alternative. Every workflow passes `actionlint` 1.7.12.
- Pending: macOS signing and notarisation (needs an Apple Developer account;
  until then a browser-downloaded binary needs `xattr -d
  com.apple.quarantine`), Windows, an install script, and Docker build caching
  (Rust layers would fill the 10 GB cache).

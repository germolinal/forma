![Forma](website/public/logo.svg)

# Forma

An AI agent runtime: write the agent once, in YAML, and serve it over HTTP, to
web apps, on WhatsApp or on Telegram.

| Directory | What it is |
| --- | --- |
| `forma/` | The runtime, the CLI and the manager (Rust) |
| `docs/` | The documentation (mdBook) |
| `website/` | The website (Astro), published to GitHub Pages with the docs under `docs/` |

See `forma/README.md` to build it, and the
[documentation](https://germolinal.github.io/forma/docs/) to use it.

Releases are `v*` tags: a tag publishes the website and the documentation
(and, later, the binaries and the images).

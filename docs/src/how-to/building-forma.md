# Building Forma from source

Forma is written in [Rust](https://www.rust-lang.org/tools/install), and the [Forma repository](https://github.com/germolinal/forma) contains everything: the CLI, the production runtimes, the manager, and the code that generates the [Reference Documentation](../reference_docs/auto-agent.md) in this book.

## The binaries

| Binary          | Built with                                              | Purpose                                                                 |
| --------------- | ------------------------------------------------------- | ----------------------------------------------------------------------- |
| `forma` (CLI)   | `cargo install --path . --bin forma`                    | Developing and evaluating agents (`forma dev`, `forma eval`, etc.)      |
| `forma` (server)| `cargo build --release --bin forma --no-default-features --features ...` | Serving agents in production. Check the [Deployment](./deployment.md) section |
| `forma-manager` | `cargo build --release --bin forma-manager --no-default-features --features ...,api` | Deploying agents to a multi-tenant server                |

The repository also includes Dockerfiles for each of these:

| File                      | Produces                                                                                     |
| ------------------------- | -------------------------------------------------------------------------------------------- |
| `Dockerfile.cli`          | An image with the CLI at `/usr/local/bin/forma`, useful for running evaluations in CI/CD     |
| `Dockerfile.channel`      | A production server, with the features passed in the `FEATURES` build argument (none, or `multi-tenant`) |
| `Dockerfile.manager`      | The manager, with the `api` feature                                                          |

## The `Makefile`

The `Makefile` at the root of the repository has the commands used while developing Forma itself:

| Command                 | What it does                                                                                       |
| ----------------------- | -------------------------------------------------------------------------------------------------- |
| `make test`             | Runs every test, in every crate. Some tests need a MongoDB at `localhost:27017` (user `root`, password `example`) |
| `make integration_test` | Runs the tests that call real external services (e.g., LLM providers). They need credentials      |
| `make lint`             | Checks formatting (`cargo fmt`) and runs `cargo clippy`                                            |
| `make fix`              | Applies `clippy`'s suggestions and formats the code                                                |
| `make check-features`   | Compiles, and runs `clippy` on, every build Forma ships: production servers (with and without `multi-tenant` and `hot-reload`), the manager (`api`) and the CLI |
| `make pr_checks`        | Everything a pull request needs to pass: `check-features`, `lint` and the tests                    |
| `make cov`              | Measures test coverage, producing a `cov.html` report                                              |
| `make docs`             | Regenerates the Reference Documentation of this book (see below)                                   |
| `make check_dependencies` | Lists outdated dependencies                                                                      |

## The Reference Documentation is generated from the code

Every page under *Reference Documentation* in this book (the ones whose file names start with `auto-`) is written by a test called `print_docs::test::write_docs`, which extracts the documentation comments (`///`) of the Rust structures that define agents, nodes, tools, etc. This means that the place to fix or extend those pages is the code itself, not the Markdown files.

The test expects this book to be in a `docs` directory next to the `forma` repository:

```text
forma/       <- run `make docs` here
docs/
  src/
    SUMMARY.md
    reference_docs/
```

And it is run with:

```sh
# Within the forma repository
make docs
```

Which is the same as:

```sh
cargo test --package forma --lib --features cli -- print_docs::test::write_docs --exact --nocapture
```

These pages are not kept in `git` (`src/reference_docs` is listed in the book's `.gitignore`), so **`make docs` must run before `mdbook build`**: otherwise, every link in the *Reference Documentation* section will be broken.

The test overwrites every `auto-*.md` file, and rewrites the *Reference Documentation* section at the end of `SUMMARY.md`. Everything in `SUMMARY.md` *before* the `# Reference Documentation` title is kept as it is, so hand-written pages must always be listed above it.

> **📌 Note**: `make cov` and `make pr_checks` skip this test (`--skip write_docs`), so running them never modifies this book.

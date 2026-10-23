![Forma Logo](src/forma-logo.svg)

# Forma Documentation

An [mdBook](https://rust-lang.github.io/mdBook/), published with Forma's website
(`website/`) under `docs/`, at <https://germolinal.github.io/forma/docs/>.
It is published from each release (a `v*` tag): see
`.github/workflows/pages.yml`.

The *Reference Documentation* pages (`src/reference_docs`, git-ignored) are
generated from the Rust doc comments in `forma/`, which also rewrites the end of
`src/SUMMARY.md`. Generate them before building:

```sh
cd forma && make docs
cd ../docs && mdbook serve
```

Or build the whole website, docs included (needs Node; see
`website/README.md`):

```sh
cd website && npm install && ./build.sh
```

Pull requests build the docs (`.github/workflows/docs.yml`), with the reference
pages regenerated, so a doc comment that breaks the book fails there.

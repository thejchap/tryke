# contributing

## local development

run `maturin develop` to build the rust extension and install it into the active
virtualenv so you can import and test the python bindings locally.

## python tests

run `cargo run test` to test tryke's own python code, using tryke

## updating CLI docs

run `cargo run --bin generate-cli-docs --` after changing the Rust CLI definitions
or their help text. this rewrites `docs/cli.md` from the clap metadata in
`crates/tryke/src/cli.rs`.

pre-commit regenerates the file locally, and CI fails if the checked-in docs are
out of sync with the Rust code.

## updating snapshot tests

1. run `cargo test` — new or changed snapshots are written to
`crates/tryke/tests/snapshots/` as `.snap.new` files
2. review pending snapshots: `cargo insta review` (interactive) or accept all:
`cargo insta accept`
3. commit the `.snap` files alongside code changes

## manual release

1. bump version in both `crates/tryke/Cargo.toml` and `pyproject.toml`
to the same value (e.g. `0.2.0`)
2. commit: `git commit -am "release v0.2.0"`
3. create and push the tag: `git tag v0.2.0 && git push origin main --tags`
4. the `release` CI pipeline triggers automatically on the `v*` tag
5. monitor the workflow at `https://github.com/thejchap/tryke/actions`
6. once the `publish` job completes, verify on PyPI: `uv tool install tryke==0.2.0`

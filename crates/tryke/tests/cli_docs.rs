//! Drift check for the generated CLI reference at `docs/reference/cli.md`.
//!
//! Run `cargo run -p tryke_dev --bin generate-cli-docs --` (or commit through
//! prek, which runs the same generator) to regenerate after editing the doc
//! comments in `crates/tryke/src/cli.rs`.

use tryke::cli_docs::{docs_path, normalize_generated_markdown, render_cli_reference};

#[test]
fn cli_reference_is_up_to_date() {
    let expected = normalize_generated_markdown(&render_cli_reference());
    let path = docs_path();
    let actual = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    let actual = normalize_generated_markdown(&actual);
    assert_eq!(
        actual,
        expected,
        "{} is out of date; regenerate with `cargo run -p tryke_dev --bin generate-cli-docs --`",
        path.display()
    );
}

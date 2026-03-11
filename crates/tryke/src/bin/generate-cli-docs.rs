use std::fs;
use std::process::ExitCode;

use tryke::cli_docs::{docs_path, normalize_generated_markdown, render_cli_reference};

fn main() -> ExitCode {
    let check = std::env::args().skip(1).any(|arg| arg == "--check");
    let docs = normalize_generated_markdown(&render_cli_reference());
    let path = docs_path();

    if check {
        match fs::read_to_string(&path) {
            Ok(existing) if normalize_generated_markdown(&existing) == docs => ExitCode::SUCCESS,
            Ok(_) => {
                eprintln!(
                    "generated CLI docs are out of date: regenerate with `cargo run --bin generate-cli-docs --`"
                );
                ExitCode::FAILURE
            }
            Err(err) => {
                eprintln!("failed to read {}: {err}", path.display());
                ExitCode::FAILURE
            }
        }
    } else {
        if let Some(parent) = path.parent()
            && let Err(err) = fs::create_dir_all(parent)
        {
            eprintln!("failed to create {}: {err}", parent.display());
            return ExitCode::FAILURE;
        }
        if let Err(err) = fs::write(&path, docs) {
            eprintln!("failed to write {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
        ExitCode::SUCCESS
    }
}

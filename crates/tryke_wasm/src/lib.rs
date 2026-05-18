use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use tryke_reporter::Reporter;
use tryke_types::TestOutcome;
use wasm_bindgen::prelude::*;

#[derive(Deserialize)]
struct PlaygroundFile {
    filename: String,
    source: String,
}

#[derive(serde::Serialize)]
struct GraphEdge {
    from: PathBuf,
    to: PathBuf,
}

#[derive(serde::Serialize)]
struct MultiResult {
    files: Vec<FileResult>,
    edges: Vec<GraphEdge>,
}

#[derive(serde::Serialize)]
struct FileResult {
    path: PathBuf,
    discovered: tryke_types::DiscoveredFile,
}

/// Parse a single Python source file and return the discovery result
/// (tests, hooks, import candidates, dynamic-import flag) as a JS
/// object.
#[expect(clippy::missing_errors_doc)]
#[wasm_bindgen]
pub fn discover(source: &str, filename: &str) -> Result<JsValue, JsError> {
    let path = PathBuf::from(filename);
    let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let result = tryke_discovery::discover_file_from_source(&root, &[], &path, source);
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&e.to_string()))
}

/// Parse multiple Python source files and return a combined result
/// including resolved import graph edges.
#[expect(clippy::missing_errors_doc)]
#[wasm_bindgen]
pub fn discover_multi(files_json: &str) -> Result<JsValue, JsError> {
    let files: Vec<PlaygroundFile> =
        serde_json::from_str(files_json).map_err(|e| JsError::new(&e.to_string()))?;
    let root = PathBuf::from(".");

    let mut all_discovered: Vec<(PathBuf, tryke_types::DiscoveredFile)> = Vec::new();

    for file in &files {
        let path = PathBuf::from(&file.filename);
        let src_roots = vec![root.clone()];
        let result =
            tryke_discovery::discover_file_from_source(&root, &src_roots, &path, &file.source);
        all_discovered.push((path, result));
    }

    let file_set: HashSet<PathBuf> = files.iter().map(|f| root.join(&f.filename)).collect();

    let mut edges: Vec<GraphEdge> = Vec::new();
    for (path, disc) in &all_discovered {
        let resolved =
            tryke_discovery::resolve_import_candidate_groups(&disc.import_candidates, &file_set);
        for target in resolved {
            let target = target.strip_prefix(&root).unwrap_or(&target).to_path_buf();
            edges.push(GraphEdge {
                from: path.clone(),
                to: target,
            });
        }
    }

    let result = MultiResult {
        files: all_discovered
            .into_iter()
            .map(|(path, discovered)| FileResult { path, discovered })
            .collect(),
        edges,
    };

    serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&e.to_string()))
}

/// Pipe test results through a real tryke reporter and return the
/// rendered output string (with ANSI escape codes for terminal-style
/// reporters).
#[expect(clippy::missing_errors_doc)]
#[wasm_bindgen]
pub fn format_results(results_json: &str, reporter_name: &str) -> Result<String, JsError> {
    let results: Vec<tryke_types::TestResult> =
        serde_json::from_str(results_json).map_err(|e| JsError::new(&e.to_string()))?;

    let tests: Vec<tryke_types::TestItem> = results.iter().map(|r| r.test.clone()).collect();
    let summary = build_summary(&results);

    let output = match reporter_name {
        "text" => run_reporter(
            tryke_reporter::TextReporter::with_writer(Vec::new()),
            &tests,
            &results,
            &summary,
        ),
        "dot" => run_reporter(
            tryke_reporter::DotReporter::with_writer(Vec::new()),
            &tests,
            &results,
            &summary,
        ),
        "json" => run_reporter(
            tryke_reporter::JSONReporter::with_writer(Vec::new()),
            &tests,
            &results,
            &summary,
        ),
        "llm" => run_reporter(
            tryke_reporter::LlmReporter::with_writer(Vec::new()),
            &tests,
            &results,
            &summary,
        ),
        other => return Err(JsError::new(&format!("unknown reporter: {other}"))),
    };

    Ok(String::from_utf8_lossy(&output).into_owned())
}

/// Render the `--collect-only` output for a set of discovered tests.
#[expect(clippy::missing_errors_doc)]
#[wasm_bindgen]
pub fn format_collect(tests_json: &str, reporter_name: &str) -> Result<String, JsError> {
    let tests: Vec<tryke_types::TestItem> =
        serde_json::from_str(tests_json).map_err(|e| JsError::new(&e.to_string()))?;

    let output = match reporter_name {
        "text" => run_collect(
            tryke_reporter::TextReporter::with_writer(Vec::new()),
            &tests,
        ),
        "dot" => run_collect(tryke_reporter::DotReporter::with_writer(Vec::new()), &tests),
        "json" => run_collect(
            tryke_reporter::JSONReporter::with_writer(Vec::new()),
            &tests,
        ),
        "llm" => run_collect(tryke_reporter::LlmReporter::with_writer(Vec::new()), &tests),
        other => return Err(JsError::new(&format!("unknown reporter: {other}"))),
    };

    Ok(String::from_utf8_lossy(&output).into_owned())
}

#[must_use]
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

trait IntoWriter {
    fn into_writer(self) -> Vec<u8>;
}

impl IntoWriter for tryke_reporter::TextReporter<Vec<u8>> {
    fn into_writer(self) -> Vec<u8> {
        self.into_writer()
    }
}

impl IntoWriter for tryke_reporter::DotReporter<Vec<u8>> {
    fn into_writer(self) -> Vec<u8> {
        self.into_writer()
    }
}

impl IntoWriter for tryke_reporter::JSONReporter<Vec<u8>> {
    fn into_writer(self) -> Vec<u8> {
        self.into_writer()
    }
}

impl IntoWriter for tryke_reporter::LlmReporter<Vec<u8>> {
    fn into_writer(self) -> Vec<u8> {
        self.into_writer()
    }
}

fn run_reporter<R: Reporter + IntoWriter>(
    mut reporter: R,
    tests: &[tryke_types::TestItem],
    results: &[tryke_types::TestResult],
    summary: &tryke_types::RunSummary,
) -> Vec<u8> {
    reporter.on_run_start(tests);
    for result in results {
        reporter.on_test_complete(result);
    }
    reporter.on_run_complete(summary);
    reporter.into_writer()
}

fn run_collect<R: Reporter + IntoWriter>(
    mut reporter: R,
    tests: &[tryke_types::TestItem],
) -> Vec<u8> {
    reporter.on_collect_complete(tests);
    reporter.into_writer()
}

fn build_summary(results: &[tryke_types::TestResult]) -> tryke_types::RunSummary {
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;
    let mut xfailed = 0usize;
    let mut todo = 0usize;
    let mut total_duration = Duration::ZERO;

    for r in results {
        total_duration += r.duration;
        match &r.outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } | TestOutcome::XPassed => failed += 1,
            TestOutcome::Skipped { .. } => skipped += 1,
            TestOutcome::Error { .. } => errors += 1,
            TestOutcome::XFailed { .. } => xfailed += 1,
            TestOutcome::Todo { .. } => todo += 1,
        }
    }

    tryke_types::RunSummary {
        passed,
        failed,
        skipped,
        errors,
        xfailed,
        todo,
        duration: total_duration,
        discovery_duration: None,
        test_duration: Some(total_duration),
        file_count: 0,
        start_time: None,
        changed_selection: None,
    }
}

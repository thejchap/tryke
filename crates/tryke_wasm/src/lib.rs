use std::collections::HashSet;
use std::path::PathBuf;

use serde::Deserialize;
use tryke_reporter::Reporter;
use tryke_types::{RunSummary, RunTestResultWire, convert_wire_result};
use wasm_bindgen::prelude::*;

/// A single test result as sent by the Python playground runner: the
/// discovered test item alongside the flat runner output. Deserialized
/// with `#[serde(flatten)]` so the JSON is `{test: {...}, outcome: "passed", duration_ms: 42, ...}`.
#[derive(Deserialize)]
struct PlaygroundResult {
    test: tryke_types::TestItem,
    #[serde(flatten)]
    result: RunTestResultWire,
}

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
///
/// # Errors
/// Returns an error if the discovery result cannot be converted to a JS value.
#[wasm_bindgen]
pub fn discover(source: &str, filename: &str) -> Result<JsValue, JsError> {
    // Use a stable root so nested files like "pkg/test_api.py" keep their
    // full module_path ("pkg.test_api") instead of being relative to their
    // parent directory.
    let root = PathBuf::from(".");
    let path = root.join(filename);
    let src_roots = vec![root.clone()];
    let result = tryke_discovery::discover_file_from_source(&root, &src_roots, &path, source);
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&e.to_string()))
}

/// Parse multiple Python source files and return a combined result
/// including resolved import graph edges.
///
/// # Errors
/// Returns an error if `files_json` is invalid or if the combined discovery
/// result cannot be converted to a JS value.
#[wasm_bindgen]
pub fn discover_multi(files_json: &str) -> Result<JsValue, JsError> {
    let files: Vec<PlaygroundFile> =
        serde_json::from_str(files_json).map_err(|e| JsError::new(&e.to_string()))?;
    let root = PathBuf::from(".");

    let mut all_discovered: Vec<(PathBuf, tryke_types::DiscoveredFile)> = Vec::new();
    let src_roots = vec![root.clone()];

    for file in &files {
        // Join with root so relative-import candidate paths (built from
        // file.parent()) share the "./" prefix with file_set entries.
        let path = root.join(&file.filename);
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
            let from = path.strip_prefix(&root).unwrap_or(path).to_path_buf();
            edges.push(GraphEdge { from, to: target });
        }
    }

    let result = MultiResult {
        files: all_discovered
            .into_iter()
            .map(|(path, discovered)| FileResult {
                path: path.strip_prefix(&root).unwrap_or(&path).to_path_buf(),
                discovered,
            })
            .collect(),
        edges,
    };

    serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&e.to_string()))
}

/// Pipe test results through a real tryke reporter and return the
/// rendered output string (with ANSI escape codes for terminal-style
/// reporters).
///
/// Accepts the flat runner format produced by the Python playground
/// runner: `[{test: {...}, outcome: "passed", duration_ms: 42, ...}]`.
/// The conversion to `tryke_types::TestResult` happens here so the
/// Python side doesn't need to know about the reporter's wire format.
///
/// # Errors
/// Returns an error if `results_json` is invalid or if `reporter_name` does
/// not name a supported reporter.
#[wasm_bindgen]
pub fn format_results(results_json: &str, reporter_name: &str) -> Result<String, JsError> {
    let raw: Vec<PlaygroundResult> =
        serde_json::from_str(results_json).map_err(|e| JsError::new(&e.to_string()))?;
    let results: Vec<tryke_types::TestResult> = raw
        .into_iter()
        .map(|r| convert_wire_result(r.test, r.result))
        .collect();

    let tests: Vec<tryke_types::TestItem> = results.iter().map(|r| r.test.clone()).collect();
    let summary = RunSummary::from_results(&results);

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
        "next" => run_reporter(
            tryke_reporter::NextReporter::with_writer(Vec::new()),
            &tests,
            &results,
            &summary,
        ),
        "sugar" => run_reporter(
            tryke_reporter::SugarReporter::with_writer(Vec::new()),
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
///
/// # Errors
/// Returns an error if `tests_json` is invalid or if `reporter_name` does not
/// name a supported reporter.
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
        "next" => run_collect(
            tryke_reporter::NextReporter::with_writer(Vec::new()),
            &tests,
        ),
        "sugar" => run_collect(
            tryke_reporter::SugarReporter::with_writer(Vec::new()),
            &tests,
        ),
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

impl IntoWriter for tryke_reporter::NextReporter<Vec<u8>> {
    fn into_writer(self) -> Vec<u8> {
        self.into_writer()
    }
}

impl IntoWriter for tryke_reporter::SugarReporter<Vec<u8>> {
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

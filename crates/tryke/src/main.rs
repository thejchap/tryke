use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use tryke_reporter::{JSONReporter, Reporter, TextReporter};
use tryke_types::{Assertion, RunSummary, TestItem, TestOutcome, TestResult};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, default_value = "text")]
    format: OutputFormat,
}

#[derive(Clone, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
enum Commands {
    Test,
    Discover,
}

fn fake_tests() -> Vec<TestItem> {
    vec![
        TestItem {
            name: "test_add".into(),
            module_path: "tests.math".into(),
            file_path: Some(PathBuf::from("tests/math.py")),
            line_number: Some(10),
        },
        TestItem {
            name: "test_sub".into(),
            module_path: "tests.math".into(),
            file_path: Some(PathBuf::from("tests/math.py")),
            line_number: Some(25),
        },
        TestItem {
            name: "test_parse".into(),
            module_path: "tests.parser".into(),
            file_path: Some(PathBuf::from("tests/parser.py")),
            line_number: Some(8),
        },
    ]
}

fn fake_results(tests: &[TestItem]) -> Vec<TestResult> {
    vec![
        TestResult {
            test: tests[0].clone(),
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(12),
            stdout: String::new(),
            stderr: String::new(),
        },
        TestResult {
            test: tests[1].clone(),
            outcome: TestOutcome::Failed {
                message: "assertion failed: 3 - 1 == 3".into(),
                assertions: vec![Assertion {
                    expression: "assert 3 - 1 == 3".into(),
                    file: None,
                    line: 26,
                    span_offset: 15,
                    span_length: 1,
                    expected: "2".into(),
                    received: "3".into(),
                }],
            },
            duration: Duration::from_millis(5),
            stdout: String::new(),
            stderr: String::new(),
        },
        TestResult {
            test: tests[2].clone(),
            outcome: TestOutcome::Skipped {
                reason: Some("not implemented yet".into()),
            },
            duration: Duration::from_millis(0),
            stdout: String::new(),
            stderr: String::new(),
        },
    ]
}

fn run_test(reporter: &mut dyn Reporter) -> Result<()> {
    let start = Instant::now();

    let tests = fake_tests();
    reporter.on_run_start(&tests);

    let results = fake_results(&tests);
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;

    for result in &results {
        match &result.outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } => failed += 1,
            TestOutcome::Skipped { .. } => skipped += 1,
        }
        reporter.on_test_complete(result);
    }

    reporter.on_run_complete(&RunSummary {
        passed,
        failed,
        skipped,
        duration: start.elapsed(),
    });

    Ok(())
}

fn run_discover() -> Result<()> {
    let tests = fake_tests();
    for test in &tests {
        println!("{}", test.id());
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Commands::Test => {
            let mut reporter: Box<dyn Reporter> = match cli.format {
                OutputFormat::Text => Box::new(TextReporter::new()),
                OutputFormat::Json => Box::new(JSONReporter::new()),
            };
            run_test(&mut *reporter)
        }
        Commands::Discover => run_discover(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_text() {
        let mut reporter = TextReporter::new();
        assert!(run_test(&mut reporter).is_ok());
    }

    #[test]
    fn test_command_json() {
        let mut reporter = JSONReporter::new();
        assert!(run_test(&mut reporter).is_ok());
    }

    #[test]
    fn discover_command() {
        assert!(run_discover().is_ok());
    }

    #[test]
    fn test_item_id_with_file() {
        let item = TestItem {
            name: "test_add".into(),
            module_path: "tests.math".into(),
            file_path: Some(PathBuf::from("tests/math.py")),
            line_number: Some(10),
        };
        assert_eq!(item.id(), "tests/math.py::test_add");
    }

    #[test]
    fn test_item_id_without_file() {
        let item = TestItem {
            name: "test_add".into(),
            module_path: "tests.math".into(),
            file_path: None,
            line_number: None,
        };
        assert_eq!(item.id(), "tests.math::test_add");
    }
}

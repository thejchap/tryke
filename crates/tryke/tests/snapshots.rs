use std::path::PathBuf;
use std::time::Duration;

use tryke_reporter::{NextReporter, Reporter, SugarReporter, TextReporter};
use tryke_types::{Assertion, RunSummary, TestItem, TestOutcome, TestResult};

fn make_test(name: &str, file: &str) -> TestItem {
    TestItem {
        name: name.into(),
        module_path: "tests.test_validation".into(),
        file_path: Some(PathBuf::from(file)),
        line_number: Some(1),
        ..Default::default()
    }
}

/// Strip ANSI escape sequences so snapshot output is stable.
fn insta_settings() -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"tryke test v\d+\.\d+\.\d+", "tryke test v[VERSION]");
    settings
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Skip until 'm' (SGR terminator)
            for inner in chars.by_ref() {
                if inner == 'm' {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[test]
fn snapshot_failed_with_assertion() {
    let mut r = TextReporter::with_writer(Vec::new());
    r.on_test_complete(&TestResult {
        test: make_test("test_credit_card_all_zeros", "tests/test_validation.py"),
        outcome: TestOutcome::Failed {
            message: "expected falsy, received True".into(),
            traceback: None,
            assertions: vec![Assertion {
                expression: "expect(is_credit_card(\"0000000000000000\")).to_be_falsy()".into(),
                file: Some("tests/test_validation.py".into()),
                line: 1,
                span_offset: 7,
                span_length: 33,
                expected: "falsy".into(),
                received: "True".into(),
                expected_arg_span: None,
            }],
            executed_lines: vec![],
        },
        duration: Duration::from_millis(75),
        stdout: String::new(),
        stderr: String::new(),
    });
    let out = String::from_utf8(r.into_writer()).expect("valid utf-8");
    insta::assert_snapshot!("snapshot_failed_with_assertion", strip_ansi(&out));
}

#[test]
fn snapshot_failed_with_traceback() {
    let mut r = TextReporter::with_writer(Vec::new());
    r.on_test_complete(&TestResult {
        test: make_test("test_credit_card_all_zeros", "tests/test_validation.py"),
        outcome: TestOutcome::Failed {
            message: "expected True to be falsy".into(),
            traceback: Some(
                "Traceback (most recent call last):\n  \
                 File \"tests/test_validation.py\", line 5, in test_credit_card_all_zeros\n    \
                 expect(is_credit_card(\"0000000000000000\")).to_be_falsy()\n\
                 AssertionError: expected True to be falsy"
                    .into(),
            ),
            assertions: vec![],
            executed_lines: vec![],
        },
        duration: Duration::from_millis(75),
        stdout: String::new(),
        stderr: String::new(),
    });
    let out = String::from_utf8(r.into_writer()).expect("valid utf-8");
    insta::assert_snapshot!("snapshot_failed_with_traceback", strip_ansi(&out));
}

#[test]
fn snapshot_grouped_test_output() {
    let mut r = TextReporter::with_writer(Vec::new());
    let file = "tests/test_math.py";
    let make = |name: &str, groups: &[&str]| TestResult {
        test: TestItem {
            name: name.into(),
            module_path: "tests.test_math".into(),
            file_path: Some(PathBuf::from(file)),
            line_number: Some(1),
            groups: groups.iter().map(|&s| s.into()).collect(),
            ..Default::default()
        },
        outcome: TestOutcome::Passed,
        duration: Duration::from_millis(1),
        stdout: String::new(),
        stderr: String::new(),
    };
    r.on_run_start(&[]);
    r.on_test_complete(&make("adds_two_numbers", &["Math", "addition"]));
    r.on_test_complete(&make("adds_floats", &["Math", "addition"]));
    r.on_test_complete(&make("subtracts", &["Math", "subtraction"]));
    r.on_test_complete(&make("standalone", &[]));
    let out = String::from_utf8(r.into_writer()).expect("valid utf-8");
    let settings = insta_settings();
    settings.bind(|| {
        insta::assert_snapshot!(strip_ansi(&out));
    });
}

fn no_summary() -> RunSummary {
    RunSummary {
        passed: 0,
        failed: 0,
        skipped: 0,
        errors: 0,
        xfailed: 0,
        todo: 0,
        duration: Duration::from_millis(10),
        discovery_duration: None,
        test_duration: None,
        file_count: 0,
        start_time: None,
        changed_selection: None,
    }
}

#[test]
fn snapshot_next_two_pass_one_fail_two_files() {
    let mut r = NextReporter::with_writer(Vec::new());
    let make_test = |name: &str, file: &str| TestItem {
        name: name.into(),
        module_path: "tests.m".into(),
        file_path: Some(PathBuf::from(file)),
        ..Default::default()
    };
    let tests = vec![
        make_test("test_alpha", "tests/test_one.py"),
        make_test("test_beta", "tests/test_one.py"),
        make_test("test_gamma", "tests/test_two.py"),
    ];
    r.on_run_start(&tests);
    r.on_test_complete(&TestResult {
        test: tests[0].clone(),
        outcome: TestOutcome::Passed,
        duration: Duration::from_millis(9),
        stdout: String::new(),
        stderr: String::new(),
    });
    r.on_test_complete(&TestResult {
        test: tests[1].clone(),
        outcome: TestOutcome::Failed {
            message: "expected 1, got 2".into(),
            traceback: None,
            assertions: vec![],
            executed_lines: vec![],
        },
        duration: Duration::from_millis(123),
        stdout: String::new(),
        stderr: String::new(),
    });
    r.on_test_complete(&TestResult {
        test: tests[2].clone(),
        outcome: TestOutcome::Passed,
        duration: Duration::from_millis(4),
        stdout: String::new(),
        stderr: String::new(),
    });
    r.on_run_complete(&RunSummary {
        passed: 2,
        failed: 1,
        ..no_summary()
    });
    let out = String::from_utf8(r.into_writer()).expect("valid utf-8");
    let settings = insta_settings();
    settings.bind(|| {
        insta::assert_snapshot!(strip_ansi(&out));
    });
}

#[test]
fn snapshot_sugar_two_files_mixed() {
    let mut r = SugarReporter::with_writer(Vec::new());
    let make_test = |name: &str, file: &str| TestItem {
        name: name.into(),
        module_path: "tests.m".into(),
        file_path: Some(PathBuf::from(file)),
        ..Default::default()
    };
    let tests = vec![
        make_test("a", "tests/a.py"),
        make_test("b", "tests/a.py"),
        make_test("c", "tests/b.py"),
    ];
    r.on_run_start(&tests);
    r.on_test_complete(&TestResult {
        test: tests[0].clone(),
        outcome: TestOutcome::Passed,
        duration: Duration::from_millis(1),
        stdout: String::new(),
        stderr: String::new(),
    });
    r.on_test_complete(&TestResult {
        test: tests[1].clone(),
        outcome: TestOutcome::Failed {
            message: "boom".into(),
            traceback: None,
            assertions: vec![],
            executed_lines: vec![],
        },
        duration: Duration::from_millis(1),
        stdout: String::new(),
        stderr: String::new(),
    });
    r.on_test_complete(&TestResult {
        test: tests[2].clone(),
        outcome: TestOutcome::Passed,
        duration: Duration::from_millis(1),
        stdout: String::new(),
        stderr: String::new(),
    });
    r.on_run_complete(&RunSummary {
        passed: 2,
        failed: 1,
        file_count: 2,
        ..no_summary()
    });
    let out = String::from_utf8(r.into_writer()).expect("valid utf-8");
    let settings = insta_settings();
    settings.bind(|| {
        insta::assert_snapshot!(strip_ansi(&out));
    });
}

#[test]
fn snapshot_collect_grouped_tests() {
    let mut r = TextReporter::with_writer(Vec::new());
    let file = "tests/test_math.py";
    let make = |name: &str, groups: &[&str]| TestItem {
        name: name.into(),
        module_path: "tests.test_math".into(),
        file_path: Some(PathBuf::from(file)),
        line_number: Some(1),
        groups: groups.iter().map(|&s| s.into()).collect(),
        ..Default::default()
    };
    r.on_collect_complete(&[
        make("adds_two_numbers", &["Math", "addition"]),
        make("adds_floats", &["Math", "addition"]),
        make("subtracts", &["Math", "subtraction"]),
        make("standalone", &[]),
    ]);
    let out = String::from_utf8(r.into_writer()).expect("valid utf-8");
    let settings = insta_settings();
    settings.bind(|| {
        insta::assert_snapshot!(strip_ansi(&out));
    });
}

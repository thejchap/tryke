use std::path::PathBuf;
use std::time::Duration;

use tryke_reporter::{Reporter, TextReporter};
use tryke_types::{Assertion, TestItem, TestOutcome, TestResult};

fn make_test(name: &str, file: &str) -> TestItem {
    TestItem {
        name: name.into(),
        module_path: "tests.test_validation".into(),
        file_path: Some(PathBuf::from(file)),
        line_number: Some(1),
        display_name: None,
        expected_assertions: vec![],
    }
}

/// Strip ANSI escape sequences so snapshot output is stable.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // skip until 'm' (SGR terminator)
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
            }],
        },
        duration: Duration::from_millis(75),
        stdout: String::new(),
        stderr: String::new(),
    });
    let out = String::from_utf8(r.into_writer()).expect("valid utf-8");
    insta::assert_snapshot!(strip_ansi(&out));
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
        },
        duration: Duration::from_millis(75),
        stdout: String::new(),
        stderr: String::new(),
    });
    let out = String::from_utf8(r.into_writer()).expect("valid utf-8");
    insta::assert_snapshot!(strip_ansi(&out));
}

use std::fmt;

use tryke_types::Assertion;

pub fn render_assertions(test_file: Option<&str>, assertions: &[Assertion], buf: &mut String) {
    use fmt::Write;

    if assertions.is_empty() {
        return;
    }

    let total = assertions.len();
    let gutter_width = total.to_string().len();

    // file path from first assertion, falling back to test_file
    let file_path = assertions[0]
        .file
        .as_deref()
        .or(test_file)
        .unwrap_or("<unknown>");
    let _ = writeln!(buf, "  {file_path}");
    let _ = writeln!(buf);

    for (i, assertion) in assertions.iter().enumerate() {
        let n = i + 1;
        let span_len = assertion.span_length.max(1);
        let carets = "^".repeat(span_len);
        let padding = " ".repeat(assertion.span_offset);

        let _ = writeln!(buf, "  {n:>gutter_width$} │ {}", assertion.expression);
        let _ = writeln!(buf, "  {:>gutter_width$} │ {padding}{carets}", "");
        let _ = writeln!(
            buf,
            "  {:>gutter_width$}   Expected: {}",
            "", assertion.expected
        );
        let _ = writeln!(
            buf,
            "  {:>gutter_width$}   Received: {}",
            "", assertion.received
        );

        if n < total {
            let _ = writeln!(buf);
        }
    }

    let _ = writeln!(buf, "  {total}/{total} assertions failed");
}

/// Extract the last frame from a Python traceback string.
/// Returns from the last `File "..."` line to the end of the traceback.
#[must_use]
pub fn extract_last_frame(traceback: &str) -> String {
    let lines: Vec<&str> = traceback.lines().collect();
    let mut last_file_idx = None;
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("File \"") {
            last_file_idx = Some(i);
        }
    }
    match last_file_idx {
        Some(idx) => lines[idx..].join("\n"),
        None => traceback.to_string(),
    }
}

/// Render a failure message with optional traceback.
/// At normal verbosity, shows only the last frame. At verbose, shows full traceback.
pub fn render_failure_message(
    message: &str,
    traceback: Option<&str>,
    verbose: bool,
    buf: &mut String,
) {
    use fmt::Write;

    let _ = writeln!(buf, "  {message}");
    if let Some(tb) = traceback {
        let display_tb = if verbose {
            tb.to_string()
        } else {
            extract_last_frame(tb)
        };
        for line in display_tb.lines() {
            let _ = writeln!(buf, "    {line}");
        }
    }
}

/// Render an error message for worker/infrastructure errors.
pub fn render_error_message(message: &str, buf: &mut String) {
    use fmt::Write;

    for line in message.lines() {
        let _ = writeln!(buf, "    {line}");
    }
}

/// Render captured stdout or stderr with a label header.
pub fn render_captured_output(label: &str, content: &str, buf: &mut String) {
    use fmt::Write;

    let _ = writeln!(buf, "  ── {label} ──");
    for line in content.lines() {
        let _ = writeln!(buf, "    {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_assertion(expression: &str, offset: usize, len: usize) -> Assertion {
        Assertion {
            expression: expression.into(),
            file: None,
            line: 10,
            span_offset: offset,
            span_length: len,
            expected: "2".into(),
            received: "3".into(),
        }
    }

    #[test]
    fn single_assertion() {
        let assertions = vec![make_assertion("assert_eq!(a, 2)", 14, 1)];
        let mut buf = String::new();
        render_assertions(Some("tests/math.py"), &assertions, &mut buf);

        assert!(buf.contains("1 │"));
        assert!(buf.contains("Expected: 2"));
        assert!(buf.contains("Received: 3"));
        assert!(buf.contains("tests/math.py"));
        assert!(buf.contains("1/1 assertions failed"));
    }

    #[test]
    fn multiple_assertions() {
        let assertions = vec![
            make_assertion("assert_eq!(a, 2)", 14, 1),
            make_assertion("assert_eq!(b, 5)", 14, 1),
        ];
        let mut buf = String::new();
        render_assertions(Some("tests/math.py"), &assertions, &mut buf);

        assert!(buf.contains("2/2 assertions failed"));
    }

    #[test]
    fn empty_assertions() {
        let mut buf = String::new();
        render_assertions(Some("tests/math.py"), &[], &mut buf);

        assert!(buf.is_empty());
    }

    #[test]
    fn no_file_path() {
        let assertions = vec![make_assertion("assert_eq!(x, 1)", 14, 1)];
        let mut buf = String::new();
        render_assertions(None, &assertions, &mut buf);

        assert!(buf.contains("<unknown>"));
        assert!(buf.contains("Expected: 2"));
    }

    #[test]
    fn assertion_file_overrides_test_file() {
        let assertions = vec![Assertion {
            expression: "assert_eq!(x, 1)".into(),
            file: Some("helpers/utils.py".into()),
            line: 5,
            span_offset: 14,
            span_length: 1,
            expected: "1".into(),
            received: "2".into(),
        }];
        let mut buf = String::new();
        render_assertions(Some("tests/math.py"), &assertions, &mut buf);

        assert!(buf.contains("helpers/utils.py"));
        assert!(!buf.contains("tests/math.py"));
    }

    #[test]
    fn render_assertions_exact_format() {
        let assertions = vec![
            Assertion {
                expression: "expect(user.name).to_equal(\"alice\")".into(),
                file: Some("src/users/test_models.py".into()),
                line: 10,
                span_offset: 7,
                span_length: 9,
                expected: "\"alice\"".into(),
                received: "\"Alice\"".into(),
            },
            Assertion {
                expression: "expect(user.age).to_be_greater_than(0)".into(),
                file: Some("src/users/test_models.py".into()),
                line: 11,
                span_offset: 7,
                span_length: 8,
                expected: "> 0".into(),
                received: "-1".into(),
            },
        ];
        let mut buf = String::new();
        render_assertions(Some("fallback.py"), &assertions, &mut buf);

        let expected = concat!(
            "  src/users/test_models.py\n",
            "\n",
            "  1 │ expect(user.name).to_equal(\"alice\")\n",
            "    │        ^^^^^^^^^\n",
            "      Expected: \"alice\"\n",
            "      Received: \"Alice\"\n",
            "\n",
            "  2 │ expect(user.age).to_be_greater_than(0)\n",
            "    │        ^^^^^^^^\n",
            "      Expected: > 0\n",
            "      Received: -1\n",
            "  2/2 assertions failed\n",
        );

        assert_eq!(buf, expected);
    }

    #[test]
    fn extract_last_frame_simple() {
        let tb = "\
Traceback (most recent call last):
  File \"tests/test_math.py\", line 5, in test_add
    assert 1 + 1 == 3
AssertionError";
        let result = extract_last_frame(tb);
        assert!(result.starts_with("  File \"tests/test_math.py\""));
        assert!(result.contains("AssertionError"));
    }

    #[test]
    fn extract_last_frame_multiple_frames() {
        let tb = "\
Traceback (most recent call last):
  File \"tests/test_math.py\", line 10, in test_div
    result = divide(1, 0)
  File \"math_utils.py\", line 3, in divide
    return a / b
ZeroDivisionError: division by zero";
        let result = extract_last_frame(tb);
        assert!(result.starts_with("  File \"math_utils.py\""));
        assert!(result.contains("ZeroDivisionError"));
        assert!(!result.contains("test_math.py"));
    }

    #[test]
    fn extract_last_frame_no_file_lines() {
        let tb = "SomeError: something went wrong";
        let result = extract_last_frame(tb);
        assert_eq!(result, tb);
    }

    #[test]
    fn render_failure_message_with_traceback_normal() {
        let mut buf = String::new();
        let tb = "\
Traceback (most recent call last):
  File \"tests/test_math.py\", line 5, in test_add
    assert 1 + 1 == 3
AssertionError";
        render_failure_message("AssertionError", Some(tb), false, &mut buf);
        assert!(buf.contains("AssertionError"));
        // normal mode shows only last frame
        assert!(buf.contains("File \"tests/test_math.py\""));
    }

    #[test]
    fn render_failure_message_with_traceback_verbose() {
        let mut buf = String::new();
        let tb = "\
Traceback (most recent call last):
  File \"tests/test_math.py\", line 10, in test_div
    result = divide(1, 0)
  File \"math_utils.py\", line 3, in divide
    return a / b
ZeroDivisionError: division by zero";
        render_failure_message(
            "ZeroDivisionError: division by zero",
            Some(tb),
            true,
            &mut buf,
        );
        assert!(buf.contains("test_math.py"));
        assert!(buf.contains("math_utils.py"));
        assert!(buf.contains("Traceback"));
    }

    #[test]
    fn render_failure_message_without_traceback() {
        let mut buf = String::new();
        render_failure_message("assertion failed", None, false, &mut buf);
        assert!(buf.contains("assertion failed"));
    }

    #[test]
    fn render_error_message_multiline() {
        let mut buf = String::new();
        render_error_message("worker spawn failed: No such file\ndetails here", &mut buf);
        assert!(buf.contains("worker spawn failed"));
        assert!(buf.contains("details here"));
    }

    #[test]
    fn render_captured_output_formats_content() {
        let mut buf = String::new();
        render_captured_output("stdout", "hello\nworld", &mut buf);
        assert!(buf.contains("── stdout ──"));
        assert!(buf.contains("hello"));
        assert!(buf.contains("world"));
    }
}

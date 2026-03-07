use std::fmt;

use miette::{
    Diagnostic, GraphicalReportHandler, GraphicalTheme, LabeledSpan, MietteError,
    MietteSpanContents, NamedSource, Report, Severity, SourceCode, SourceSpan, SpanContents,
};
use tryke_types::Assertion;

/// Wraps a source string with a line offset so miette reports the correct
/// line number instead of always starting at line 1.
struct OffsetSource {
    source: String,
    line_offset: usize, // 0-based
}

impl SourceCode for OffsetSource {
    fn read_span<'a>(
        &'a self,
        span: &SourceSpan,
        context_lines_before: usize,
        context_lines_after: usize,
    ) -> Result<Box<dyn SpanContents<'a> + 'a>, MietteError> {
        let inner = self
            .source
            .read_span(span, context_lines_before, context_lines_after)?;
        Ok(Box::new(MietteSpanContents::new(
            inner.data(),
            *inner.span(),
            inner.line() + self.line_offset,
            inner.column(),
            inner.line_count(),
        )))
    }
}

struct AssertionReport {
    source: NamedSource<OffsetSource>,
    label: LabeledSpan,
}

impl fmt::Debug for AssertionReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssertionReport").finish()
    }
}

impl fmt::Display for AssertionReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "assertion failed")
    }
}

impl std::error::Error for AssertionReport {}

impl Diagnostic for AssertionReport {
    fn severity(&self) -> Option<Severity> {
        Some(Severity::Error)
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(&self.source)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(std::iter::once(self.label.clone())))
    }
}

pub fn render_assertions(test_file: Option<&str>, assertions: &[Assertion], buf: &mut String) {
    render_assertions_themed(test_file, assertions, GraphicalTheme::unicode(), buf);
}

pub fn render_assertions_plain(
    test_file: Option<&str>,
    assertions: &[Assertion],
    buf: &mut String,
) {
    render_assertions_themed(
        test_file,
        assertions,
        GraphicalTheme::unicode_nocolor(),
        buf,
    );
}

fn render_assertions_themed(
    test_file: Option<&str>,
    assertions: &[Assertion],
    theme: GraphicalTheme,
    buf: &mut String,
) {
    use fmt::Write;

    if assertions.is_empty() {
        return;
    }

    let handler = GraphicalReportHandler::new_themed(theme);
    let mut failed = 0;

    for assertion in assertions {
        // prefer the assertion's own file, fall back to the test's file
        let source_name = assertion
            .file
            .as_deref()
            .or(test_file)
            .unwrap_or("<unknown>");
        let offset_source = OffsetSource {
            source: assertion.expression.clone(),
            line_offset: assertion.line.saturating_sub(1),
        };
        let source = NamedSource::new(source_name, offset_source);
        let label_text = format!(
            "expected {}, received {}",
            assertion.expected, assertion.received
        );
        let label = LabeledSpan::new(
            Some(label_text),
            assertion.span_offset,
            assertion.span_length,
        );

        let report = AssertionReport { source, label };
        let report = Report::new(report);

        let mut rendered = String::new();
        if handler
            .render_report(&mut rendered, report.as_ref())
            .is_ok()
        {
            buf.push_str(&rendered);
        }

        failed += 1;
    }

    let _ = writeln!(buf, "  {failed}/{} assertions failed", assertions.len());
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

/// Render a failure message with optional traceback via miette.
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

        assert!(buf.contains("assertion failed"));
        assert!(buf.contains("expected 2, received 3"));
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
        assert!(buf.contains("assertion failed"));
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

    #[test]
    fn assertion_shows_correct_line_number() {
        let assertions = vec![Assertion {
            expression: "expect(x).to_equal(2)".into(),
            file: Some("tests/test_math.py".into()),
            line: 42,
            span_offset: 7,
            span_length: 1,
            expected: "2".into(),
            received: "3".into(),
        }];
        let mut buf = String::new();
        render_assertions_plain(None, &assertions, &mut buf);
        assert!(
            buf.contains("42"),
            "expected line 42 in output, got:\n{buf}"
        );
    }

    #[test]
    fn assertion_line_zero_handled() {
        let assertions = vec![Assertion {
            expression: "expect(x).to_equal(1)".into(),
            file: Some("test.py".into()),
            line: 0,
            span_offset: 7,
            span_length: 1,
            expected: "1".into(),
            received: "2".into(),
        }];
        let mut buf = String::new();
        // Should not panic with line 0 (saturating_sub handles it)
        render_assertions_plain(None, &assertions, &mut buf);
        assert!(buf.contains("assertion failed"));
    }
}

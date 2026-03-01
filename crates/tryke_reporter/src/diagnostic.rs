use std::fmt;

use miette::{
    Diagnostic, GraphicalReportHandler, GraphicalTheme, LabeledSpan, NamedSource, Report, Severity,
    SourceCode,
};
use tryke_types::Assertion;

struct AssertionReport {
    source: NamedSource<String>,
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

pub fn render_assertions(file: Option<&str>, assertions: &[Assertion], buf: &mut String) {
    use fmt::Write;

    if assertions.is_empty() {
        return;
    }

    let handler = GraphicalReportHandler::new_themed(GraphicalTheme::unicode_nocolor());
    let mut failed = 0;

    for assertion in assertions {
        let source_name = file.unwrap_or("<unknown>");
        let source = NamedSource::new(source_name, assertion.expression.clone());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_assertion(expression: &str, offset: usize, len: usize) -> Assertion {
        Assertion {
            expression: expression.into(),
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
        render_assertions(Some("tests/math.rs"), &assertions, &mut buf);

        assert!(buf.contains("assertion failed"));
        assert!(buf.contains("expected 2, received 3"));
        assert!(buf.contains("tests/math.rs"));
        assert!(buf.contains("1/1 assertions failed"));
    }

    #[test]
    fn multiple_assertions() {
        let assertions = vec![
            make_assertion("assert_eq!(a, 2)", 14, 1),
            make_assertion("assert_eq!(b, 5)", 14, 1),
        ];
        let mut buf = String::new();
        render_assertions(Some("tests/math.rs"), &assertions, &mut buf);

        assert!(buf.contains("2/2 assertions failed"));
    }

    #[test]
    fn empty_assertions() {
        let mut buf = String::new();
        render_assertions(Some("tests/math.rs"), &[], &mut buf);

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
}

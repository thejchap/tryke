use std::io;

use owo_colors::OwoColorize;
use tryke_types::{RunSummary, TestItem};

use crate::duration::format_duration;
use crate::reporter::WatchIdleInfo;

/// Keyboard shortcuts shown beneath the summary/idle badge in watch
/// mode. Stacked vertically with keys right-aligned in the same
/// column as the `Tests` / `Discovery` labels so the footer reads as
/// a continuation of the summary block.
const WATCH_KEYBINDINGS: &[(&str, &str)] = &[("q / esc", "Quit"), ("enter", "Run all tests")];

/// Width of the right-aligned label column shared by `Tests`,
/// `Start at`, `Duration`, and the watch keybinding keys.
const LABEL_COLUMN_WIDTH: usize = 11;

fn write_watch_keybindings<W: io::Write>(writer: &mut W) {
    let _ = writeln!(writer);
    for (key, label) in WATCH_KEYBINDINGS {
        let pad = " ".repeat(LABEL_COLUMN_WIDTH.saturating_sub(key.len()));
        let _ = writeln!(writer, "{pad}{}  {}", (*key).bold(), (*label).dimmed());
    }
}

// Layout — labels right-aligned, values line up at column 13:
//
// Test Files  3 passed (3)
//      Tests  41 passed (41)
//   Start at  16:28:06
//   Duration  36.98ms (discover 12.34ms, tests 24.64ms)
//
//  PASS

pub fn write_summary<W: io::Write>(writer: &mut W, summary: &RunSummary) {
    write_summary_with_hint(writer, summary, None);
}

#[expect(clippy::too_many_lines)]
pub fn write_summary_with_hint<W: io::Write>(
    writer: &mut W,
    summary: &RunSummary,
    watch_hint: Option<&str>,
) {
    let total = summary.passed
        + summary.failed
        + summary.skipped
        + summary.errors
        + summary.xfailed
        + summary.todo;

    let has_failures = summary.failed > 0 || summary.errors > 0;

    let mut parts: Vec<String> = Vec::new();

    if summary.failed > 0 {
        parts.push(format!(
            "{}",
            format!("{} failed", summary.failed).red().bold()
        ));
    }
    if summary.errors > 0 {
        parts.push(format!(
            "{}",
            format!("{} error", summary.errors).red().bold()
        ));
    }
    if summary.passed > 0 {
        parts.push(format!(
            "{}",
            format!("{} passed", summary.passed).green().bold()
        ));
    }
    if summary.skipped > 0 {
        parts.push(format!(
            "{}",
            format!("{} skipped", summary.skipped).yellow()
        ));
    }
    if summary.xfailed > 0 {
        parts.push(format!("{}", format!("{} xfail", summary.xfailed).dimmed()));
    }
    if summary.todo > 0 {
        parts.push(format!("{}", format!("{} todo", summary.todo).cyan()));
    }

    if parts.is_empty() {
        parts.push(format!("{}", "0 passed".green().bold()));
    }

    let separator = format!(" {} ", "|".dimmed());

    let _ = writeln!(writer);

    // "Test Files" = 10 chars, right-aligned with other labels
    if summary.file_count > 0 {
        let files_label = if has_failures {
            format!("{} ran", summary.file_count)
                .red()
                .bold()
                .to_string()
        } else {
            format!("{} passed", summary.file_count)
                .green()
                .bold()
                .to_string()
        };
        let _ = writeln!(
            writer,
            " {}  {} {}",
            "Test Files".dimmed(),
            files_label,
            format!("({})", summary.file_count).dimmed()
        );
    }

    // "     Tests" = 10 chars
    let _ = writeln!(
        writer,
        "      {}  {} {}",
        "Tests".dimmed(),
        parts.join(&separator),
        format!("({total})").dimmed()
    );

    if let Some(changed) = &summary.changed_selection {
        let _ = writeln!(
            writer,
            "    {}  {} {} {}",
            "Changed".dimmed(),
            format!("{} files", changed.changed_files).cyan(),
            "->".dimmed(),
            format!("{} tests", changed.affected_tests).cyan()
        );
    }

    // "  Start at" = 10 chars
    if let Some(ref t) = summary.start_time {
        let _ = writeln!(writer, "   {}  {}", "Start at".dimmed(), t);
    }

    // "  Duration" = 10 chars
    let mut breakdown_parts: Vec<String> = Vec::new();
    if let Some(d) = summary.discovery_duration {
        breakdown_parts.push(format!("discover {}", format_duration(d)));
    }
    if let Some(d) = summary.test_duration {
        breakdown_parts.push(format!("tests {}", format_duration(d)));
    }

    let breakdown = if breakdown_parts.is_empty() {
        String::new()
    } else {
        format!(" {}", format!("({})", breakdown_parts.join(", ")).dimmed())
    };

    let _ = writeln!(
        writer,
        "   {}  {}{}",
        "Duration".dimmed(),
        format_duration(summary.duration),
        breakdown
    );

    let badge = if has_failures {
        format!("{}", " FAIL ".on_red().black().bold())
    } else {
        format!("{}", " PASS ".on_green().black().bold())
    };

    let _ = writeln!(writer);
    if let Some(hint) = watch_hint {
        let _ = writeln!(writer, " {badge} {}", hint.dimmed());
        write_watch_keybindings(writer);
    } else {
        let _ = writeln!(writer, " {badge}");
    }
}

/// Render the idle frame shown when watch mode is awaiting the first
/// file change. Mirrors the live-run summary layout (Tests / Start at
/// / Duration block plus a trailing badge) but stamps an `IDLE` badge
/// in blue and reports the discovery duration only — no test
/// execution has happened yet.
pub fn write_idle_summary<W: io::Write>(writer: &mut W, info: &WatchIdleInfo<'_>) {
    let badge = format!("{}", " IDLE ".on_blue().black().bold());

    let _ = writeln!(writer);

    let _ = writeln!(
        writer,
        "      {}  {} {}",
        "Tests".dimmed(),
        "No tests run yet".dimmed(),
        "(0)".dimmed()
    );

    if let Some(t) = info.start_time {
        let _ = writeln!(writer, "   {}  {}", "Start at".dimmed(), t);
    }

    if let Some(d) = info.discovery_duration {
        let _ = writeln!(writer, "  {}  {}", "Discovery".dimmed(), format_duration(d));
    }

    let _ = writeln!(writer);
    let _ = writeln!(writer, " {badge} {}", info.hint.dimmed());
    write_watch_keybindings(writer);
}

/// Render the canonical `--collect-only` view: a banner with the
/// subcommand label, then tests grouped by file with `describe()`
/// nesting indented, and a `N tests collected.` footer. Reporters
/// share this implementation so the `--collect-only` output is
/// identical regardless of `--reporter`, with the sole exception of
/// machine-readable formats (json, junit) that have their own
/// representation.
pub fn write_collect_list<W: io::Write>(
    writer: &mut W,
    subcommand_label: &str,
    tests: &[TestItem],
) {
    let _ = writeln!(
        writer,
        "{} {}",
        subcommand_label.bold(),
        format!("v{}", env!("CARGO_PKG_VERSION")).dimmed()
    );
    let _ = writeln!(writer);
    let mut current_file: Option<&std::path::Path> = None;
    let mut current_groups: Vec<String> = Vec::new();
    for test in tests {
        let file = test.file_path.as_deref();
        if file != current_file {
            if current_file.is_some() {
                let _ = writeln!(writer);
            }
            if let Some(path) = file {
                let _ = writeln!(writer, "{}:", path.display());
            }
            current_file = file;
            current_groups.clear();
        }
        if test.groups != current_groups {
            let common = current_groups
                .iter()
                .zip(test.groups.iter())
                .take_while(|(a, b)| a == b)
                .count();
            for (depth, group) in test.groups.iter().enumerate().skip(common) {
                let indent = "  ".repeat(depth + 1);
                let _ = writeln!(writer, "{indent}{group}");
            }
            current_groups.clone_from(&test.groups);
        }
        let group_indent = "  ".repeat(test.groups.len());
        let display = test.display_label();
        let _ = writeln!(writer, "  {group_indent}{}", display.dimmed());
    }
    let _ = writeln!(writer);
    let _ = writeln!(writer, "{} tests collected.", tests.len());
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tryke_types::RunSummary;

    use super::*;

    fn render(summary: &RunSummary) -> String {
        let mut buf = Vec::new();
        write_summary(&mut buf, summary);
        String::from_utf8(buf).expect("valid utf-8")
    }

    #[test]
    fn all_passed_shows_pass_badge() {
        let out = render(&RunSummary {
            passed: 5,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(50),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(out.contains("PASS"));
        assert!(out.contains("5 passed"));
        assert!(out.contains("(5)"));
        assert!(!out.contains("failed"));
        assert!(!out.contains("skipped"));
        assert!(out.contains("Duration"));
        assert!(out.contains("50.00ms"));
    }

    #[test]
    fn failures_show_fail_badge() {
        let out = render(&RunSummary {
            passed: 3,
            failed: 1,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(100),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(out.contains("FAIL"));
        assert!(out.contains("1 failed"));
        assert!(out.contains("3 passed"));
    }

    #[test]
    fn errors_show_fail_badge() {
        let out = render(&RunSummary {
            passed: 3,
            failed: 0,
            skipped: 0,
            errors: 1,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(100),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(out.contains("FAIL"));
        assert!(out.contains("1 error"));
    }

    #[test]
    fn mixed_results() {
        let out = render(&RunSummary {
            passed: 3,
            failed: 1,
            skipped: 2,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(100),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(out.contains("1 failed"));
        assert!(out.contains("3 passed"));
        assert!(out.contains("2 skipped"));
        assert!(out.contains("(6)"));
    }

    #[test]
    fn includes_all_categories() {
        let out = render(&RunSummary {
            passed: 1,
            failed: 1,
            skipped: 1,
            errors: 1,
            xfailed: 1,
            todo: 1,
            duration: Duration::from_millis(200),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(out.contains("1 failed"));
        assert!(out.contains("1 error"));
        assert!(out.contains("1 passed"));
        assert!(out.contains("1 skipped"));
        assert!(out.contains("1 xfail"));
        assert!(out.contains("1 todo"));
        assert!(out.contains("(6)"));
    }

    #[test]
    fn zero_tests() {
        let out = render(&RunSummary {
            passed: 0,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(1),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(out.contains("PASS"));
        assert!(out.contains("0 passed"));
        assert!(out.contains("(0)"));
    }

    #[test]
    fn duration_seconds() {
        let out = render(&RunSummary {
            passed: 1,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(1500),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(out.contains("1.50s"));
    }

    #[test]
    fn duration_minutes_seconds() {
        let out = render(&RunSummary {
            passed: 1,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(65_500),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(out.contains("1:05.50"), "expected M:SS.SS, got: {out}");
        assert!(!out.contains("65.50s"));
    }

    #[test]
    fn duration_exactly_one_minute() {
        let out = render(&RunSummary {
            passed: 1,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_secs(60),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(out.contains("1:00.00"));
    }

    #[test]
    fn duration_rounds_centiseconds_with_carry() {
        // 119.999s should round up to 2:00.00, not truncate to 1:59.99.
        let out = render(&RunSummary {
            passed: 1,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(119_999),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(out.contains("2:00.00"), "expected carry, got: {out}");
        assert!(!out.contains("1:59.99"));
    }

    #[test]
    fn duration_breakdown_uses_minutes_seconds() {
        let out = render(&RunSummary {
            passed: 5,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_secs(125),
            discovery_duration: Some(Duration::from_millis(30)),
            test_duration: Some(Duration::from_secs(95)),
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(out.contains("2:05.00"));
        assert!(out.contains("tests 1:35.00"));
        assert!(out.contains("discover 30.00ms"));
    }

    #[test]
    fn failed_appears_before_passed() {
        let out = render(&RunSummary {
            passed: 3,
            failed: 1,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(50),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        let failed_pos = out.find("failed").expect("should contain failed");
        let passed_pos = out.find("passed").expect("should contain passed");
        assert!(
            failed_pos < passed_pos,
            "failed should appear before passed"
        );
    }

    #[test]
    fn duration_breakdown_shown() {
        let out = render(&RunSummary {
            passed: 5,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(100),
            discovery_duration: Some(Duration::from_millis(30)),
            test_duration: Some(Duration::from_millis(70)),
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(out.contains("discover 30.00ms"));
        assert!(out.contains("tests 70.00ms"));
    }

    #[test]
    fn no_breakdown_when_durations_absent() {
        let out = render(&RunSummary {
            passed: 5,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(100),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(!out.contains("discover"));
        assert!(!out.contains("tests "));
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut in_escape = false;
        for c in s.chars() {
            if in_escape {
                if c.is_ascii_alphabetic() {
                    in_escape = false;
                }
            } else if c == '\x1b' {
                in_escape = true;
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn labels_right_aligned() {
        let out = render(&RunSummary {
            passed: 1,
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
        });
        let lines: Vec<&str> = out.lines().collect();
        let tests_line = lines.iter().find(|l| l.contains("Tests")).unwrap();
        let dur_line = lines.iter().find(|l| l.contains("Duration")).unwrap();
        let t_plain = strip_ansi(tests_line);
        let d_plain = strip_ansi(dur_line);
        // "Tests" ends at col 10, "Duration" ends at col 10 — values start at same column
        let t_val_col = t_plain.find("Tests").unwrap() + "Tests".len();
        let d_val_col = d_plain.find("Duration").unwrap() + "Duration".len();
        assert_eq!(
            t_val_col, d_val_col,
            "values after Tests and Duration should start at same column"
        );
    }

    #[test]
    fn badge_on_separate_line() {
        let out = render(&RunSummary {
            passed: 1,
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
        });
        let lines: Vec<&str> = out.lines().collect();
        let tests_idx = lines.iter().position(|l| l.contains("Tests")).unwrap();
        let badge_idx = lines.iter().position(|l| l.contains("PASS")).unwrap();
        assert!(
            badge_idx > tests_idx,
            "badge should appear after the Tests line"
        );
        // Badge line should not contain "Tests" or "Duration"
        let badge_line = lines[badge_idx];
        assert!(!badge_line.contains("Tests"));
        assert!(!badge_line.contains("Duration"));
    }

    #[test]
    fn file_count_shown() {
        let out = render(&RunSummary {
            passed: 5,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(50),
            discovery_duration: None,
            test_duration: None,
            file_count: 3,
            start_time: None,
            changed_selection: None,
        });
        assert!(out.contains("Test Files"));
        assert!(out.contains("3 passed"));
    }

    #[test]
    fn file_count_hidden_when_zero() {
        let out = render(&RunSummary {
            passed: 5,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(50),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: None,
            changed_selection: None,
        });
        assert!(!out.contains("Test Files"));
    }

    #[test]
    fn start_time_shown() {
        let out = render(&RunSummary {
            passed: 1,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(10),
            discovery_duration: None,
            test_duration: None,
            file_count: 0,
            start_time: Some("16:28:06".into()),
            changed_selection: None,
        });
        assert!(out.contains("Start at"));
        assert!(out.contains("16:28:06"));
    }

    #[test]
    fn start_time_hidden_when_absent() {
        let out = render(&RunSummary {
            passed: 1,
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
        });
        assert!(!out.contains("Start at"));
    }

    #[test]
    fn watch_hint_appended_to_pass_badge() {
        let mut buf = Vec::new();
        write_summary_with_hint(
            &mut buf,
            &RunSummary {
                passed: 1,
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
            },
            Some("Waiting for file changes..."),
        );
        let out = String::from_utf8(buf).expect("utf-8");
        assert!(out.contains("PASS"));
        assert!(out.contains("Waiting for file changes..."));
        // Hint should be on the same line as the badge.
        let badge_line = out
            .lines()
            .find(|l| l.contains("PASS"))
            .expect("badge line");
        assert!(badge_line.contains("Waiting"));
        // Keybindings render on a separate line below the badge.
        assert!(out.contains("Quit"));
        assert!(out.contains("Run all tests"));
    }

    #[test]
    fn no_watch_hint_when_none() {
        let out = render(&RunSummary {
            passed: 1,
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
        });
        assert!(!out.contains("Waiting"));
    }

    #[test]
    fn changed_summary_shown() {
        let out = render(&RunSummary {
            passed: 2,
            failed: 0,
            skipped: 0,
            errors: 0,
            xfailed: 0,
            todo: 0,
            duration: Duration::from_millis(10),
            discovery_duration: None,
            test_duration: None,
            file_count: 1,
            start_time: None,
            changed_selection: Some(tryke_types::ChangedSelectionSummary {
                changed_files: 3,
                affected_tests: 2,
            }),
        });
        assert!(out.contains("Changed"));
        assert!(out.contains("3 files"));
        assert!(out.contains("2 tests"));
    }
}

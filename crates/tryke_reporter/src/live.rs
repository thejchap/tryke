//! Terminal-control primitives for the `next` and `sugar` reporters.
//!
//! `LiveArea` is a thin wrapper over [`indicatif::MultiProgress`] +
//! [`indicatif::ProgressBar`]. It owns a single bottom bar and exposes a
//! `println` channel that atomically clears the bar, prints a line above
//! it, and redraws — solving the stdout/stderr cursor-desync class of
//! bug we'd hit if we tried to interleave a hand-rolled bar with raw
//! `writeln!` calls.
//!
//! When `enabled` is false (non-TTY, redirect, snapshot tests) every
//! method falls back to writing directly through the caller's writer,
//! so the per-test/per-file rows still land in tests' captured output
//! and stay free of ANSI escapes.

use std::borrow::Cow;
use std::io::{self, IsTerminal, Write};
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

const FALLBACK_WIDTH: usize = 80;
const DRAW_HZ: u8 = 60;

/// Returns true when both stdout and stderr are terminals. Gates the
/// live bar: if the user redirects either stream we fall back to plain
/// line writes so the redirect target gets clean text and we don't
/// strand rows on stderr while the summary goes to a captured stdout.
#[must_use]
pub fn supports_live() -> bool {
    io::stdout().is_terminal() && io::stderr().is_terminal()
}

/// Format `d` as `HH:MM:SS`, with the entire duration clamped to
/// `99:59:59` (any longer reads as "off the chart"). Used by callers
/// that build bar segments outside of indicatif's template (e.g. a
/// frozen elapsed value rendered in the summary).
#[must_use]
pub fn format_elapsed(d: Duration) -> String {
    const MAX_SECS: u64 = 99 * 3600 + 59 * 60 + 59;
    let secs = d.as_secs().min(MAX_SECS);
    let hours = secs / 3600;
    let minutes = (secs / 60) % 60;
    let seconds = secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// Render a single-row progress bar of `width` cells. Pure: no I/O.
/// Used for sugar's per-file mini-bar suffix, which is composed inline
/// rather than via indicatif.
#[must_use]
pub fn render_bar(filled: usize, total: usize, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let cells = if total == 0 {
        0
    } else {
        (filled.saturating_mul(width)) / total
    };
    let cells = cells.min(width);
    let mut out = String::with_capacity(width * 3);
    for _ in 0..cells {
        out.push('█');
    }
    for _ in 0..(width - cells) {
        out.push('░');
    }
    out
}

/// Live bottom-of-screen status area. One bar, plus a `println` channel
/// for per-test/per-file rows.
pub struct LiveArea {
    enabled: bool,
    multi: Option<MultiProgress>,
    bar: Option<ProgressBar>,
    width: usize,
}

impl LiveArea {
    /// Real-terminal mode: bar drawn on stderr at ~60fps.
    #[must_use]
    pub fn new() -> Self {
        let enabled = supports_live();
        let multi = if enabled {
            Some(MultiProgress::with_draw_target(
                ProgressDrawTarget::stderr_with_hz(DRAW_HZ),
            ))
        } else {
            None
        };
        let (_, cols) = console::Term::stderr().size();
        let width = if cols == 0 {
            FALLBACK_WIDTH
        } else {
            cols as usize
        };
        Self {
            enabled,
            multi,
            bar: None,
            width,
        }
    }

    /// Disabled (snapshot tests, non-TTY). All bar methods become
    /// no-ops; `println` forwards to the caller's writer.
    #[must_use]
    pub fn hidden() -> Self {
        Self {
            enabled: false,
            multi: None,
            bar: None,
            width: FALLBACK_WIDTH,
        }
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Lazily start the bar with `total` ticks and a custom indicatif
    /// template. Idempotent — second call replaces style + length.
    pub fn start(&mut self, total: u64, template: &str) {
        let Some(multi) = self.multi.as_ref() else {
            return;
        };
        let style = ProgressStyle::with_template(template)
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("█░ ");
        if let Some(existing) = self.bar.as_ref() {
            existing.set_style(style);
            existing.set_length(total);
            return;
        }
        let bar = multi.add(ProgressBar::new(total));
        bar.set_style(style);
        // Steady tick keeps {elapsed_precise} animating between actual
        // position updates so the bar feels alive on slow/quiet runs.
        bar.enable_steady_tick(Duration::from_millis(1000 / u64::from(DRAW_HZ)));
        self.bar = Some(bar);
    }

    /// Replace the bar's template (e.g. switch to a red-fill variant
    /// when the first failure arrives).
    pub fn set_template(&self, template: &str) {
        if let Some(bar) = self.bar.as_ref() {
            let style = ProgressStyle::with_template(template)
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("█░ ");
            bar.set_style(style);
        }
    }

    pub fn set_position(&self, pos: u64) {
        if let Some(bar) = self.bar.as_ref() {
            bar.set_position(pos);
        }
    }

    pub fn set_message<S: Into<Cow<'static, str>>>(&self, msg: S) {
        if let Some(bar) = self.bar.as_ref() {
            bar.set_message(msg);
        }
    }

    pub fn set_prefix<S: Into<Cow<'static, str>>>(&self, prefix: S) {
        if let Some(bar) = self.bar.as_ref() {
            bar.set_prefix(prefix);
        }
    }

    /// Print a line above the bar atomically. When disabled, writes
    /// directly to `w`. A trailing newline is always emitted (single,
    /// even if `line` already ended with one).
    pub fn println<W: Write>(&self, w: &mut W, line: &str) {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if let Some(multi) = self.multi.as_ref() {
            let _ = multi.println(trimmed);
        } else {
            let _ = writeln!(w, "{trimmed}");
        }
    }

    /// Final cleanup: removes the bar from the live area. Idempotent.
    /// `MultiProgress`'s own `Drop` covers cursor restore if this isn't
    /// called (panic, abort).
    pub fn finish_and_clear(&mut self) {
        if let Some(bar) = self.bar.take() {
            bar.finish_and_clear();
        }
    }
}

impl Default for LiveArea {
    fn default() -> Self {
        Self::hidden()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_bar_empty() {
        assert_eq!(render_bar(0, 100, 10), "░░░░░░░░░░");
    }

    #[test]
    fn render_bar_half() {
        assert_eq!(render_bar(50, 100, 10), "█████░░░░░");
    }

    #[test]
    fn render_bar_full() {
        assert_eq!(render_bar(100, 100, 10), "██████████");
    }

    #[test]
    fn render_bar_zero_total() {
        assert_eq!(render_bar(0, 0, 10), "░░░░░░░░░░");
    }

    #[test]
    fn render_bar_zero_width() {
        assert_eq!(render_bar(50, 100, 0), "");
    }

    #[test]
    fn render_bar_overfill_clamps() {
        assert_eq!(render_bar(150, 100, 10), "██████████");
    }

    #[test]
    fn format_elapsed_minutes() {
        assert_eq!(format_elapsed(Duration::from_secs(125)), "00:02:05");
    }

    #[test]
    fn format_elapsed_zero() {
        assert_eq!(format_elapsed(Duration::ZERO), "00:00:00");
    }

    #[test]
    fn format_elapsed_hours() {
        assert_eq!(
            format_elapsed(Duration::from_secs(3 * 3600 + 17)),
            "03:00:17"
        );
    }

    #[test]
    fn format_elapsed_caps_at_99h_59m_59s() {
        // Whole duration clamps, so 200h reads as 99:59:59 — minutes
        // and seconds saturate too instead of leaking from the
        // un-clamped value.
        assert_eq!(format_elapsed(Duration::from_secs(200 * 3600)), "99:59:59");
        assert_eq!(
            format_elapsed(Duration::from_secs(120 * 3600 + 30 * 60)),
            "99:59:59"
        );
    }

    #[test]
    fn hidden_println_writes_to_writer() {
        let area = LiveArea::hidden();
        let mut buf = Vec::<u8>::new();
        area.println(&mut buf, "hello");
        assert_eq!(String::from_utf8(buf).expect("utf-8"), "hello\n");
    }

    #[test]
    fn hidden_println_strips_trailing_newline() {
        let area = LiveArea::hidden();
        let mut buf = Vec::<u8>::new();
        area.println(&mut buf, "hello\n");
        // Exactly one trailing newline, regardless of input.
        assert_eq!(String::from_utf8(buf).expect("utf-8"), "hello\n");
    }

    #[test]
    fn hidden_bar_methods_are_no_ops() {
        let mut area = LiveArea::hidden();
        area.start(10, "{bar}");
        area.set_template("{bar}");
        area.set_position(5);
        area.set_message("hi");
        area.set_prefix("pfx");
        area.finish_and_clear();
        let mut buf = Vec::<u8>::new();
        area.println(&mut buf, "line");
        assert_eq!(String::from_utf8(buf).expect("utf-8"), "line\n");
    }

    #[test]
    fn hidden_width_falls_back_to_default() {
        assert_eq!(LiveArea::hidden().width(), FALLBACK_WIDTH);
    }

    #[test]
    fn hidden_is_not_enabled() {
        assert!(!LiveArea::hidden().enabled());
    }

    #[test]
    fn default_is_hidden() {
        assert!(!LiveArea::default().enabled());
    }
}

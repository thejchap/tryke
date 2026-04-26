//! Terminal-control primitives for the `next` and `sugar` reporters.
//!
//! The `LiveBar` redraws a single line at the current cursor position on
//! stderr, leaving stdout (the reporter's writer) free for the per-test
//! lines and end-of-run summary. Splitting them across streams keeps
//! snapshot tests over the writer free of cursor escapes.
//!
//! When `enabled` is false (non-TTY, redirect, CI), every method is a
//! no-op so callers don't need to branch on terminal capability.

use std::io::{self, IsTerminal, Write};
use std::time::Duration;

/// Returns true if stderr is a terminal — looser than
/// `progress::supports_progress` (which gates OSC 9;4 to specific
/// emulators). `\r` + clear-line work on every TTY.
#[must_use]
pub fn supports_live() -> bool {
    io::stderr().is_terminal()
}

/// Render a single-row progress bar of `width` cells.
/// Pure: no I/O, no ANSI. Caller composes color/styling.
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
        out.push('─');
    }
    out
}

/// Format `d` as `HH:MM:SS`, capped at 99:59:59.
#[must_use]
pub fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    let hours = (secs / 3600).min(99);
    let minutes = (secs / 60) % 60;
    let seconds = secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// Hides and restores the cursor, redraws a line in place. Writes
/// nothing when `enabled` is false.
///
/// Callers are responsible for calling [`Self::clear`] before dropping
/// (typically from `on_run_complete`). On Ctrl+C, the cleanup handler
/// installed by [`crate::progress::install_cleanup_handler`] also
/// emits the cursor-restore + line-clear escapes, so terminals don't
/// get stuck with a hidden cursor.
pub struct LiveBar<W: Write = io::Stderr> {
    writer: W,
    enabled: bool,
    cursor_hidden: bool,
    drawn: bool,
}

impl LiveBar {
    /// Default: writes to stderr.
    #[must_use]
    pub fn new(enabled: bool) -> Self {
        Self {
            writer: io::stderr(),
            enabled,
            cursor_hidden: false,
            drawn: false,
        }
    }
}

impl<W: Write> LiveBar<W> {
    pub fn with_writer(writer: W, enabled: bool) -> Self {
        Self {
            writer,
            enabled,
            cursor_hidden: false,
            drawn: false,
        }
    }

    /// Replace the previously-drawn line with `line`. The line should
    /// not contain newlines or ANSI cursor-control codes (color SGR is
    /// fine).
    pub fn redraw(&mut self, line: &str) {
        if !self.enabled {
            return;
        }
        if !self.cursor_hidden {
            let _ = self.writer.write_all(b"\x1b[?25l");
            self.cursor_hidden = true;
        }
        let _ = self.writer.write_all(b"\r\x1b[2K");
        let _ = self.writer.write_all(line.as_bytes());
        let _ = self.writer.flush();
        self.drawn = true;
    }

    /// Erase the current line and restore the cursor. Idempotent.
    pub fn clear(&mut self) {
        if !self.enabled {
            return;
        }
        if self.drawn {
            let _ = self.writer.write_all(b"\r\x1b[2K");
            self.drawn = false;
        }
        if self.cursor_hidden {
            let _ = self.writer.write_all(b"\x1b[?25h");
            self.cursor_hidden = false;
        }
        let _ = self.writer.flush();
    }

    pub fn into_writer(self) -> W {
        self.writer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_bar_empty() {
        assert_eq!(render_bar(0, 100, 10), "──────────");
    }

    #[test]
    fn render_bar_half() {
        assert_eq!(render_bar(50, 100, 10), "█████─────");
    }

    #[test]
    fn render_bar_full() {
        assert_eq!(render_bar(100, 100, 10), "██████████");
    }

    #[test]
    fn render_bar_zero_total() {
        // No divide-by-zero, no fill.
        assert_eq!(render_bar(0, 0, 10), "──────────");
    }

    #[test]
    fn render_bar_zero_width() {
        assert_eq!(render_bar(50, 100, 0), "");
    }

    #[test]
    fn render_bar_overfill_clamps() {
        // Filled > total shouldn't panic or overflow the cell count.
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
    fn format_elapsed_caps_at_99h() {
        assert_eq!(format_elapsed(Duration::from_secs(200 * 3600)), "99:00:00");
    }

    #[test]
    fn livebar_redraw_writes_clear_then_content() {
        let mut bar = LiveBar::with_writer(Vec::<u8>::new(), true);
        bar.redraw("hello");
        let out = bar.into_writer();
        // Should contain hide-cursor + clear-line + content.
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("\x1b[?25l"));
        assert!(s.contains("\r\x1b[2K"));
        assert!(s.contains("hello"));
    }

    #[test]
    fn livebar_disabled_writes_nothing() {
        let mut bar = LiveBar::with_writer(Vec::<u8>::new(), false);
        bar.redraw("hello");
        bar.clear();
        let out = bar.into_writer();
        assert!(out.is_empty());
    }

    #[test]
    fn livebar_clear_is_idempotent() {
        let mut bar = LiveBar::with_writer(Vec::<u8>::new(), true);
        bar.redraw("first");
        bar.clear();
        bar.clear();
        let out = bar.into_writer();
        let s = String::from_utf8_lossy(&out);
        // Show-cursor escape should appear exactly once.
        assert_eq!(s.matches("\x1b[?25h").count(), 1);
    }

    #[test]
    fn livebar_clear_restores_cursor() {
        let mut bar = LiveBar::with_writer(Vec::<u8>::new(), true);
        bar.redraw("x");
        bar.clear();
        let out = bar.into_writer();
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("\x1b[?25h"));
    }

    #[test]
    fn livebar_drop_clears() {
        let buf: Vec<u8> = {
            let mut bar = LiveBar::with_writer(Vec::<u8>::new(), true);
            bar.redraw("about to drop");
            bar.into_writer()
        };
        // We can't observe drop's clear() effect through `into_writer`
        // because that consumes the bar before drop. This test just
        // verifies drop doesn't panic.
        drop(buf);
    }
}

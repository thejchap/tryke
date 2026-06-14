use std::borrow::Cow;
use std::io::Write;
use std::time::Duration;

const FALLBACK_WIDTH: usize = 80;

#[must_use]
pub fn supports_live() -> bool {
    false
}

#[must_use]
pub fn format_elapsed(d: Duration) -> String {
    const MAX_SECS: u64 = 99 * 3600 + 59 * 60 + 59;
    let secs = d.as_secs().min(MAX_SECS);
    let hours = secs / 3600;
    let minutes = (secs / 60) % 60;
    let seconds = secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

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

pub struct LiveArea {
    enabled: bool,
    width: usize,
}

impl LiveArea {
    #[must_use]
    pub fn new() -> Self {
        Self::hidden()
    }

    #[must_use]
    pub fn hidden() -> Self {
        Self {
            enabled: false,
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

    pub fn start(&mut self, total: u64, template: &str) {
        let _ = (total, template);
    }

    pub fn set_template(&self, template: &str) {
        let _ = template;
    }

    pub fn set_position(&self, pos: u64) {
        let _ = pos;
    }

    pub fn set_message<S: Into<Cow<'static, str>>>(&self, msg: S) {
        let _ = msg;
    }

    pub fn set_prefix<S: Into<Cow<'static, str>>>(&self, prefix: S) {
        let _ = prefix;
    }

    pub fn println<W: Write>(&self, w: &mut W, line: &str) {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        let _ = writeln!(w, "{trimmed}");
    }

    pub fn finish_and_clear(&mut self) {}
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
}

use std::time::Duration;

/// Format a duration for human-facing reporter output.
///
/// Sub-second durations render as milliseconds (`48.00ms`),
/// sub-minute as seconds (`1.50s`), and anything longer as
/// `m:ss.cc` (`1:05.50`). Centiseconds are rounded — not
/// truncated — so 119.999s carries up to `2:00.00`.
#[must_use]
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        format!("{:.2}ms", secs * 1000.0)
    } else if secs < 60.0 {
        format!("{secs:.2}s")
    } else {
        // Integer math avoids the float-cast precision lint. The `+ 5`
        // rounds to the nearest centisecond so output matches the
        // sub-minute branches (which round via `:.2`); collapsing
        // everything into centiseconds first lets carry through
        // seconds/minutes happen naturally on decomposition.
        let centis = (d.as_millis() + 5) / 10;
        let minutes = centis / 6000;
        let secs = (centis / 100) % 60;
        let cs = centis % 100;
        format!("{minutes}:{secs:02}.{cs:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_millisecond() {
        assert_eq!(format_duration(Duration::from_micros(170)), "0.17ms");
    }

    #[test]
    fn milliseconds() {
        assert_eq!(format_duration(Duration::from_millis(48)), "48.00ms");
    }

    #[test]
    fn seconds() {
        assert_eq!(format_duration(Duration::from_millis(1_500)), "1.50s");
    }

    #[test]
    fn minutes_seconds() {
        assert_eq!(format_duration(Duration::from_millis(65_500)), "1:05.50");
    }

    #[test]
    fn exactly_one_minute() {
        assert_eq!(format_duration(Duration::from_secs(60)), "1:00.00");
    }

    #[test]
    fn rounds_with_carry_through_minutes() {
        assert_eq!(format_duration(Duration::from_millis(119_999)), "2:00.00");
    }
}

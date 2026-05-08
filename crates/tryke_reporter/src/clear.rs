use std::io::IsTerminal;

/// Whether stdout is a real terminal we can safely send a clear
/// sequence to. Captured at reporter construction time and stored on
/// the reporter so that reporters writing to a non-stdout target
/// (e.g. `with_writer(Vec<u8>)` for tests, or any other captured
/// writer) never trip the clear.
#[must_use]
pub fn stdout_is_terminal() -> bool {
    std::io::stdout().is_terminal()
}

/// Clear the terminal screen unconditionally. Callers must check
/// their own "should clear" gate (typically `clear_enabled` cached at
/// construction time) — this function does not consult stdout's TTY
/// status.
pub fn clear_terminal() {
    let _ = clearscreen::clear();
}

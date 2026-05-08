use std::io::IsTerminal;

/// Clear the terminal screen if stdout is a TTY. Used by reporters
/// that defer the watch-mode clear to the moment results begin
/// streaming, so the user doesn't stare at a blank screen during the
/// (potentially slow) discovery + worker-warmup phase between a save
/// and the first new test event.
pub fn clear_terminal_if_tty() {
    if std::io::stdout().is_terminal() {
        let _ = clearscreen::clear();
    }
}

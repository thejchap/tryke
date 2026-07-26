use std::io::{self, Write};

use console::style;
use tryke::ExitStatus;

fn main() -> ExitStatus {
    tryke::run().unwrap_or_else(|error| {
        // Exit gracefully when output is piped to a process that closes early.
        if error.chain().any(|cause| {
            cause
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
        }) {
            return ExitStatus::Success;
        }

        // Avoid panicking if writing the error itself fails.
        let mut stderr = io::stderr().lock();
        let _ = writeln!(
            stderr,
            "{}",
            style("tryke failed").red().bold().for_stderr()
        );
        for cause in error.chain() {
            let _ = writeln!(stderr, "  {} {cause}", style("Cause:").bold().for_stderr());
        }

        ExitStatus::Error
    })
}

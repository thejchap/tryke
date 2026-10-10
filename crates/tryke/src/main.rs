use std::io::Write as _;

fn main() -> tryke::ExitStatus {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "tryke failed\n  Cause: {error}");
            return tryke::ExitStatus::Error;
        }
    };
    let status = runtime.block_on(tryke::run());

    // Commands have already shut down their workers and server tasks. Do not
    // wait for uncancellable stdin reads or interrupted blocking discovery.
    runtime.shutdown_background();
    status
}

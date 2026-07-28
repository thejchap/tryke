mod cli;
mod commands;
mod logging;

use std::{
    io::{self, Write},
    process::{ExitCode, Termination},
};

use clap::{CommandFactory, Parser};
use console::style;
use log::debug;
use tokio_util::sync::CancellationToken;

use cli::{Cli, Commands};
use commands::{
    CommandOrigin, run_clean_command, run_graph_command, run_server_command, run_test_command,
};
use logging::LogConfig;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitStatus {
    /// Test run was successful and there were no failures.
    Success = 0,

    /// Test run was successful but there were failures.
    Failure = 1,

    /// Test run failed due to an invocation error.
    Error = 2,

    /// Test run was interrupted by Ctrl+C.
    Interrupted = 130,
}

impl Termination for ExitStatus {
    fn report(self) -> ExitCode {
        ExitCode::from(self as u8)
    }
}

pub async fn run() -> ExitStatus {
    try_run().await.unwrap_or_else(report_error)
}

async fn try_run() -> anyhow::Result<ExitStatus> {
    let cli = Cli::parse();
    let logging = LogConfig::from_env(cli.global.verbose.log_level_filter())?;
    logging.init_rust_logging();

    debug!("{cli:?}");

    let origin = if cli.command.is_some() {
        CommandOrigin::Explicit
    } else {
        CommandOrigin::Bare
    };

    let command = cli.command.unwrap_or_else(Commands::default_watch);
    let global = cli.global;

    match command {
        Commands::Test(args) => {
            let cancellation = CancellationToken::new();
            let command = run_test_command(args, &global, origin, logging, cancellation.clone());
            tokio::pin!(command);

            tokio::select! {
                biased;
                signal = tokio::signal::ctrl_c() => {
                    signal?;
                    cancellation.cancel();
                    command.await?;

                    Ok(ExitStatus::Interrupted)
                }
                result = &mut command => result,
            }
        }
        Commands::Server(args) => {
            let cancellation = CancellationToken::new();
            let command = run_server_command(args, &global, logging, cancellation.clone());
            tokio::pin!(command);

            tokio::select! {
                biased;
                signal = tokio::signal::ctrl_c() => {
                    signal?;
                    cancellation.cancel();
                    command.await?;

                    Ok(ExitStatus::Interrupted)
                }
                result = &mut command => result,
            }
        }
        Commands::Clean(args) => run_clean_command(args, &global),
        Commands::Graph(args) => run_graph_command(args, &global),
    }
}

fn report_error(error: anyhow::Error) -> ExitStatus {
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
}

/// Exported for use by `tryke_dev` CLI doc generation.
#[doc(hidden)]
#[must_use]
pub fn cli_command() -> clap::Command {
    Cli::command()
}

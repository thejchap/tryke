mod cli;
mod commands;

use std::env;
use std::process::{ExitCode, Termination};

use clap::{CommandFactory, Parser};
use log::debug;

use cli::{Cli, Commands};
use commands::{
    CommandOrigin, run_clean_command, run_graph_command, run_server_command, run_test_command,
};

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

pub fn run() -> anyhow::Result<ExitStatus> {
    let cli = Cli::parse();
    let cli_filter = cli.global.verbose.log_level_filter();
    let tryke_log = env::var("TRYKE_LOG").ok();
    let rust_default = tryke_config::rust_log_default(tryke_log.as_deref(), cli_filter);

    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(rust_default.as_str().to_ascii_lowercase()),
    )
    .init();

    debug!("{cli:?}");

    let origin = if cli.command.is_some() {
        CommandOrigin::Explicit
    } else {
        CommandOrigin::Bare
    };

    let command = cli.command.unwrap_or_else(Commands::default_watch);
    let global = cli.global;

    match command {
        Commands::Test(args) => run_test_command(args, &global, origin),
        Commands::Server(args) => run_server_command(args, &global),
        Commands::Clean(args) => run_clean_command(args, &global),
        Commands::Graph(args) => run_graph_command(args, &global),
    }
}

/// Exported for use by `tryke_dev` CLI doc generation.
#[doc(hidden)]
#[must_use]
pub fn cli_command() -> clap::Command {
    Cli::command()
}

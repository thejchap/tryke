use std::env;

use anyhow::Result;
use tokio_util::sync::CancellationToken;
use tryke_config::{Project, ProjectMetadata};
use tryke_discovery::Discoverer;
use tryke_runner::{WorkerPool, WorkerPoolOptions};
use tryke_server::Server;

use crate::ExitStatus;
use crate::cli::{GlobalArgs, ServerArgs};
use crate::logging::LogConfig;

pub(crate) async fn run_server_command(
    args: ServerArgs,
    global: &GlobalArgs,
    logging: LogConfig,
    cancellation: CancellationToken,
) -> Result<ExitStatus> {
    let cwd = env::current_dir()?;

    let mut metadata = ProjectMetadata::new(args.root.as_deref().unwrap_or(&cwd));
    metadata.apply_configuration_file();
    metadata.apply_cli_args(args.project_options(global));
    let project = Project::from_metadata(metadata);

    let worker_pool = WorkerPool::spawn(
        &project,
        WorkerPoolOptions {
            size: args.workers,
            python_path: None,
            log_level: logging.level(),
            warm: false,
        },
    )
    .await;

    let discoverer = Discoverer::new(&project);
    let server = Server::new(worker_pool, discoverer);

    server.serve_with_cancellation(cancellation).await?;

    Ok(ExitStatus::Success)
}

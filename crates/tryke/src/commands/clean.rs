use std::env;

use anyhow::Result;
use tryke_config::Project;

use crate::ExitStatus;
use crate::cli::{CleanArgs, GlobalArgs};

pub(crate) fn run_clean_command(args: CleanArgs, global: &GlobalArgs) -> Result<ExitStatus> {
    let cwd = env::current_dir()?;
    let project = Project::load(
        args.root.as_deref().unwrap_or(&cwd),
        global.config_file.as_deref(),
        args.project_options(global),
    )?;
    let report = tryke_discovery::clean_project_cache(&project)?;

    if report.removed_entries == 0 {
        println!(
            "No tryke discovery cache found at {}",
            report.cache_dir.display()
        );
    } else {
        println!(
            "Cleaned tryke discovery cache at {}",
            report.cache_dir.display()
        );
    }
    Ok(ExitStatus::Success)
}

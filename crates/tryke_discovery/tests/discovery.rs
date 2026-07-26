/// Snapshot test discovery for fixtures under tests/resources/collect/
use std::path::Path;

use tryke_discovery::{Discoverer, DiscoveryOptions};
use tryke_reporter::{JSONReporter, Reporter};
use tryke_testing::TestProject;

const FIXTURE_ROOT: &str = "tests/resources/collect";

fn collect_fixture(path: &Path, source: String) -> datatest_stable::Result<()> {
    let relative_path = path.strip_prefix(FIXTURE_ROOT)?;
    let project = TestProject::with_files([(relative_path, source)])?;
    let mut discoverer = Discoverer::new(&project.project());
    let selection = discoverer.discover(DiscoveryOptions::default());

    let mut reporter = JSONReporter::with_writer(Vec::new());
    for warning in &selection.warnings {
        reporter.on_discovery_warning(warning);
    }
    reporter.on_collect_complete(&selection.tests);

    let output = serde_json::from_slice::<serde_json::Value>(&reporter.into_writer())?;
    let snapshot_name = relative_path
        .with_extension("")
        .to_string_lossy()
        .replace(['/', '\\'], "__");
    let mut settings = insta::Settings::clone_current();
    settings.set_input_file(path);
    settings.bind(|| insta::assert_json_snapshot!(snapshot_name, output));
    Ok(())
}

datatest_stable::harness! {
    {
        test = collect_fixture,
        root = FIXTURE_ROOT,
        pattern = r"^.*\.py$",
    },
}

use std::collections::BTreeSet;
use std::process::Command;

use tryke_testing::TestProject;
use tryke_types::TestItem;

const EDITOR_FIXTURE: &str = r#"from tryke import test


@test
def first():
    pass


@test
def second():
    pass


@test.cases(test.case("sp ace", value=1), test.case("other", value=2))
def param(value):
    pass
"#;

/// Run `tryke test <args> --collect-only --reporter json` against `project`
/// and return the identities from its `collect_complete` event.
fn collect_ids(
    project: &TestProject,
    args: &[&str],
) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_tryke"))
        .arg("test")
        .args(args)
        .arg("--root")
        .arg(project.root())
        .args(["--python", &tryke_testing::python_bin()])
        .args(["--collect-only", "--reporter", "json"])
        .output()?;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert_eq!(output.status.code(), Some(0), "{stderr}\n{stdout}");
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let mut event: serde_json::Value = serde_json::from_str(line)?;
        if event["event"] == "collect_complete" {
            let tests: Vec<TestItem> = serde_json::from_value(event["tests"].take())?;
            return Ok(tests.iter().map(TestItem::id).collect());
        }
    }
    Err(format!("no collect_complete event in stdout: {stdout}").into())
}

#[test]
fn empty_path_argument_collects_whole_project() -> Result<(), Box<dyn std::error::Error>> {
    let project = TestProject::with_files([("test_editor.py", EDITOR_FIXTURE)])?;
    let ids = collect_ids(&project, &[""])?;
    let expected: BTreeSet<String> = [
        "test_editor.py::first",
        "test_editor.py::second",
        "test_editor.py::param[sp ace]",
        "test_editor.py::param[other]",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(ids, expected);
    Ok(())
}

#[test]
fn keyword_with_spaced_case_label_selects_exact_case() -> Result<(), Box<dyn std::error::Error>> {
    let project = TestProject::with_files([("test_editor.py", EDITOR_FIXTURE)])?;
    let ids = collect_ids(&project, &["-k", "param[sp ace]"])?;
    let expected: BTreeSet<String> =
        std::iter::once("test_editor.py::param[sp ace]".to_owned()).collect();
    assert_eq!(ids, expected);
    Ok(())
}

#[test]
fn post_parse_errors_use_invocation_error_status() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_tryke"))
        .args(["test", "-k", "("])
        .output()?;
    let stderr = String::from_utf8(output.stderr)?;

    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(output.stdout.is_empty(), "unexpected stdout");
    assert!(stderr.contains("tryke failed"), "{stderr}");
    assert!(
        stderr.contains("invalid filter expression: unexpected end of expression"),
        "{stderr}"
    );
    Ok(())
}

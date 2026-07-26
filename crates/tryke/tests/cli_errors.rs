use std::process::Command;

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
        stderr.contains("Cause: invalid filter expression: unexpected end of expression"),
        "{stderr}"
    );
    Ok(())
}

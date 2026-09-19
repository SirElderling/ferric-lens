use std::process::Command;

use super::{parse_rustc_host_output, ProfileContext};

#[test]
fn normalizes_explicit_profile_inputs() {
    let features = vec![" z ".into(), "a".into(), "a".into(), " ".into()];
    let profile = ProfileContext::resolve(Some(" x86_64-unknown-linux-gnu "), &features).unwrap();

    assert_eq!(profile.public.target, "x86_64-unknown-linux-gnu");
    assert_eq!(profile.public.features, ["a", "z"]);
    assert!(profile.public.id.contains("target=x86_64-unknown-linux-gnu"));
    assert!(profile.public.id.contains("default+a,z"));
}

#[test]
fn rustc_host_parser_reports_command_failure_and_missing_host() {
    let failed = Command::new("rustc")
        .arg("--definitely-invalid-ferric-lens-option")
        .output()
        .unwrap();
    assert!(parse_rustc_host_output(&failed).is_err());

    let version_only = Command::new("rustc").arg("--version").output().unwrap();
    assert!(parse_rustc_host_output(&version_only)
        .unwrap_err()
        .contains("did not report a host target"));
}

#[cfg(unix)]
#[test]
fn rustc_host_parser_reports_empty_stderr_failure_status() {
    use std::os::unix::process::ExitStatusExt;

    let output = std::process::Output {
        status: std::process::ExitStatus::from_raw(1 << 8),
        stdout: Vec::new(),
        stderr: Vec::new(),
    };

    let error = parse_rustc_host_output(&output).unwrap_err();
    assert!(error.contains("failed with status"));
}

#[test]
fn rustc_host_command_reports_spawn_errors() {
    let missing = std::env::temp_dir().join(format!(
        "ferric-lens-host-missing-cwd-{}",
        std::process::id()
    ));
    let mut command = Command::new("rustc");
    command.current_dir(missing).arg("-vV");

    assert!(super::run_rustc_host(&mut command)
        .unwrap_err()
        .contains("could not execute rustc"));
}

#[test]
fn profile_resolution_reports_invalid_explicit_targets() {
    let error = ProfileContext::resolve(Some("ferric-lens-invalid-target"), &[]).unwrap_err();

    assert!(!error.is_empty());
}

#[test]
fn rustc_host_parse_wrapper_propagates_spawn_errors() {
    let missing = std::env::temp_dir().join(format!(
        "ferric-lens-host-parse-missing-cwd-{}",
        std::process::id()
    ));
    let mut command = Command::new("rustc");
    command.current_dir(missing).arg("-vV");

    assert!(super::run_rustc_host_and_parse(&mut command)
        .unwrap_err()
        .contains("could not execute rustc"));
}

#[test]
fn profile_resolution_propagates_host_lookup_failures() {
    let error = ProfileContext::resolve_with_host(None, &[], || Err("host unavailable".into()))
        .unwrap_err();
    assert_eq!(error, "host unavailable");
}

#[test]
fn profile_resolution_propagates_host_lookup_failure_without_an_explicit_target() {
    let error = ProfileContext::resolve_with_host(None, &[], || Err("fixture host failure".into()))
        .unwrap_err();

    assert_eq!(error, "fixture host failure");
}

use std::process::Command;

use super::{parse_rustc_host_output, ProfileContext};

#[test]
fn normalizes_explicit_profile_inputs() {
    let features = vec![" z ".into(), "a".into(), "a".into(), " ".into()];
    let profile = ProfileContext::resolve(Some(" x86_64-unknown-linux-gnu "), &features).unwrap();

    assert_eq!(profile.public.target, "x86_64-unknown-linux-gnu");
    assert_eq!(profile.public.features, ["a", "z"]);
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

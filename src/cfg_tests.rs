impl HostCfg {
    pub fn test(lines: &[&str]) -> Self {
        Self::from_lines(lines.iter().copied()).unwrap()
    }

    pub fn test_with_features(lines: &[&str], features: &[&str]) -> Self {
        let mut cfg = Self::from_lines(lines.iter().copied()).unwrap();
        cfg.explicit_features = features
            .iter()
            .map(|feature| (*feature).to_owned())
            .collect();
        cfg.rehash();
        cfg
    }
}

use syn::parse_quote;

use super::{HostCfg, Truth};

fn linux() -> HostCfg {
    HostCfg::test(&[
        "unix",
        "target_arch=\"x86_64\"",
        "target_family=\"unix\"",
        "target_os=\"linux\"",
        "target_pointer_width=\"64\"",
    ])
}

#[test]
fn evaluates_standard_host_predicates() {
    let cfg = linux();
    assert_eq!(cfg.evaluate(&parse_quote!(unix)), Truth::True);
    assert_eq!(
        cfg.evaluate(&parse_quote!(target_os = "linux")),
        Truth::True
    );
    assert_eq!(
        cfg.evaluate(&parse_quote!(target_os = "macos")),
        Truth::False
    );
    assert_eq!(cfg.evaluate(&parse_quote!(windows)), Truth::False);
    assert_eq!(cfg.evaluate(&parse_quote!(test)), Truth::False);
}

#[test]
fn evaluates_boolean_cfg_combinators() {
    let cfg = linux();
    assert_eq!(
        cfg.evaluate(&parse_quote!(all(unix, not(windows)))),
        Truth::True
    );
    assert_eq!(cfg.evaluate(&parse_quote!(not(unix))), Truth::False);
    assert_eq!(
        cfg.evaluate(&parse_quote!(any(target_os = "macos", windows))),
        Truth::False
    );
}

#[test]
fn preserves_unknown_custom_and_unselected_feature_cfg() {
    let cfg = linux();
    assert_eq!(
        cfg.evaluate(&parse_quote!(feature = "fast")),
        Truth::Unknown
    );
    assert_eq!(cfg.evaluate(&parse_quote!(my_custom_cfg)), Truth::Unknown);
    assert_eq!(
        cfg.evaluate(&parse_quote!(any(my_custom_cfg, windows))),
        Truth::Unknown
    );
}

#[test]
fn selected_explicit_feature_is_true() {
    let cfg = HostCfg::test_with_features(&["unix", "target_os=\"linux\""], &["fast"]);
    assert_eq!(cfg.evaluate(&parse_quote!(feature = "fast")), Truth::True);
    assert_eq!(
        cfg.evaluate(&parse_quote!(feature = "other")),
        Truth::Unknown
    );
}

#[test]
fn detects_host_cfg_and_rejects_an_invalid_target() {
    let detected = HostCfg::detect().unwrap();
    assert!(!detected.digest().is_empty());
    assert!(!detected.canonical_lines().is_empty());

    let error = HostCfg::detect_for_target(Some("ferric-lens-invalid-target"), &[]).unwrap_err();
    assert!(!error.is_empty());
}

#[test]
fn unavailable_cfg_never_invents_non_test_truth() {
    let cfg = HostCfg::unavailable();
    assert_eq!(cfg.digest(), "cfg-unavailable");
    assert!(cfg.canonical_lines().is_empty());
    assert_eq!(cfg.evaluate(&parse_quote!(test)), Truth::False);
    assert_eq!(cfg.evaluate(&parse_quote!(unix)), Truth::Unknown);
}

#[test]
fn malformed_or_semantically_unknown_meta_is_unknown() {
    let cfg = linux();

    assert_eq!(cfg.evaluate(&parse_quote!(foo::bar)), Truth::Unknown);
    assert_eq!(
        cfg.evaluate(&parse_quote!(foo::bar = "value")),
        Truth::Unknown
    );
    assert_eq!(cfg.evaluate(&parse_quote!(feature = SOME)), Truth::Unknown);
    assert_eq!(
        cfg.evaluate(&parse_quote!(target_os = SOME)),
        Truth::Unknown
    );
    assert_eq!(cfg.evaluate(&parse_quote!(foo::bar(unix))), Truth::Unknown);

    let malformed: syn::Meta = syn::parse_str("all(@)").unwrap();
    assert_eq!(cfg.evaluate(&malformed), Truth::Unknown);
    assert_eq!(cfg.evaluate(&parse_quote!(xor(unix))), Truth::Unknown);
    assert_eq!(
        cfg.evaluate(&parse_quote!(not(unix, windows))),
        Truth::Unknown
    );
}

#[test]
fn boolean_cfg_operators_cover_true_false_and_unknown_paths() {
    let cfg = linux();

    assert_eq!(
        cfg.evaluate(&parse_quote!(all(unix, windows))),
        Truth::False
    );
    assert_eq!(
        cfg.evaluate(&parse_quote!(all(unix, my_custom_cfg))),
        Truth::Unknown
    );
    assert_eq!(cfg.evaluate(&parse_quote!(any(windows, unix))), Truth::True);
    assert_eq!(
        cfg.evaluate(&parse_quote!(not(my_custom_cfg))),
        Truth::Unknown
    );
}

#[test]
fn parses_cfg_lines_canonically_and_rejects_unquoted_values() {
    let cfg = HostCfg::from_lines([
        "",
        "unix",
        "target_os=\"linux\"",
        "target_feature=\"sse2\"",
        "target_feature=\"avx\"",
    ])
    .unwrap();

    assert_eq!(
        cfg.canonical_lines(),
        [
            "target_feature=\"avx\"",
            "target_feature=\"sse2\"",
            "target_os=\"linux\"",
            "unix",
        ]
    );
    assert!(HostCfg::from_lines(["target_os=linux"]).is_err());
}

#[test]
fn recognizes_every_builtin_cfg_family() {
    for key in [
        "unix",
        "windows",
        "debug_assertions",
        "proc_macro",
        "target_thread_local",
        "doctest",
    ] {
        assert!(super::is_known_bare_cfg(key));
    }
    assert!(!super::is_known_bare_cfg("custom"));

    for key in [
        "target_arch",
        "target_endian",
        "target_env",
        "target_family",
        "target_feature",
        "target_has_atomic",
        "target_os",
        "target_pointer_width",
        "target_vendor",
        "panic",
    ] {
        assert!(super::is_known_value_cfg(key));
    }
    assert!(!super::is_known_value_cfg("custom"));
}

#[test]
fn missing_known_value_cfg_is_false_while_missing_custom_value_is_unknown() {
    let cfg = HostCfg::test(&["unix"]);

    assert_eq!(
        cfg.evaluate(&parse_quote!(target_arch = "x86_64")),
        Truth::False
    );
    assert_eq!(
        cfg.evaluate(&parse_quote!(custom_value = "x")),
        Truth::Unknown
    );
}\n
#[test]
fn rustc_cfg_command_and_parser_report_spawn_and_utf8_errors() {
    let missing = std::env::temp_dir().join(format!(
        "ferric-lens-cfg-missing-cwd-{}",
        std::process::id()
    ));
    let mut command = std::process::Command::new("rustc");
    command.current_dir(missing).args(["--print", "cfg"]);
    assert!(super::run_rustc_cfg(&mut command)
        .unwrap_err()
        .contains("could not execute"));

    assert!(super::parse_rustc_cfg(&[0xff])
        .unwrap_err()
        .contains("not UTF-8"));
}

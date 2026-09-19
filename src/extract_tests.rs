use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{extract, resolve_workspace_dependencies};
use crate::{
    cfg::HostCfg,
    input::{SourceFile, WorkspaceAliases},
    model::{
        DeltaStatus, Evidence, EvidenceClass, Finding, ModuleMetrics, Priority, SourceContext,
    },
};

fn host() -> HostCfg {
    HostCfg::test(&["unix", "target_family=\"unix\"", "target_os=\"linux\""])
}

fn source(text: &str) -> SourceFile {
    SourceFile {
        crate_name: "demo".into(),
        module_path: "engine".into(),
        relative_path: "src/engine.rs".into(),
        bytes: text.as_bytes().to_vec(),
    }
}

#[test]
fn counts_inline_module_decision_sites_in_containing_file() {
    let metrics = extract(
        &source(
            r#"
        pub fn run(x: bool) {
            if x && true { loop { break; } }
            match x { true => (), _ => () }
        }
        mod child { fn hidden() { if true {} } }
        "#,
        ),
        &host(),
    )
    .unwrap();

    assert_eq!(metrics.decision_sites, 5);
    assert_eq!(metrics.public_items, 1);
}

#[test]
fn ignores_exact_cfg_test_items_for_production_metrics() {
    let metrics = extract(
        &source(
            r#"
        fn production() { if true {} }
        #[cfg(test)]
        fn only_test() { if true {} }
        "#,
        ),
        &host(),
    )
    .unwrap();

    assert_eq!(metrics.decision_sites, 1);
    assert!(metrics.gate_complete);
}

#[test]
fn marks_unknown_cfg_and_macros_incomplete_for_gating() {
    let cfg_metrics = extract(&source("#[cfg(my_custom_cfg)] fn platform() {}"), &host()).unwrap();
    assert!(!cfg_metrics.gate_complete);

    let macro_metrics = extract(&source("fn f() { vec![1, 2, 3]; }"), &host()).unwrap();
    assert!(!macro_metrics.gate_complete);
}

#[test]
fn excludes_false_host_cfg_items_from_metrics() {
    let metrics = extract(
        &source(
            r#"
            #[cfg(target_os = "macos")]
            fn mac_only() { if true {} }

            #[cfg(target_os = "linux")]
            fn linux_only() { if true {} }
            "#,
        ),
        &host(),
    )
    .unwrap();

    assert_eq!(metrics.decision_sites, 1);
    assert!(metrics.gate_complete);
}

#[test]
fn formatting_does_not_change_structure_digest() {
    let left = extract(&source("fn f(){if true{}}"), &host()).unwrap();
    let right = extract(&source("fn f() { if true { } }"), &host()).unwrap();
    assert_eq!(left.structure_digest, right.structure_digest);
}

#[test]
fn flattens_grouped_imports_deterministically() {
    let metrics = extract(
        &source("use crate::model::{Thing, nested::Other};"),
        &host(),
    )
    .unwrap();
    let paths = metrics
        .explicit_imports
        .iter()
        .map(|path| path.segments.join("::"))
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec!["crate::model::Thing", "crate::model::nested::Other"]
    );
}

#[test]
fn resolves_local_and_workspace_module_imports() {
    let mut modules = vec![
        extracted(
            "demo",
            "engine",
            "src/engine.rs",
            "use crate::model::Thing; use shared::nested::Other;",
        ),
        extracted("demo", "model", "src/model.rs", ""),
        extracted("shared", "", "crates/shared/src/lib.rs", ""),
        extracted("shared", "nested", "crates/shared/src/nested.rs", ""),
    ];
    let aliases = WorkspaceAliases::from([(
        "demo".into(),
        BTreeMap::from([("shared".into(), "shared".into())]),
    )]);

    resolve_workspace_dependencies(&mut modules, &aliases);

    assert_eq!(
        modules[0].local_dependency_modules,
        vec!["demo::model", "shared::nested"]
    );
    assert!(modules[0].gate_complete);
}

#[test]
fn glob_import_makes_gate_dependency_evidence_incomplete() {
    let mut modules = vec![
        extracted("demo", "engine", "src/engine.rs", "use crate::model::*;"),
        extracted("demo", "model", "src/model.rs", ""),
    ];

    resolve_workspace_dependencies(&mut modules, &WorkspaceAliases::new());

    assert!(!modules[0].gate_complete);
    assert!(modules[0].local_dependency_modules.is_empty());
}

#[test]
fn unresolved_workspace_import_makes_gate_evidence_incomplete() {
    let mut modules = vec![extracted(
        "demo",
        "engine",
        "src/engine.rs",
        "use shared::missing::Thing;",
    )];
    let aliases = WorkspaceAliases::from([(
        "demo".into(),
        BTreeMap::from([("shared".into(), "shared".into())]),
    )]);

    resolve_workspace_dependencies(&mut modules, &aliases);

    assert!(!modules[0].gate_complete);
    assert!(modules[0].local_dependency_modules.is_empty());
}

fn extracted(
    crate_name: &str,
    module_path: &str,
    relative_path: &str,
    text: &str,
) -> ModuleMetrics {
    let source = SourceFile {
        crate_name: crate_name.into(),
        module_path: module_path.into(),
        relative_path: relative_path.into(),
        bytes: text.as_bytes().to_vec(),
    };
    extract(&source, &host()).unwrap()
}

#[test]
fn rejects_invalid_source_bytes_and_invalid_rust() {
    let mut invalid_utf8 = source("");
    invalid_utf8.bytes = vec![0xff];
    assert!(extract(&invalid_utf8, &host())
        .unwrap_err()
        .contains("not valid UTF-8"));

    assert!(extract(&source("fn {"), &host())
        .unwrap_err()
        .contains("Rust parse failed"));
}

#[test]
fn cfg_attr_and_malformed_cfg_make_gate_evidence_incomplete() {
    let cfg_attr = extract(
        &source("#[cfg_attr(unix, inline)] fn configured() {}"),
        &host(),
    )
    .unwrap();
    assert!(!cfg_attr.gate_complete);
    assert!(cfg_attr.limitation.as_deref().unwrap().contains("cfg_attr"));

    let malformed = extract(&source("#[cfg()] fn configured() {}"), &host()).unwrap();
    assert!(!malformed.gate_complete);
    assert!(malformed
        .limitation
        .as_deref()
        .unwrap()
        .contains("unresolved production cfg"));
}

#[test]
fn counts_every_v1_decision_syntax_family() {
    let metrics = extract(
        &source(
            r#"
            fn run(values: &[bool]) {
                for value in values {
                    while *value { break; }
                }
                match true {
                    value if value => (),
                    false => (),
                    _ => (),
                }
                let _ = true || false;
            }
            "#,
        ),
        &host(),
    )
    .unwrap();

    assert_eq!(metrics.decision_sites, 6);
}

#[test]
fn visits_supported_item_families_and_counts_only_declared_public_surfaces() {
    let metrics = extract(
        &source(
            r#"
            pub const C: u8 = 0;
            pub enum E { A }
            pub extern crate core;
            pub fn f() {}
            unsafe extern "C" { fn foreign(); }
            impl E {}
            macro_rules! local { () => {} }
            pub mod inline {}
            pub static S: u8 = 0;
            pub struct St;
            pub trait Tr {}
            pub trait Alias = Tr;
            pub type T = u8;
            pub union U { value: u8 }
            pub use crate::inline as renamed;
            "#,
        ),
        &host(),
    )
    .unwrap();

    assert_eq!(metrics.public_items, 12);
    assert!(!metrics.gate_complete);
}

#[test]
fn flattens_renamed_imports_without_using_the_alias_as_identity() {
    let metrics = extract(&source("use crate::model::Thing as LocalThing;"), &host()).unwrap();

    assert_eq!(metrics.explicit_imports.len(), 1);
    assert_eq!(
        metrics.explicit_imports[0].segments,
        ["crate", "model", "Thing"]
    );
    assert!(!metrics.explicit_imports[0].glob);
}

#[test]
fn resolves_relative_local_external_empty_and_missing_import_cases() {
    use super::ImportResolution;

    let known = BTreeMap::from([(
        "demo".to_owned(),
        BTreeSet::from([
            String::new(),
            "a".to_owned(),
            "a::b".to_owned(),
            "local".to_owned(),
        ]),
    )]);

    assert!(matches!(
        super::resolve_import("demo", "a::b", &[], &known, None),
        ImportResolution::External
    ));
    assert!(matches!(
        super::resolve_import("missing", "", &["crate".into()], &known, None),
        ImportResolution::UnresolvedRepository
    ));

    for segments in [
        vec!["crate".into(), "a".into(), "b".into(), "Thing".into()],
        vec!["self".into(), "Thing".into()],
        vec!["super".into(), "Thing".into()],
        vec!["super".into(), "super".into(), "Thing".into()],
        vec!["local".into(), "Thing".into()],
    ] {
        assert!(matches!(
            super::resolve_import("demo", "a::b", &segments, &known, None),
            ImportResolution::Repository(_)
        ));
    }

    assert!(matches!(
        super::resolve_import(
            "demo",
            "a",
            &["serde".into(), "Serialize".into()],
            &known,
            None
        ),
        ImportResolution::External
    ));

    let aliases = BTreeMap::from([("shared".to_owned(), "missing".to_owned())]);
    assert!(matches!(
        super::resolve_import(
            "demo",
            "a",
            &["shared".into(), "Thing".into()],
            &known,
            Some(&aliases)
        ),
        ImportResolution::UnresolvedRepository
    ));

    assert!(super::resolve_in_crate("a", &["external".into()], &known["demo"]).is_none());
    assert!(
        super::resolve_absolute(&["a".into(), "b".into(), "Thing".into()], &known["demo"])
            .is_some()
    );
    assert!(super::longest_module_prefix(
        vec!["missing".into()],
        &BTreeSet::from(["known".to_owned()])
    )
    .is_none());
    assert_eq!(super::qualify("demo", ""), "demo");
}

#[test]
fn dependency_resolution_ignores_external_and_self_edges_and_preserves_limitations_once() {
    let mut modules = vec![
        extracted(
            "demo",
            "engine",
            "src/engine.rs",
            "use serde::Serialize; use crate::engine::Thing; use crate::missing::Thing; use crate::missing::*;",
        ),
        extracted("demo", "other", "src/other.rs", ""),
    ];
    modules[0].limitation = Some("existing".into());

    resolve_workspace_dependencies(&mut modules, &WorkspaceAliases::new());
    resolve_workspace_dependencies(&mut modules, &WorkspaceAliases::new());

    assert!(modules[0].local_dependency_modules.is_empty());
    let limitation = modules[0].limitation.as_deref().unwrap();
    assert!(limitation.contains("existing"));
    assert_eq!(
        limitation
            .matches("module contains a glob import whose gate dependency surface is ambiguous")
            .count(),
        1
    );
    assert_eq!(
        limitation
            .matches("one or more repository-owned explicit imports could not be resolved")
            .count(),
        1
    );
}

#[test]
fn parse_incomplete_modules_are_not_linked() {
    let mut incomplete = extracted("demo", "broken", "src/broken.rs", "");
    incomplete.parse_complete = false;
    incomplete.explicit_imports = vec![crate::model::ImportPath {
        segments: vec!["crate".into(), "other".into()],
        glob: false,
    }];
    let mut modules = vec![incomplete, extracted("demo", "other", "src/other.rs", "")];

    resolve_workspace_dependencies(&mut modules, &WorkspaceAliases::new());

    assert!(modules[0].local_dependency_modules.is_empty());
}

#[test]
fn unsupported_syn_item_variants_have_no_attributes_or_public_surface() {
    let item = syn::Item::Verbatim(quote::quote!(unsupported));

    assert!(super::item_attributes(&item).is_empty());
    assert!(!super::is_public(&item));
}

#[test]
fn gate_limitation_preserves_the_first_reason() {
    let cfg = host();
    let mut visitor = super::MetricsVisitor::new(&cfg);

    visitor.limit_gate("first");
    visitor.limit_gate("second");

    assert_eq!(visitor.gate_limitation.as_deref(), Some("first"));
}

#[test]
fn non_boolean_binary_expressions_do_not_count_as_decisions() {
    let metrics = extract(&source("fn f() { let _ = 1 + 2; }"), &host()).unwrap();

    assert_eq!(metrics.decision_sites, 0);
}

#[test]
fn extracts_function_type_and_clone_syntax_facts_deterministically() {
    use crate::model::{FunctionKind, TypeKind};

    let metrics = extract(
        &source(
            r#"
            pub struct PublicType;
            enum PrivateEnum { A }

            pub fn top_level(value: PublicType) {
                let _ = value.clone();
            }

            impl PublicType {
                pub fn make() -> Self { Self }
                fn duplicate(&self) {
                    let _ = self.clone();
                    let _ = self.clone();
                }
            }

            pub trait PublicTrait {
                fn required(&self);
                fn defaulted(&self) {}
            }

            mod nested {
                pub type Alias = u8;
                pub fn nested_fn() {}
            }
            "#,
        ),
        &host(),
    )
    .unwrap();

    assert_eq!(metrics.clone_calls, 3);
    assert!(metrics.functions.iter().any(|fact| {
        fact.name == "top_level" && fact.kind == FunctionKind::Function && fact.public_declared
    }));
    assert!(metrics.functions.iter().any(|fact| {
        fact.name == "PublicType::make" && fact.kind == FunctionKind::Method && fact.public_declared
    }));
    assert!(metrics.functions.iter().any(|fact| {
        fact.name == "PublicType::duplicate"
            && fact.kind == FunctionKind::Method
            && !fact.public_declared
    }));
    assert!(metrics.functions.iter().any(|fact| {
        fact.name == "PublicTrait::required"
            && fact.kind == FunctionKind::TraitMethod
            && fact.public_declared
    }));
    assert!(metrics
        .functions
        .iter()
        .any(|fact| fact.name == "nested::nested_fn"));

    assert!(metrics.types.iter().any(|fact| {
        fact.name == "PublicType" && fact.kind == TypeKind::Struct && fact.public_declared
    }));
    assert!(metrics.types.iter().any(|fact| {
        fact.name == "PrivateEnum" && fact.kind == TypeKind::Enum && !fact.public_declared
    }));
    assert!(metrics.types.iter().any(|fact| {
        fact.name == "PublicTrait" && fact.kind == TypeKind::Trait && fact.public_declared
    }));
    assert!(metrics
        .types
        .iter()
        .any(|fact| fact.name == "nested::Alias" && fact.kind == TypeKind::TypeAlias));

    let mut function_names = metrics
        .functions
        .iter()
        .map(|fact| fact.name.as_str())
        .collect::<Vec<_>>();
    let sorted = {
        let mut value = function_names.clone();
        value.sort_unstable();
        value
    };
    assert_eq!(function_names, sorted);

    function_names.dedup();
    assert_eq!(function_names.len(), metrics.functions.len());
}

#[test]
fn cfg_disabled_impl_members_do_not_contribute_item_or_clone_facts() {
    let metrics = extract(
        &source(
            r#"
            struct Item;
            impl Item {
                #[cfg(target_os = "macos")]
                fn disabled(&self) {
                    let _ = self.clone();
                }

                #[cfg(target_os = "linux")]
                fn enabled(&self) {}
            }
            "#,
        ),
        &host(),
    )
    .unwrap();

    assert_eq!(metrics.clone_calls, 0);
    assert!(!metrics
        .functions
        .iter()
        .any(|fact| fact.name.ends_with("disabled")));
    assert!(metrics
        .functions
        .iter()
        .any(|fact| fact.name.ends_with("enabled")));
}

#[test]
fn member_cfg_gaps_skip_impl_trait_and_foreign_facts() {
    let metrics = extract(
        &source(
            r#"
            struct Item;
            impl Item {
                #[cfg_attr(unix, inline)]
                fn impl_cfg_attr(&self) {}

                #[cfg(my_custom_cfg)]
                fn impl_unknown(&self) {}
            }

            trait Contract {
                #[cfg(my_custom_cfg)]
                fn trait_unknown(&self);
            }

            unsafe extern "C" {
                #[cfg_attr(unix, allow(dead_code))]
                fn foreign_cfg_attr();
            }
            "#,
        ),
        &host(),
    )
    .unwrap();

    assert!(!metrics.gate_complete);
    for name in [
        "impl_cfg_attr",
        "impl_unknown",
        "trait_unknown",
        "foreign_cfg_attr",
    ] {
        assert!(!metrics
            .functions
            .iter()
            .any(|fact| fact.name.ends_with(name)));
    }
}

#[test]
fn non_clone_method_calls_do_not_increment_clone_syntax_count() {
    let metrics = extract(&source("fn f(value: Value) { value.observe(); }"), &host()).unwrap();

    assert_eq!(metrics.clone_calls, 0);
}

static CONTEXT_TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn context_root(name: &str) -> std::path::PathBuf {
    let counter = CONTEXT_TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "ferric-lens-source-context-{name}-{}-{counter}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).unwrap();
    root
}

fn context_module(module_path: &str, path: &str, text: &str) -> ModuleMetrics {
    let mut module = extracted("demo", module_path, path, text);
    module.local_dependency_modules.clear();
    module
}

fn context_finding(subject: &str, metrics: &[&str]) -> Finding {
    Finding {
        fingerprint: "context".into(),
        rule: "test.context".into(),
        subject: subject.into(),
        identity: subject.into(),
        configuration: "host".into(),
        evidence_class: EvidenceClass::Candidate,
        priority: Priority::Observe,
        delta: DeltaStatus::Current,
        gate: false,
        accepted: false,
        acceptance_reason: None,
        summary: "context".into(),
        direction: "inspect".into(),
        evidence: metrics
            .iter()
            .map(|metric| Evidence {
                metric: (*metric).into(),
                value: 1,
                reference: 0,
                population: 1,
                baseline: None,
                material_delta: None,
            })
            .collect(),
    }
}

fn source_digest(text: &[u8]) -> String {
    blake3::hash(text).to_hex().to_string()
}

#[test]
fn source_contexts_cover_decisions_clones_local_and_reverse_dependencies() {
    let root = context_root("signals");
    let engine = concat!(
        "use crate::target;\n",
        "fn run(value: String, flag: bool) {\n",
        "    if flag { let _ = value.clone(); }\n",
        "    if flag {}\n",
        "    if flag {}\n",
        "    if flag {}\n",
        "}\n",
    );
    let caller = "use crate::engine;\nfn caller() {}\n";
    let target = "pub fn target() {}\n";
    fs::write(root.join("src/engine.rs"), engine).unwrap();
    fs::write(root.join("src/caller.rs"), caller).unwrap();
    fs::write(root.join("src/target.rs"), target).unwrap();

    let mut modules = vec![
        context_module("engine", "src/engine.rs", engine),
        context_module("caller", "src/caller.rs", caller),
        context_module("target", "src/target.rs", target),
    ];
    resolve_workspace_dependencies(&mut modules, &WorkspaceAliases::new());

    let digests = BTreeMap::from([
        ("src/engine.rs".into(), source_digest(engine.as_bytes())),
        ("src/caller.rs".into(), source_digest(caller.as_bytes())),
        ("src/target.rs".into(), source_digest(target.as_bytes())),
    ]);
    let findings = vec![
        context_finding(
            "demo::engine",
            &[
                "decision_sites",
                "clone_call_syntax_sites",
                "local_dependency_modules",
            ],
        ),
        context_finding("demo::engine", &["reverse_repository_dependents"]),
        context_finding("demo::engine", &["public_items"]),
    ];

    let contexts = super::source_contexts_for_findings(
        &root,
        &modules,
        &WorkspaceAliases::new(),
        &digests,
        &findings,
        &host(),
    )
    .unwrap();

    assert_eq!(
        contexts
            .iter()
            .filter(|context| {
                context.subject == "demo::engine" && context.metric == "decision_sites"
            })
            .count(),
        3
    );
    assert!(contexts.iter().any(|context| {
        context.subject == "demo::engine"
            && context.metric == "clone_call_syntax_sites"
            && context.path == "src/engine.rs"
            && context.start_line == 3
    }));
    assert!(contexts.iter().any(|context| {
        context.subject == "demo::engine"
            && context.metric == "local_dependency_modules"
            && context.path == "src/engine.rs"
            && context.start_line == 1
    }));
    assert!(contexts.iter().any(|context| {
        context.subject == "demo::engine"
            && context.metric == "reverse_repository_dependents"
            && context.path == "src/caller.rs"
            && context.start_line == 1
    }));
    assert!(!contexts
        .iter()
        .any(|context| context.metric == "public_items"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_contexts_ignore_globs_external_imports_and_missing_subjects() {
    let root = context_root("ignored-imports");
    let text = "use crate::target::*;\nuse serde::Serialize;\nfn run() {}\n";
    fs::write(root.join("src/engine.rs"), text).unwrap();

    let mut engine = context_module("engine", "src/engine.rs", text);
    engine.local_dependency_modules = vec!["demo::target".into()];
    let target = context_module("target", "src/target.rs", "");
    let modules = vec![engine, target];
    let digests = BTreeMap::from([("src/engine.rs".into(), source_digest(text.as_bytes()))]);
    let findings = vec![
        context_finding("demo::engine", &["local_dependency_modules"]),
        context_finding("demo::missing", &["decision_sites"]),
    ];

    let contexts = super::source_contexts_for_findings(
        &root,
        &modules,
        &WorkspaceAliases::new(),
        &digests,
        &findings,
        &host(),
    )
    .unwrap();

    assert!(contexts.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_contexts_return_empty_when_no_supported_evidence_is_requested() {
    let root = context_root("no-supported-metrics");
    let contexts = super::source_contexts_for_findings(
        &root,
        &[],
        &WorkspaceAliases::new(),
        &BTreeMap::new(),
        &[context_finding("demo::engine", &["public_items"])],
        &host(),
    )
    .unwrap();

    assert!(contexts.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_contexts_detect_missing_digest_changed_missing_invalid_and_malformed_sources() {
    let root = context_root("source-errors");
    let module = context_module("engine", "src/engine.rs", "fn run() { if true {} }\n");
    let finding = context_finding("demo::engine", &["decision_sites"]);

    fs::write(root.join("src/engine.rs"), "fn run() { if true {} }\n").unwrap();
    let error = super::source_contexts_for_findings(
        &root,
        std::slice::from_ref(&module),
        &WorkspaceAliases::new(),
        &BTreeMap::new(),
        std::slice::from_ref(&finding),
        &host(),
    )
    .unwrap_err();
    assert!(error.contains("missing source digest"));

    let digests = BTreeMap::from([("src/engine.rs".into(), "wrong".into())]);
    let error = super::source_contexts_for_findings(
        &root,
        std::slice::from_ref(&module),
        &WorkspaceAliases::new(),
        &digests,
        std::slice::from_ref(&finding),
        &host(),
    )
    .unwrap_err();
    assert!(error.contains("source changed"));

    fs::remove_file(root.join("src/engine.rs")).unwrap();
    let digests = BTreeMap::from([("src/engine.rs".into(), "irrelevant".into())]);
    let error = super::source_contexts_for_findings(
        &root,
        std::slice::from_ref(&module),
        &WorkspaceAliases::new(),
        &digests,
        std::slice::from_ref(&finding),
        &host(),
    )
    .unwrap_err();
    assert!(error.contains("cannot read"));

    fs::write(root.join("src/engine.rs"), [0xff]).unwrap();
    let digests = BTreeMap::from([("src/engine.rs".into(), source_digest(&[0xff]))]);
    let error = super::source_contexts_for_findings(
        &root,
        std::slice::from_ref(&module),
        &WorkspaceAliases::new(),
        &digests,
        std::slice::from_ref(&finding),
        &host(),
    )
    .unwrap_err();
    assert!(error.contains("not valid UTF-8"));

    let malformed = b"fn {";
    fs::write(root.join("src/engine.rs"), malformed).unwrap();
    let digests = BTreeMap::from([("src/engine.rs".into(), source_digest(malformed))]);
    let error = super::source_contexts_for_findings(
        &root,
        &[module],
        &WorkspaceAliases::new(),
        &digests,
        &[finding],
        &host(),
    )
    .unwrap_err();
    assert!(error.contains("Rust parse failed"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_context_excerpt_bounds_lines_characters_and_deduplicates_spans() {
    let long = "x".repeat(700);
    let text = format!("first\n{long}\nthird\nfourth\nfifth\n");
    let mut contexts = Vec::<SourceContext>::new();
    super::push_span_contexts(
        &mut contexts,
        "demo::engine",
        "decision_sites",
        "src/engine.rs",
        &[
            super::LineSpan { start: 1, end: 5 },
            super::LineSpan { start: 1, end: 5 },
        ],
        &text,
    );

    assert_eq!(contexts.len(), 1);
    let context = &contexts[0];
    assert_eq!(context.start_line, 1);
    assert_eq!(context.end_line, 5);
    assert!(context.excerpt_truncated);
    assert!(context.excerpt.ends_with('…'));
    assert!(context.excerpt.chars().count() <= 600);

    let (short, truncated) = super::truncate_excerpt("short");
    assert_eq!(short, "short");
    assert!(!truncated);
}

#[test]
fn source_context_span_capture_covers_every_decision_syntax_family() {
    let root = context_root("decision-spans");
    let text = concat!(
        "fn run(values: &[bool]) {\n",
        "    for value in values {\n",
        "        while *value { break; }\n",
        "    }\n",
        "    loop { break; }\n",
        "    match true {\n",
        "        value if value => (),\n",
        "        false => (),\n",
        "        _ => (),\n",
        "    }\n",
        "    let _ = true && false || true;\n",
        "}\n",
    );
    fs::write(root.join("src/engine.rs"), text).unwrap();
    let module = context_module("engine", "src/engine.rs", text);
    let digests = BTreeMap::from([("src/engine.rs".into(), source_digest(text.as_bytes()))]);

    let contexts = super::source_contexts_for_findings(
        &root,
        &[module],
        &WorkspaceAliases::new(),
        &digests,
        &[context_finding("demo::engine", &["decision_sites"])],
        &host(),
    )
    .unwrap();

    assert_eq!(contexts.len(), 3);
    assert!(contexts.iter().all(|context| context.start_line > 0));
    fs::remove_dir_all(root).unwrap();
}

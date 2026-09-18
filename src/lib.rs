//! Ferric Lens analysis core.
//!
//! The implementation intentionally starts as one synchronous package. The
//! modules mirror the boundaries described in ARCHITECTURE.md without
//! introducing a framework or cross-module trait hierarchy prematurely.

pub mod extract;
pub mod git;
pub mod input;
pub mod model;
pub mod report;
pub mod rules;

use std::path::Path;

use model::{AnalysisResult, Capability, CapabilityStatus, GateVerdict};

/// Analyze the current working tree.
///
/// V1 baseline comparison is not implemented in this foundation slice yet, so
/// the returned gate verdict is explicitly inconclusive. Current-snapshot facts
/// and advisory findings are nevertheless complete for the files successfully
/// inventoried and parsed.
pub fn analyze(root: &Path) -> Result<AnalysisResult, String> {
    let inventory = input::inventory(root)?;
    let git = git::inspect(root);

    let mut modules = Vec::with_capacity(inventory.sources.len());
    let mut parse_failures = 0usize;

    for source in &inventory.sources {
        match extract::extract(source) {
            Ok(module) => modules.push(module),
            Err(error) => {
                parse_failures += 1;
                modules.push(model::ModuleMetrics::unsupported(
                    source.crate_name.clone(),
                    source.module_path.clone(),
                    source.relative_path.clone(),
                    error,
                ));
            }
        }
    }

    modules.sort_by(|a, b| {
        (&a.crate_name, &a.module_path, &a.path).cmp(&(&b.crate_name, &b.module_path, &b.path))
    });

    extract::resolve_local_dependencies(&mut modules);
    let findings = rules::current_snapshot_findings(&modules);

    let mut capabilities = vec![Capability {
        name: "source_inventory".into(),
        status: if inventory.metadata_complete {
            CapabilityStatus::Complete
        } else {
            CapabilityStatus::Partial
        },
        detail: inventory.metadata_detail.clone(),
    }];

    capabilities.push(Capability {
        name: "syntax_extraction".into(),
        status: if parse_failures == 0 {
            CapabilityStatus::Complete
        } else {
            CapabilityStatus::Partial
        },
        detail: if parse_failures == 0 {
            None
        } else {
            Some(format!(
                "{parse_failures} Rust source file(s) could not be parsed"
            ))
        },
    });

    capabilities.push(Capability {
        name: "baseline_comparison".into(),
        status: CapabilityStatus::Unavailable,
        detail: Some("baseline comparison is not implemented in the foundation slice".into()),
    });

    capabilities.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(AnalysisResult {
        schema_version: 1,
        tool_version: env!("CARGO_PKG_VERSION").into(),
        snapshot: model::Snapshot {
            content_digest: inventory.content_digest,
            git_head: git.head,
            dirty: git.dirty,
            source_files: modules.len(),
        },
        verdict: GateVerdict::Inconclusive,
        verdict_reason:
            "baseline comparison is not implemented yet; advisory current-snapshot analysis is available"
                .into(),
        capabilities,
        modules,
        findings,
    })
}

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GateVerdict {
    Pass,
    Regression,
    Inconclusive,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Capability {
    pub name: String,
    pub status: CapabilityStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Snapshot {
    pub content_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_head: Option<String>,
    pub dirty: Option<bool>,
    pub source_files: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ImportPath {
    pub segments: Vec<String>,
    pub glob: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModuleMetrics {
    pub crate_name: String,
    pub module_path: String,
    pub path: String,
    pub lines: usize,
    pub decision_sites: usize,
    pub public_items: usize,
    pub explicit_imports: Vec<ImportPath>,
    pub local_dependency_modules: Vec<String>,
    pub parse_complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limitation: Option<String>,
}

impl ModuleMetrics {
    pub fn unsupported(
        crate_name: String,
        module_path: String,
        path: String,
        limitation: String,
    ) -> Self {
        Self {
            crate_name,
            module_path,
            path,
            lines: 0,
            decision_sites: 0,
            public_items: 0,
            explicit_imports: Vec::new(),
            local_dependency_modules: Vec::new(),
            parse_complete: false,
            limitation: Some(limitation),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceClass {
    Proven,
    Strong,
    Candidate,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Investigate,
    Observe,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Evidence {
    pub metric: String,
    pub value: usize,
    pub reference: usize,
    pub population: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Finding {
    pub rule: String,
    pub subject: String,
    pub evidence_class: EvidenceClass,
    pub priority: Priority,
    pub summary: String,
    pub direction: String,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AnalysisResult {
    pub schema_version: u32,
    pub tool_version: String,
    pub snapshot: Snapshot,
    pub verdict: GateVerdict,
    pub verdict_reason: String,
    pub capabilities: Vec<Capability>,
    pub modules: Vec<ModuleMetrics>,
    pub findings: Vec<Finding>,
}

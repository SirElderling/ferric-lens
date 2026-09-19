use serde::{Deserialize, Serialize};

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
pub struct AnalysisProfile {
    pub id: String,
    pub target: String,
    pub resolved_target: String,
    pub features: Vec<String>,
    pub target_cfg: Vec<String>,
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
pub struct BaselineContext {
    pub target_ref: String,
    pub target_oid: String,
    pub merge_base: String,
    pub source_files: usize,
    pub content_digest: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DependencyCycle {
    pub modules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ArchitectureSummary {
    pub modules: usize,
    pub explicit_dependency_edges: usize,
    pub incomplete_modules: usize,
    pub cycles: Vec<DependencyCycle>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportPath {
    pub segments: Vec<String>,
    pub glob: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum FunctionKind {
    Function,
    Method,
    TraitMethod,
    ForeignFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct FunctionFact {
    pub name: String,
    pub kind: FunctionKind,
    pub public_declared: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TypeKind {
    Struct,
    Enum,
    Union,
    Trait,
    TraitAlias,
    TypeAlias,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct TypeFact {
    pub name: String,
    pub kind: TypeKind,
    pub public_declared: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum FunctionKind {
    Function,
    Method,
    TraitMethod,
    ForeignFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct FunctionFact {
    pub name: String,
    pub kind: FunctionKind,
    pub public_declared: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TypeKind {
    Struct,
    Enum,
    Union,
    TypeAlias,
    Trait,
    TraitAlias,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct TypeFact {
    pub name: String,
    pub kind: TypeKind,
    pub public_declared: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModuleMetrics {
    pub crate_name: String,
    pub module_path: String,
    pub path: String,
    pub lines: usize,
    pub decision_sites: usize,
    pub public_items: usize,
    #[serde(default)]
    pub clone_calls: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<FunctionFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<TypeFact>,
    pub explicit_imports: Vec<ImportPath>,
    pub local_dependency_modules: Vec<String>,
    pub structure_digest: String,
    pub parse_complete: bool,
    pub gate_complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limitation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryEvidence>,
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
            clone_calls: 0,
            functions: Vec::new(),
            types: Vec::new(),
            explicit_imports: Vec::new(),
            local_dependency_modules: Vec::new(),
            structure_digest: String::new(),
            parse_complete: false,
            gate_complete: false,
            limitation: Some(limitation),
            history: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoChangeEvidence {
    pub path: String,
    pub shared_commits: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEvidence {
    pub change_commits: usize,
    pub sampled_commits: usize,
    pub cochange: Vec<CoChangeEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistorySummary {
    pub sampled_commits: usize,
    pub changed_path_records: usize,
    pub broad_commits_excluded_from_cochange: usize,
    pub truncated: bool,
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
    ActFirst,
    Investigate,
    Observe,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeltaStatus {
    Current,
    New,
    Worsened,
    Unchanged,
    Unknown,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Evidence {
    pub metric: String,
    pub value: usize,
    pub reference: usize,
    pub population: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material_delta: Option<usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Finding {
    pub fingerprint: String,
    pub rule: String,
    pub subject: String,
    pub identity: String,
    pub configuration: String,
    pub evidence_class: EvidenceClass,
    pub priority: Priority,
    pub delta: DeltaStatus,
    pub gate: bool,
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acceptance_reason: Option<String>,
    pub summary: String,
    pub direction: String,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ImportedObservation {
    pub subject: String,
    pub metric: String,
    pub value: i64,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ImportedEvidence {
    pub producer: String,
    pub producer_version: String,
    pub source_content_digest: Option<String>,
    pub source_git_commit: Option<String>,
    pub target: String,
    pub features: Vec<String>,
    pub attached: bool,
    pub attachment_reason: String,
    pub observations: Vec<ImportedObservation>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AnalysisResult {
    pub schema_version: u32,
    pub tool_version: String,
    pub snapshot: Snapshot,
    pub profile: AnalysisProfile,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<BaselineContext>,
    pub verdict: GateVerdict,
    pub verdict_reason: String,
    pub applicable_gate_subjects: usize,
    pub architecture: ArchitectureSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<HistorySummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_evidence: Option<ImportedEvidence>,
    pub capabilities: Vec<Capability>,
    pub modules: Vec<ModuleMetrics>,
    pub findings: Vec<Finding>,
}

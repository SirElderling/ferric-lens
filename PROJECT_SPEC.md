# Ferric Lens Project Specification

## 1. Product definition

Ferric Lens is a deterministic, zero-configuration Rust codebase analysis tool.

It produces a unified structural model of a Rust repository, identifies evidence-backed optimization and refactoring candidates, and can act as a CI gate.

The initial product is static and offline. It does not modify source code and does not depend on AI, network services, or project-specific configuration.

## 2. Primary goals

Ferric Lens must:

- provide useful results with zero configuration,
- work on Linux and macOS,
- use stable Rust only,
- produce deterministic and reproducible canonical output,
- provide a self-contained static HTML report,
- provide machine-readable JSON,
- support fast CI gating and deeper full analysis,
- prioritize changed-code impact,
- identify architecture, coupling, complexity, refactor, runtime-risk, and build-efficiency concerns,
- use Git history as targeted evidence,
- distinguish proven findings from strong evidence and candidates,
- avoid opaque aggregate quality scores,
- avoid single-heuristic CI failures,
- remain fully functional offline.

## 3. Non-goals

Ferric Lens v1 is not:

- a general correctness linter,
- a vulnerability or dependency-security scanner,
- a replacement for Clippy, Miri, cargo-audit, or similar specialist tools,
- an automated refactoring engine,
- an AI-assisted analysis system,
- a repository analytics/history dashboard,
- a runtime profiler,
- a project-specific architecture-policy engine,
- a source-code rewriting tool.

## 4. Default operating model

Ferric Lens has two user-facing jobs.

### 4.1 Fast CI check

```text
ferric-lens check
```

Purpose:

- determine whether the current change introduces gate-worthy regressions,
- minimize work by computing only the evidence necessary to reach that decision,
- expand analysis only when required.

### 4.2 Full analysis

```text
ferric-lens analyze
```

Purpose:

- build the complete available analysis model,
- generate the full HTML report,
- generate the canonical JSON result,
- provide richer evidence for hotspots and refactor candidates.

The meaning of findings must not differ between modes. The fast mode may omit optional enrichment that is not required to reach a gate decision.

## 5. Canonical analysis model

All human and machine outputs must derive from one canonical deterministic analysis model.

The model must represent at least:

- workspace,
- crates,
- modules,
- functions,
- types,
- dependency relationships,
- public surfaces,
- structural metrics,
- change relationships,
- findings,
- finding evidence,
- analysis completeness,
- imported evidence,
- platform/configuration origin.

No output path may independently recompute the same semantic result.

## 6. Zero-configuration analysis

Ferric Lens must not require the user to maintain threshold values or architecture descriptions.

The default analysis should derive findings from:

- objective invariants,
- repository-relative distributions,
- structural outliers,
- graph relationships,
- baseline changes,
- corroborating evidence,
- targeted Git history,
- Cargo and Rust structure.

Examples of repository-relative evidence include:

- unusually high complexity relative to the repository,
- unusually broad dependency reach,
- unusually high fan-in or fan-out,
- concentrated churn,
- co-change relationships,
- increasing public-surface exposure.

## 7. Finding model

Each finding must have a stable identity and explicit evidence.

A finding must include enough information to answer:

- what was detected,
- where it was detected,
- why it matters,
- what evidence triggered it,
- whether it is new, worsened, resolved, accepted, or unchanged,
- how confident Ferric Lens is,
- which analysis configuration observed it.

### 7.1 Evidence classes

Ferric Lens uses three evidence classes:

```text
proven
strong
candidate
```

Definitions:

- **proven**: deterministically established by an invariant or directly observable condition.
- **strong**: supported by multiple independent deterministic signals.
- **candidate**: notable evidence exists, but not enough to justify a strong conclusion.

### 7.2 Priority

Ferric Lens must not produce an opaque overall quality score.

Findings are prioritized using transparent evidence such as:

- evidence strength,
- scope or blast radius,
- baseline regression,
- number of independent corroborating signals,
- recurrence or churn,
- affected public surface,
- changed-code relevance.

Human-facing priority bands should remain understandable, for example:

```text
Priority 1 — act first
Priority 2 — investigate
Observe
```

Every priority must be explainable from the finding evidence.

## 8. CI gating

Ferric Lens should maximize signal while avoiding noisy gates.

### 8.1 Gate rule

CI fails when either:

1. a new proven regression is introduced, or
2. a new material regression is supported by a quorum of independent evidence.

A single non-provable heuristic must never fail CI by itself.

### 8.2 Existing debt

Existing unchanged findings do not fail a PR merely because they exist.

Ferric Lens should classify findings relative to the baseline as:

- new,
- worsened,
- resolved,
- unchanged,
- carried through structural movement.

### 8.3 Baseline source

Ferric Lens does not require a committed baseline snapshot.

Git is the source of truth.

For PR-style analysis, Ferric Lens should use merge-base-aware attribution rather than naive comparison against only the current tip of `main`.

Large changes must not receive relaxed quality standards. They must receive better attribution.

## 9. Git history

Ferric Lens uses native Git tooling as the default history engine.

History analysis must be:

- targeted,
- bounded,
- path-aware,
- merge-base-aware,
- fast enough for CI,
- explicit about completeness.

A repository with extensive history must not imply scanning every commit on every run.

History should be queried primarily for changed files, candidate hotspots, and evidence expansion.

Relevant history evidence may include:

- change frequency,
- temporal coupling,
- repeated boundary instability,
- files/modules that repeatedly change together.

History is evidence only; Ferric Lens does not provide a general historical timeline product.

If history is insufficient, Ferric Lens must report that condition explicitly and must not silently calculate misleading history-derived results.

## 10. Structural architecture analysis

Ferric Lens should infer structural boundaries deterministically from objective repository structure.

Possible evidence includes:

- crate/module dependency density,
- public API boundaries,
- fan-in and fan-out,
- dependency direction,
- type ownership,
- behavior ownership,
- co-change relationships.

Inferred boundaries are evidence, not policy.

Ferric Lens may report a boundary-crossing dependency as a structural anomaly but must not call it a policy violation unless a future product version gains explicit repository policy support.

Project-specific architecture rules are out of scope for v1.

## 11. Refactor candidates

Ferric Lens should identify refactor candidates only when supported by deterministic evidence.

Potential evidence includes:

- structural complexity,
- coupling,
- dependency reach,
- change concentration,
- repeated co-change,
- public-surface expansion,
- test weakness from imported evidence,
- performance-risk signals,
- repeated traversal or allocation patterns.

Ferric Lens may provide a deterministic structural direction when the evidence supports it.

Allowed directions include:

- split mixed responsibilities,
- reduce dependency surface,
- isolate volatility,
- move behavior closer to owned data,
- break dependency cycles,
- reduce repeated traversal,
- reduce unnecessary allocation or copying,
- narrow public surface,
- establish a clearer stable boundary.

Ferric Lens must not generate source changes or prescribe a final architecture.

## 12. Runtime performance-risk analysis

Ferric Lens performs static performance-risk analysis.

It must distinguish:

### Proven static cost

A cost directly observable from source/structure.

### Strong performance risk

Multiple independent deterministic signals indicate a material risk.

### Candidate

A suspicious pattern warrants investigation but is not proven to matter at runtime.

Ferric Lens must never call a static risk a measured runtime bottleneck.

Examples of relevant static runtime signals may include:

- unnecessary or repeated allocation,
- repeated cloning/copying,
- repeated graph/tree traversal,
- algorithmic complexity risks,
- broad or repeated data movement,
- expensive repeated dependency paths.

## 13. Build and compile efficiency

Build efficiency is a separate first-class category from runtime performance.

Relevant evidence may include:

- generic/codegen expansion,
- monomorphization concentration,
- code-size contribution,
- dependency weight,
- transitive dependency expansion,
- incremental-build blast radius,
- compilation concentration.

Runtime and build findings must not be merged into one score.

## 14. Third-party dependencies

Ferric Lens deeply analyzes repository-owned Rust code.

Third-party dependencies receive shallow objective analysis only.

Relevant external-dependency evidence may include:

- fan-in,
- enabled features,
- duplicate versions,
- transitive dependency depth,
- which workspace components introduce the dependency,
- build reach,
- public API exposure.

Ferric Lens must not judge third-party implementation internals.

## 15. External deterministic evidence

Ferric Lens may import deterministic evidence generated elsewhere.

Examples:

- coverage results,
- mutation-test results,
- code-size measurements,
- compiler/codegen measurements,
- profiler artifacts.

Rules:

- external evidence is optional,
- core Ferric Lens analysis remains fully functional without it,
- missing evidence never counts against a repository,
- Ferric Lens does not automatically invoke external tools,
- imported evidence may refine confidence or priority,
- imported evidence must be identified in the analysis snapshot.

## 16. Source scopes

Production code is the primary analysis scope.

Ferric Lens should also recognize auxiliary source classes separately, including:

- tests,
- benches,
- examples,
- build scripts,
- generated code,
- proc-macro-related output where observable.

Auxiliary code must not distort the main production-code findings.

Generated code should not become a normal refactor target.

## 17. Rust configurations and platforms

Ferric Lens analyzes concrete Rust configurations independently.

It must not create a union graph that pretends mutually exclusive `cfg` branches coexist.

Findings must preserve the configuration in which they were observed.

Linux and macOS are first-class supported platforms.

A CI setup may use one canonical full analysis plus targeted platform validation so shared work is not unnecessarily duplicated.

## 18. Partial analysis

Ferric Lens should provide useful partial analysis when the target repository does not compile completely.

It must explicitly record analysis completeness.

Example categories:

```text
Cargo workspace       complete
Module structure      complete
Git evidence          complete
Dependency resolution partial
Type-level relations  unavailable
Build analysis        unavailable
```

Findings requiring unavailable evidence must not be emitted.

Partial analysis must never be presented as complete analysis.

## 19. Output

### 19.1 Static HTML

The primary human-facing artifact is one self-contained HTML file.

It must require:

- no server,
- no network,
- no external asset directory.

Presentation and interaction should use HTML and CSS wherever reasonably possible.

JavaScript is permitted only where equivalent behavior cannot reasonably be achieved with HTML/CSS.

Browser code only explores the immutable result. It performs no analysis.

### 19.2 Integrated codebase map

The report should revolve around the repository structure:

```text
workspace
  └─ crate
      └─ module
          └─ function/type
```

Evidence is overlaid on this model.

The report should make it possible to understand:

- priority,
- architecture/dependency relationships,
- refactor evidence,
- performance risks,
- build-cost evidence,
- change relevance,
- test-strength context,
- analysis completeness.

Specialized graph or matrix views may support the codebase map but should not become disconnected products.

### 19.3 JSON

JSON is a first-class machine interface.

It must represent the same canonical findings as the HTML report and CI decision.

## 20. Reproducibility

Given identical:

- repository state,
- Ferric Lens version,
- Rust toolchain,
- concrete analysis configuration,
- imported evidence,

Ferric Lens must produce equivalent canonical results.

Canonical output should avoid:

- timestamps,
- absolute local paths,
- random identifiers,
- unstable ordering,
- machine-specific noise.

Finding fingerprints must remain deterministic for the analyzed state.

## 21. Accepted findings

Intentional findings may be accepted explicitly.

Acceptances live in a tool-managed repository file:

```text
.ferric-lens/acceptances.toml
```

Users should not need to edit it manually.

An acceptance is tied to a deterministic finding fingerprint and includes a reason.

When the underlying evidence materially changes, the previous fingerprint no longer matches and the finding becomes visible again.

Ferric Lens suppresses specific understood findings, not broad categories of evidence.

## 22. Network contract

Ferric Lens must be fully functional offline after installation.

No analysis or report generation may depend on:

- GitHub APIs,
- remote package registries,
- telemetry services,
- external web services,
- remote configuration.

## 23. Toolchain contract

Ferric Lens uses stable Rust only.

Nightly-only compiler internals are outside the initial product contract.

## 24. Version behavior

Ferric Lens does not maintain a separately pinned ruleset version.

The installed Ferric Lens version defines the current desired analysis behavior.

If a newer version finds an issue that an older version did not, the newer result is authoritative.

## 25. Success criteria

Ferric Lens v1 is successful when a developer can point it at a Rust repository with no configuration and receive a reproducible result that clearly answers:

- what structural concerns exist,
- what changed relative to the branch baseline,
- which areas deserve attention first,
- which performance risks are proven versus candidates,
- which refactor candidates are supported by multiple signals,
- what broad structural direction the evidence suggests,
- whether the change should pass CI,
- what Ferric Lens could and could not analyze.

The tool should reduce the effort required to optimize and maintain Rust codebases without replacing engineering judgment.

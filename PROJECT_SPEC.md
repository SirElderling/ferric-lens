# Ferric Lens Project Specification

## 1. Product definition

Ferric Lens is a deterministic, zero-configuration Rust codebase analysis tool.

It produces a unified structural model of a Rust repository, identifies evidence-backed optimization and refactoring candidates, and can act as a CI gate.

The initial product is static and offline. It does not modify source code and does not depend on AI, network services, or project-specific configuration.

[ARCHITECTURE.md](ARCHITECTURE.md) defines the proposed implementation and records decision rationale. This specification defines behavior; the vision defines intent. Neither document claims that performance or rule precision has already been measured.

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

Both commands use the same engine and gate rules. `check` emits a compact summary and canonical JSON; HTML is optional. `analyze` adds the full report and bounded enrichment. With identical inputs, both must return the same gate verdict and gate findings. An inconclusive verdict also has the same meaning in both modes.

Default source input is the working tree, including tracked modifications and non-ignored untracked Rust files reachable from selected targets. Deleted files are absent. A CI run normally uses a clean checkout. Record a content digest; never label dirty content as the HEAD commit alone. Detect changes during reading and return inconclusive rather than mixing revisions.

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

Every relationship records its origin and resolution status. Keep declared dependencies, resolved imports, observed syntax, inferred relationships, and imported measurements distinct. Unknown edges are not absent edges. Configuration and source class are part of graph membership.

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

Use like-for-like populations (same item kind, source class, and concrete configuration). Freeze the baseline population for both sides of a gate comparison. Percentile movement alone is not deterioration. Tiny or degenerate populations produce descriptive metrics, not outlier-based gates. Built-in statistical definitions and minimum samples ship with the binary; users do not maintain them.

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

Evidence strength describes the claim, not permission to fail CI. For example, a syntax-level `.clone()` occurrence proves only that syntax exists; it does not establish allocation, unnecessary copying, or a runtime bottleneck. A complete observed dependency cycle proves a graph property, not a policy violation.

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

CI reports a regression only for a gate-eligible rule when either:

1. a proven regression is new or materially worsened, or
2. a new or worsened material regression satisfies the rule's independent-evidence quorum.

A single non-provable heuristic must never fail CI by itself.

Each shipped gate rule must define its scope, required capabilities, evidence families, material-change formula, baseline comparability, identity, and positive/negative fixtures. Two transformations of the same fact (such as fan-out and its percentile) count as one signal. Operational independence means distinct underlying observations supporting the same concern; it does not claim statistical independence.

V1 gates use core structural evidence only. History and external imports may refine advisory confidence and presentation but cannot create or remove a gate regression. The initial concrete rule and conservative thresholds are specified in ARCHITECTURE.md; measured false-positive validation is a release requirement, not an assumed result.

Exit codes for both commands:

| Code | Verdict | Meaning |
| --- | --- | --- |
| 0 | pass | All enabled gate checks completed; no unaccepted gate regression. |
| 1 | regression | At least one unaccepted gate regression established. |
| 2 | inconclusive/error | Required comparison/evidence unavailable, input changed, or execution failed. |

If a regression is established alongside missing required evidence, return 1 and record incomplete coverage. If no regression is established and required evidence is missing, return 2. Missing optional enrichment alone does not prevent a pass. Without a baseline, `analyze` still writes a useful report but returns 2 for the unavailable gate.

### 8.2 Existing debt

Existing unchanged findings do not fail a PR merely because they exist.

Ferric Lens should classify findings relative to the baseline as:

- new,
- worsened,
- resolved,
- unchanged,
- carried through structural movement.

Delta status, movement, and acceptance are separate fields. An accepted finding remains in JSON and HTML with its reason. A finding may be marked resolved only when the relevant evidence is complete on both sides; otherwise its delta is unknown.

### 8.3 Baseline source

Ferric Lens does not require a committed baseline snapshot.

Git is the source of truth.

For PR-style analysis, Ferric Lens should use merge-base-aware attribution rather than naive comparison against only the current tip of `main`.

Large changes must not receive relaxed quality standards. They must receive better attribution.

Resolve the target in this order: explicit `--base <ref>`, recognized local CI target ref, local remote-default symbolic ref, then local `main`. Never use the feature branch's tracking ref as an assumed PR target. Record the chosen ref and immutable object ID. If unavailable, ambiguous, equal to HEAD without an explicit base, or without one usable merge base, report inconclusive; never fetch or silently compare with an empty repository. `--base` selects an input, not a maintained policy.

Analyze the unique merge base and current source snapshot with the same Ferric Lens version and configuration. Missing baseline objects in a shallow clone require locally supplying the target/history. A merge-base comparison attributes branch changes; it does not certify compatibility with the latest target tip.

Structural movement is matched conservatively: stable qualified identity first, then unique exact structural matches ignoring location, comments, and whitespace. Ambiguous rewrites, splits, and merges remain unmatched with an explanation; never invent semantic equivalence or hide an eligible new finding using fuzzy matching.

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

History enrichment has versioned deterministic work limits and reports examined commits, exclusions, and truncation. Co-change denominators must describe the sampled commit population; a path-filtered history query must not discard the other paths needed to establish co-change. No full-repository pairwise co-change matrix is required.

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

V1 uses Cargo crates and Rust modules as boundaries. It reports observable coupling, cycles, public declarations, and change concentration. Automatic semantic responsibility clustering, complete call graphs, type/behavior ownership inference, and architecture-policy generation are deferred. This preserves useful structural guidance without requiring compiler-grade semantics or speculative clustering.

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

V1 syntax patterns are candidates unless a rule proves its exact source-level claim. Method names alone do not identify allocator behavior or concrete types. Suggestions must state the unknowns and the evidence needed to validate them; no estimated milliseconds, bytes allocated, or speedups may be invented.

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

Core v1 reports dependency reach, feature/dependency structure where resolved, and potential rebuild exposure. Monomorphization counts, code-size contribution, and actual compilation cost require matching imported measurements. Generic syntax counts and graph reach must be labelled structural proxies, never measured build cost.

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

V1 accepts one versioned normalized local JSON envelope, not a collection of vendor-specific parsers. It includes producer/version, source digest or clean commit, target/features, units, and repository-relative subjects. Validate schema, size, source/configuration match, and paths. Reject malformed imports; retain stale or mismatched imports only as explicitly unattached context, never as current evidence. Cross-revision measurements require matched artifacts for both sides. This envelope is an exchange interface; project configuration remains unnecessary.

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

The default is one host target, default Cargo features, and production library/binary targets. Explicit target/feature input selects additional concrete runs; v1 does not enumerate the feature powerset or assume `--all-features` is valid. Record rustc target cfg facts, selected features, Cargo resolution identity, and relevant input digests.

Unknown build-script cfg, unresolved feature activation, and macro-generated structure are unknown, not false. Exclude unresolved branches from definite graphs and mark affected capabilities incomplete. Do not gate rules that require those missing facts. Combining platform results preserves separate configuration keys; cache only facts actually shared across configurations.

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

Completeness is tracked per capability, subject, configuration, and baseline side, rather than one repository-wide boolean. Each rule declares its dependencies. A compiler error in unrelated code must not erase reliable syntax findings elsewhere; missing evidence within a rule's required scope cannot be treated as a zero metric.

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

V1 uses a linked hierarchy, summary tables, and native `details` disclosure. Avoid a force-directed graph engine and rendering every dependency edge into the DOM. Summarize repeated evidence, retain all findings, and link findings to compact source excerpts rather than embedding every source file. Report deterministic excerpt/display limits explicitly. Escape all repository/imported strings as untrusted content. The report must remain useful with JavaScript disabled.

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

Publish a schema version independently of the tool version. Use explicit unavailable/null states, deterministic ordering, stable string identifiers, exact integer metrics, and documented units. Runtime timings and cache statistics belong in separate diagnostic output, not canonical artifacts.

## 20. Reproducibility

Given identical:

- repository state,
- Ferric Lens version,
- Rust toolchain,
- concrete analysis configuration,
- imported evidence,
- resolved baseline and available Git objects,
- source and acceptance-file contents,
- Cargo resolution and relevant cfg/environment facts,

Ferric Lens must produce equivalent canonical results.

Canonical output should avoid:

- timestamps,
- absolute local paths,
- random identifiers,
- unstable ordering,
- machine-specific noise.

Finding fingerprints must remain deterministic for the analyzed state.

Canonical JSON and HTML must be byte-identical for identical effective inputs, regardless of thread scheduling, cache hits, or checkout location. The snapshot records the exact bounded history slice and coverage. Resource interruption can produce an explicit partial result but must not be mistaken for the complete canonical result.

## 21. Accepted findings

Intentional findings may be accepted explicitly.

Acceptances live in a tool-managed repository file:

```text
.ferric-lens/acceptances.toml
```

Users should not need to edit it manually.

An acceptance is tied to a deterministic finding fingerprint and includes a reason. Separate entity identity (for movement/baseline matching) from evidence fingerprint (for accepting an exact material condition).

When the underlying evidence materially changes, the previous fingerprint no longer matches and the finding becomes visible again.

Ferric Lens suppresses specific understood findings, not broad categories of evidence.

The evidence fingerprint includes rule semantics, configuration, stable subject identity, and material core evidence; exclude line numbers, report ordering, optional enrichment, and timestamps. Pure movement can preserve acceptance only with an unambiguous identity match. An ambiguous match cannot inherit acceptance. New rule semantics or materially changed core evidence invalidate acceptance.

`ferric-lens accept <fingerprint> --reason <text>` reads a current local result, verifies its source digest, and atomically updates only the acceptance file. Existing acceptances are applied after comparison; accepted findings remain visible and auditable. Invalid/stale result input cannot create an acceptance.

## 22. Network contract

Ferric Lens must be fully functional offline after installation.

No analysis or report generation may depend on:

- GitHub APIs,
- remote package registries,
- telemetry services,
- external web services,
- remote configuration.

Native Git and stable Cargo/rustc are local prerequisites for their capabilities. Use Cargo metadata in offline, locked mode; never build, invoke project scripts, install toolchains, or change Cargo.lock. If metadata cannot resolve locally, fall back to manifest/source facts with explicit missing capabilities. Offline operation does not imply that uncached third-party metadata becomes available.

## 23. Toolchain contract

Ferric Lens uses stable Rust only.

Nightly-only compiler internals are outside the initial product contract.

## 24. Version behavior

Ferric Lens does not maintain a separately pinned ruleset version.

The installed Ferric Lens version defines the current desired analysis behavior.

If a newer version finds an issue that an older version did not, the newer result is authoritative.

Recompute both baseline and head using that same version. Never classify a new rule's discovery of unchanged historical debt as a PR regression merely by comparing output from different tool versions.

## 25. Test-driven development contract

Test-driven development is a core project requirement, not an optional implementation preference.

For every behavior change, bug fix, rule, parser capability, output contract, or regression:

1. derive the expected behavior from this specification and the public contract,
2. add or change a focused automated test first,
3. run it and confirm that it fails for the intended reason,
4. implement the smallest change that makes the test pass,
5. refactor only while the full suite remains green.

A change is not complete merely because the implementation appears correct. Its externally observable behavior must be covered by deterministic automated tests. Bug fixes require a regression test that demonstrates the bug before the fix. Gate rules require positive, negative, boundary, incomplete-evidence, baseline, and acceptance cases where applicable. Determinism and output-contract changes require repeatability and cross-artifact consistency tests.

Tests must assert behavior rather than mirror implementation details. Prefer public CLI and analysis-model contracts for integration tests and small focused unit tests for deterministic algorithms. Tests must be offline, reproducible, isolated from user/global Git and Cargo configuration where relevant, and must not depend on wall-clock time, network access, random ordering, or mutable external state.

CI must treat the automated test suite as a required gate on Linux and macOS. Linux CI must additionally enforce 100% line, region, and function coverage for repository-owned production Rust code using a deterministic coverage tool. Any uncovered production path fails CI. Platform-specific code that can only execute on macOS must be covered by the macOS suite and must not be hidden through coverage exclusions. Generated code and third-party dependencies are outside this coverage denominator. Coverage exclusions for repository-owned production code are not permitted merely to satisfy the threshold. No production behavior may be added solely to make an after-the-fact characterization test pass if that behavior is not supported by this specification.

The existing implementation predates this explicit TDD contract. Before v1 is considered complete, its already implemented behavior must be backfilled with specification-derived characterization and regression tests. Those tests are to be designed from the documented behavior first and then run against the implementation; failures indicate either an implementation gap or a specification/test mismatch that must be resolved explicitly.

## 26. Success criteria

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

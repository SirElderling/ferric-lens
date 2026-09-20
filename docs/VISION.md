# Ferric Lens Vision

## Purpose

Ferric Lens is a deterministic, zero-configuration analysis tool for Rust codebases.

Its purpose is to answer one practical question:

> Where is this Rust codebase becoming structurally expensive, risky to change, slow, or unnecessarily costly, and what deterministic evidence supports that conclusion?

Ferric Lens is intended for everyday development, code review, refactoring, and CI. It should help optimize projects such as Jeko, Kronicle, and future Rust codebases without requiring teams to maintain hand-tuned thresholds, architecture descriptions, or analysis configuration.

## Product thesis

Most code-quality tools expose one narrow dimension at a time: dependency graphs, complexity, coupling, performance, history, coverage, mutation results, or build cost.

Ferric Lens takes a different approach.

It builds one coherent model of the current Rust codebase and combines independent deterministic signals. The primary experience is not a collection of unrelated dashboards and not a single opaque score. It is a codebase map that shows where attention is most valuable and explains the evidence behind every conclusion.

Ferric Lens should be useful even when run with no configuration.

The behavioral contract lives in [PROJECT_SPEC.md](PROJECT_SPEC.md). The implemented V1 boundaries, trade-offs, decision rationale, and release-validation criteria live in [ARCHITECTURE.md](ARCHITECTURE.md).

## Core principles

### Deterministic by design

The same repository state, Ferric Lens version, Rust toolchain, analysis profile, and imported evidence must produce the same canonical findings and report content.

Ferric Lens must avoid hidden inputs, random ordering, machine-specific paths, timestamps in canonical artifacts, or environment-dependent scoring.

Resolved baseline, available history, source contents, Cargo resolution, and configuration are explicit snapshot inputs. Cache availability and worker scheduling must never change conclusions.

### Zero configuration by default

Ferric Lens should not require users to choose complexity limits, fan-out thresholds, or architecture rules before receiving useful results.

It should derive value from:

- objective invariants,
- repository-relative structural outliers,
- corroboration across independent signals,
- baseline regressions,
- targeted Git history,
- Rust and Cargo structure.

Project-specific policy is not part of the initial product.

### Evidence over opinion

Every finding must explain why it exists.

Ferric Lens should report measurable facts such as dependency relationships, graph structure, complexity, churn, change coupling, public-surface expansion, build reach, code-size contribution, allocation patterns, or imported test evidence.

It must not make unsupported claims such as "this code is bad" or "this function is definitely slow."

### Corroboration over noisy heuristics

One heuristic should not block CI.

Non-provable findings become important only when multiple independent signals agree and the change is materially worse than the baseline.

A single unusual metric may be worth observing. A cluster of independent signals may justify action.

An observable fact is not automatically a harmful regression: an added dependency, cycle, or clone must not fail CI merely because its presence can be proven. Gate eligibility requires a published rule with a material-change predicate and sufficient comparable evidence.

### Static performance analysis without false certainty

Ferric Lens distinguishes:

- proven static costs,
- strong performance-risk evidence,
- performance candidates.

Static analysis must never label a risk candidate as a measured runtime bottleneck.

Runtime efficiency and build/compile efficiency are separate first-class concerns.

### Refactoring guidance without automated redesign

Ferric Lens identifies refactor candidates and may describe a deterministic structural direction supported by evidence, such as:

- split mixed responsibilities,
- reduce dependency surface,
- isolate volatility,
- move behavior closer to owned data,
- break dependency cycles,
- reduce repeated traversal,
- reduce unnecessary allocation or copying,
- narrow public surface,
- introduce a clearer boundary.

Ferric Lens does not generate, apply, or decide the final refactor.

### Current attention matters most

Ferric Lens models the full repository, but the default presentation prioritizes:

1. new problems introduced by the current change,
2. existing hotspots worsened by the current change,
3. existing hotspots touched by the current change,
4. unchanged repository-wide findings.

The purpose is to help developers decide where attention is most valuable now.

### History is evidence, not the product

Git history may strengthen findings through churn, temporal coupling, or repeated instability, but Ferric Lens is not a repository analytics dashboard.

History analysis should be targeted, bounded, and performed using native Git tooling.

### Large changes require better attribution, not relaxed standards

Large PRs, long-lived branches, moves, renames, crate splits, and structural rewrites should not be treated as hundreds of unrelated new problems.

Ferric Lens should distinguish structural movement from genuine deterioration by using merge-base-aware analysis and stable structural finding identity.

### Offline by default

After installation, Ferric Lens must be fully functional without network access.

It analyzes local repository state, local Cargo metadata, local Git history, and explicitly supplied local evidence.

Missing local dependency artifacts reduce resolution coverage, not offline usability. The default analysis does not compile the project, execute build scripts, expand procedural macros, or fetch missing data.

### Small operational footprint

One short-lived native process should do the useful work, then exit. No daemon, database service, compiler fork, embedded browser, or frontend build system is required.

Parse source once per content version, retain compact facts, reuse unchanged work, and enrich only relevant findings. Resource limits must be visible when they limit coverage; fast must never mean silently incomplete.

### Stable Rust only

Ferric Lens should build and operate using stable Rust.

Nightly compiler internals are not part of the product contract.

### Focused scope

Ferric Lens is not a general correctness, security, or vulnerability scanner.

It focuses on:

- architecture and dependency structure,
- coupling,
- structural complexity,
- refactor candidates,
- runtime performance risks,
- build and compile efficiency,
- change hotspots,
- blast radius,
- dependency cost,
- test-strength context when evidence is supplied.

Existing Rust tools remain responsible for conventional linting, memory safety diagnostics, vulnerabilities, and other specialist domains.

## Primary experience

The primary human-facing output is one self-contained static HTML report.

The report should present a unified codebase map:

```text
workspace
  └─ crate
      └─ module
          └─ item
```

Architecture, coupling, history, performance-risk, build-cost, refactoring, and test evidence are overlays on that structure rather than separate products.

The report should prefer HTML and CSS for interaction and presentation. JavaScript should be used only where native HTML/CSS cannot reasonably provide the required behavior.

Ferric Lens performs no analysis in the browser. The HTML report is an interactive viewer over an immutable analysis snapshot.

Machine-readable JSON is a first-class output alongside the HTML report.

## CI philosophy

Ferric Lens is intended to run as a CI gate.

CI should be powerful without becoming noisy.

A gate fails when:

- a gate-eligible proven regression is introduced or materially worsened, or
- a new or worsened material regression satisfies a published quorum rule.

The outcome is pass, regression, or inconclusive. Inconclusive means evidence needed for the gate is missing; it is not a clean pass. Optional history or imported evidence cannot change the v1 gate decision.

Existing debt does not fail a PR merely because it exists.

The baseline comes from Git rather than a separately maintained health snapshot.

For large or divergent branches, Ferric Lens should reason from the merge base and distinguish moved or renamed structures from genuinely new or worsened findings.

## Platform model

Ferric Lens must support Linux and macOS.

Shared analysis should not be duplicated unnecessarily across platforms. Platform-specific Rust configurations should be analyzed as real configurations and their findings merged afterward.

Ferric Lens must never pretend mutually exclusive `cfg` configurations coexist.

## Extensibility without hidden dependencies

Ferric Lens owns its static analysis.

It may optionally import deterministic evidence produced elsewhere, such as:

- coverage data,
- mutation-test results,
- code-size data,
- compiler or codegen data,
- profiler output.

External evidence may refine confidence or priority but is never required for core operation and is never invoked implicitly.

## Trust model

Ferric Lens should always make its limits visible.

Reports should distinguish:

- complete analysis,
- partial analysis,
- unavailable evidence,
- unsupported constructs,
- excluded/generated/support code.

Partial evidence may produce partial insight, but never false certainty.

V1 starts with syntax, explicit resolvable relationships, Cargo structure, and bounded history. Full type inference, inferred semantic ownership, macro expansion, and measured codegen/runtime costs are not implied by a successful source scan. Measurements are shown only when supplied with matching provenance.

## What success looks like

Ferric Lens succeeds when a developer can run one binary against a Rust repository and quickly understand:

- what changed structurally,
- what newly deserves attention,
- which areas are becoming expensive or risky,
- which findings are strongly corroborated,
- why each finding exists,
- what broad structural direction may improve it,
- whether the current change should pass CI.

It should provide more power with less noise, without requiring the user to become an expert in static-analysis configuration.

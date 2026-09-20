# Ferric Lens

Ferric Lens is a deterministic, zero-configuration analysis tool for Rust repositories.

It helps answer:

- What changed structurally?
- What deserves attention first?
- Did this change make an existing hotspot worse?
- Are there architecture, complexity, dependency, copy-risk, or build-cost signals worth investigating?
- Should this change pass the Ferric Lens CI gate?

Ferric Lens is designed to be conservative. It reports evidence and limitations instead of treating every unusual metric as a defect.

## Quick start

If the binary is installed:

```bash
ferric-lens check .
```

When developing from this repository, replace `ferric-lens` with:

```bash
cargo run -- 
```

For example:

```bash
cargo run -- check .
```

## Common workflows

### Check a pull request against main

From the PR or feature-branch checkout:

```bash
git fetch origin main
ferric-lens check . --base origin/main
```

Ferric Lens analyzes the current repository and its merge base with `origin/main`, then gates regressions introduced or materially worsened by the current change.

This is the closest workflow to "check the PR only", but Ferric Lens does **not** scan only changed files. It still needs repository-wide facts for dependency graphs, reference populations, and correct attribution. Existing unchanged debt does not fail the PR merely because it exists.

To save the complete machine-readable result as well:

```bash
ferric-lens check . --base origin/main --json ferric-lens.json
```

### Analyze a pull request in detail

Use `analyze` when you want the full human report:

```bash
ferric-lens analyze . --base origin/main
```

By default this writes:

```text
ferric-lens.json
ferric-lens-report.html
```

Open the HTML report first. It is the easiest way to review introduced or worsened findings, existing findings, source evidence, and analysis limitations.

### Analyze current main

Checkout main and use the current commit as its own baseline:

```bash
git switch main
ferric-lens analyze . --base HEAD
```

This is useful for inspecting the current repository state without treating historical debt as a new branch regression. Because the baseline and current commit are the same, change attribution is effectively unchanged/current-state analysis.

### Compare the current branch with another branch or release

Any Git ref can be used as the comparison target:

```bash
ferric-lens check . --base origin/release
```

or:

```bash
ferric-lens analyze . --base v1.0.0
```

Ferric Lens resolves the merge base between the current repository state and the supplied ref.

### Let Ferric Lens choose the baseline

```bash
ferric-lens check .
```

Without `--base`, Ferric Lens tries the GitHub PR target, then the remote default branch, then local `main`.

For important CI workflows, an explicit `--base` is usually easier to reason about.

### Analyze a target and Cargo features

Analyze a specific Rust target:

```bash
ferric-lens analyze . \
  --base origin/main \
  --target aarch64-apple-darwin
```

Enable one or more additional Cargo features:

```bash
ferric-lens analyze . \
  --base origin/main \
  --feature telemetry \
  --feature fast-path
```

Each invocation analyzes one concrete target/profile. Run Ferric Lens more than once when you need separate platform or feature combinations.

### Produce compact output for an AI agent

```bash
ferric-lens check . --base origin/main --ai
```

or:

```bash
ferric-lens analyze . --base origin/main --ai
```

`--ai` prints compact deterministic JSON to stdout. It prioritizes actionable findings, bounded source evidence, inspection questions, and limitations.

With `analyze --ai`, JSON and HTML files are **not** written by default. Request them explicitly when needed:

```bash
ferric-lens analyze . \
  --base origin/main \
  --ai \
  --json ferric-lens.json \
  --html ferric-lens-report.html
```

### Add external measured evidence

Full analysis can attach a normalized local evidence file:

```bash
ferric-lens analyze . \
  --base origin/main \
  --evidence measurements.json
```

Imported evidence is advisory in V1. It can add context, but it does not change the `check` gate verdict.

## What Ferric Lens does

Ferric Lens builds a deterministic model of the Rust repository and uses it to inspect areas such as:

- module and dependency structure,
- explicit repository dependency edges and cycles,
- structural complexity,
- dependency surface and blast radius,
- changed-code regressions against a Git baseline,
- refactor candidates supported by compatible evidence,
- contextual copy-risk patterns such as clone-for-iteration and clone-then-mutate,
- bounded Git churn and co-change context in full analysis,
- build/rebuild exposure indicators,
- a deliberately small set of deterministic correctness-risk patterns,
- optional imported measurements.

Findings keep observed facts separate from interpretation. Candidate evidence is not presented as a measured runtime bottleneck.

## What Ferric Lens does not do

Ferric Lens does not:

- compile or run the analyzed project during normal source analysis,
- execute build scripts,
- profile runtime performance,
- claim that static syntax proves an allocation or bottleneck,
- expand procedural macros,
- provide full Rust type inference or a complete semantic call graph,
- replace Clippy, Miri, vulnerability scanners, or specialist correctness tools,
- automatically rewrite or refactor source code,
- require AI or a network service,
- use one opaque repository quality score.

When evidence is incomplete, Ferric Lens reports the limitation instead of inventing certainty.

## Commands and options

| Command / option | Purpose |
| --- | --- |
| `check [PATH]` | CI-oriented analysis. Prints a concise result and returns a gate exit code. |
| `analyze [PATH]` | Full analysis with richer advisory evidence. Writes JSON and HTML by default. |
| `accept <FINGERPRINT>` | Accept one exact current finding with a required reason. |
| `PATH` | Repository path. Defaults to the current directory. |
| `--base <REF>` | Compare against the merge base with a Git ref such as `origin/main`, `HEAD`, or a tag. |
| `--target <TRIPLE>` | Analyze a specific Rust target triple. Defaults to the host target. |
| `--feature <NAME>` | Enable an additional Cargo feature. Repeat for multiple features. |
| `--json <PATH>` | Write canonical machine-readable JSON. |
| `--html <PATH>` | `analyze` only: write the self-contained HTML report. |
| `--evidence <PATH>` | `analyze` only: attach a normalized local evidence envelope. |
| `--ai` | Print compact deterministic JSON intended for agents instead of the human CLI summary. |
| `accept --reason <TEXT>` | Required explanation for accepting the exact finding. |
| `accept --path <PATH>` | Repository path for `accept`; defaults to the current directory. |

Use `ferric-lens <command> --help` for the CLI-generated help for the installed version.

## Understanding the result

Ferric Lens has three gate outcomes:

- `Pass` — no enabled gate found a qualifying regression with the available evidence.
- `Regression` — an enabled gate found a qualifying new or worsened regression.
- `Inconclusive` — required evidence was missing or analysis could not establish a reliable gate result.

Process exit codes are:

```text
0 = pass
1 = regression
2 = inconclusive or operational error
```

A pass is not a claim that the repository has no technical debt. It means the enabled gate did not establish a blocking regression for the analyzed comparison.

## What to do with the output

Start with the human CLI summary or HTML report.

For each finding:

1. Check whether it is introduced/worsened by the current change or pre-existing.
2. Read the observed facts and source evidence before the interpretation.
3. Check the stated limitations; do not treat a candidate as a proven runtime problem.
4. Follow the bounded inspection questions to decide whether the code needs a change.
5. Re-run Ferric Lens after making the change and compare the result.

Use the outputs for different jobs:

- **HTML** — human review and investigation.
- **Canonical JSON** — CI artifacts, auditing, automation, or downstream tooling.
- **`--ai` JSON** — compact context for an AI coding/review agent.
- **CLI exit code** — CI pass/fail/inconclusive control flow.

If a finding is intentional and should remain visible but not block the exact current condition, accept its fingerprint:

```bash
ferric-lens accept <fingerprint> \
  --reason "Intentional boundary for this design" \
  --base origin/main
```

Ferric Lens writes the acceptance to:

```text
.ferric-lens/acceptances.toml
```

Review and commit that file when the acceptance is part of the repository policy. A material change to the evidence produces a different fingerprint, so an old acceptance does not broadly suppress future findings.

## Documentation

- [Vision](docs/VISION.md)
- [Project specification](docs/PROJECT_SPEC.md)
- [Architecture and decision record](docs/ARCHITECTURE.md)

# Ferric Lens

Ferric Lens is a deterministic, zero-configuration structural analysis tool for Rust codebases.

The product contract is defined in [VISION.md](VISION.md), [PROJECT_SPEC.md](PROJECT_SPEC.md), and [ARCHITECTURE.md](ARCHITECTURE.md).

## Current V1 implementation

Ferric Lens currently:

- inventories production Rust sources from Cargo library/binary target roots and follows reachable `mod` declarations, with an explicit partial-coverage fallback,
- excludes orphan `.rs` files from Cargo-backed production analysis and keeps same-named library/binary targets distinct,
- parses Rust syntax without compiling or executing the target project,
- measures module decision sites, declared public items, explicit imports, and resolvable local dependency breadth,
- identifies advisory current-snapshot structural outliers,
- synthesizes first-class refactoring candidates when at least two independent deterministic signal kinds corroborate the same module, with evidence and a bounded structural direction rather than a generated redesign,
- renders a human-first HTML report led by **What needs attention**, with plain-language consequences, next investigation steps, source evidence, and technical rule IDs kept secondary,
- resolves a Git target and analyzes the unique merge base under the same rules as the current tree,
- conservatively matches modules by stable identity, Git rename, then unique normalized structure,
- gates the documented `structure.coupled_complexity_growth` regression only when both independent signals materially worsen,
- evaluates standard Rust target cfg predicates for one concrete target profile per invocation,
- supports explicit additional Cargo features without inventing a feature powerset,
- reports unresolved custom/default-feature cfg evidence as `inconclusive` rather than pretending the gate passed,
- emits deterministic canonical JSON for complete machine/audit use,
- emits a compact deterministic `--ai` JSON view for agents that keeps actionable findings, evidence, source contexts, result identity, and analysis limitations while omitting raw repository inventory,
- emits one self-contained HTML/CSS report with no JavaScript,
- prints a concise human-oriented CLI summary by default rather than a raw metric dump,
- attaches deterministic source evidence to supported findings: repository-relative path, exact 1-based line span, and a bounded escaped excerpt, while retaining module navigation when no truthful syntax span exists,
- reuses content-addressed raw syntax facts through a disposable 256 MiB repository cache,
- enriches full `analyze` reports with bounded recent churn and co-change evidence,
- keeps `check` on the fast core path without optional history or imported evidence,
- reports resolved repository dependency edges and observed explicit-import cycles as architecture evidence,
- accepts one validated, normalized local JSON evidence envelope for full-report enrichment,
- supports fingerprinted, reasoned finding acceptances in a tool-managed repository file,
- keeps accepted findings visible while excluding only exact accepted gate evidence from failure,
- embeds the same semantic result digest in JSON and HTML so mixed artifact generations are detectable,
- publishes JSON/HTML files through atomic replacement,
- uses exit codes `0=pass`, `1=regression`, and `2=inconclusive/error`.

Existing unchanged debt does not fail a branch.

## Usage

Analyze a repository and write JSON plus HTML:

```bash
cargo run -- analyze /path/to/rust/repository --base origin/main
```

Analyze a concrete non-host target or enable additional Cargo features:

```bash
cargo run -- analyze /path/to/rust/repository \
  --base origin/main \
  --target aarch64-apple-darwin \
  --feature fast-path \
  --feature telemetry
```

Each invocation analyzes one real profile. The default profile is the host target with Cargo default features. Repeated `--feature` options add explicit features to the default feature set. Ferric Lens records the resolved target, selected features, and canonical `rustc --print cfg` facts in the result.

This writes:

```text
ferric-lens.json
ferric-lens-report.html
```

Run the CI-oriented path:

```bash
cargo run -- check /path/to/rust/repository --base origin/main --json ferric-lens.json
```

For an AI agent or another tool that wants the actionable subset without the full repository inventory, add `--ai`:

```bash
cargo run -- analyze /path/to/rust/repository --base origin/main --ai
cargo run -- check /path/to/rust/repository --base origin/main --ai
```

`--ai` changes stdout only. It emits compact deterministic JSON containing the verdict, result digest, active findings, plain-language meaning, core evidence, exact source contexts where available, recommended next investigation step, and non-complete analysis capabilities. It intentionally omits the full module/function/type inventory and complete capability dump. The normal canonical JSON file remains the complete machine-readable record.

`--base` is optional. Without it, Ferric Lens tries the GitHub PR target, the local remote-default branch, then local `main`. It always compares against the unique merge base, not the moving target tip.

The initial blocking rule requires a baseline crate population of at least 20 production modules with complete required evidence. Smaller crates still receive descriptive/advisory output.

Full `analyze` mode samples at most 2,000 recent non-merge commits and 100,000 changed-path records for advisory history context. Commits touching more than 200 paths are excluded from co-change calculations and reported as such. History never changes the gate verdict.

## Importing deterministic evidence

Full `analyze` mode can attach an optional normalized local evidence envelope:

```bash
cargo run -- analyze /path/to/repository \
  --base origin/main \
  --evidence measurements.json
```

The V1 envelope is intentionally generic rather than vendor-specific:

```json
{
  "schema_version": 1,
  "producer": {"name": "my-benchmark", "version": "1.0"},
  "source": {"content_digest": "<Ferric Lens snapshot digest>"},
  "configuration": {"target": "host", "features": []},
  "observations": [
    {
      "subject": "src/engine.rs",
      "metric": "instructions",
      "value": 123456,
      "unit": "count",
      "note": "representative simulation workload"
    }
  ]
}
```

Imports are limited to 16 MiB, require repository-relative subjects, and are sorted deterministically. Source/configuration matches are explicit: the evidence target and explicit feature list must match the active Ferric Lens profile. Mismatched evidence is retained only as unattached context.

Imported evidence is advisory in V1. It never changes the `check` gate verdict and Ferric Lens never invokes the producing tool automatically.

## Accepting an intentional finding

Every finding has a deterministic fingerprint. To accept one exact current condition:

```bash
cargo run -- accept <fingerprint> \
  --reason "Intentional boundary for the current design" \
  --base origin/main
```

Ferric Lens verifies that the fingerprint exists in a fresh local analysis, re-checks the source digest immediately before writing, and atomically updates:

```text
.ferric-lens/acceptances.toml
```

The file is intended to be reviewed and committed. Accepted findings remain visible in JSON and HTML with their reason.

An acceptance does not suppress a rule broadly. Material evidence changes produce a different fingerprint, so the finding becomes active again automatically. An unambiguous pure move can preserve the stable entity identity.

## Evidence limits

Ferric Lens does not expand macros or pretend mutually exclusive platform `cfg` branches coexist. Standard target cfg predicates are evaluated from the selected target's stable `rustc --print cfg` output. Explicitly requested features can satisfy matching `cfg(feature = "...")`; unselected feature cfg remains unknown because default/transitive feature activation is not inferred from source alone. A changed gate subject or required baseline population affected by unsupported evidence makes the relevant gate inconclusive.

Static findings are not runtime profiling claims.

Finding source context is representative rather than an exhaustive source dump. V1 records at most three source contexts per supported evidence signal. Each excerpt is limited to three source lines and 600 Unicode characters; truncation is explicit. Current exact-span support covers decision-site, clone-call, resolved dependency, and reverse-dependency evidence. Line/excerpt metadata is presentation evidence and does not participate in finding fingerprints or acceptance identity.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
python3 -m unittest tools.test_perf_fixture tools.test_perf_acceptance
cargo build --release --locked
```

GitHub Actions runs verification on GitHub-hosted Linux and macOS runners with full Git history. Each runner performs formatting, Clippy with warnings denied, all tests, release-acceptance tooling tests, a real Ferric Lens self-check against `origin/main`, and a release build.

A separate performance-acceptance workflow generates a deterministic 20-crate, 100,000-production-line repository with 200 fixed history commits, runs cold `analyze` and warm `check`, records sampled process-tree RSS, wall/CPU time, cache/report sizes, toolchain and hardware, and uploads the evidence. The documented engineering targets are reported but never used as correctness gates on shared CI hardware.

Run the same standard fixture locally after a release build:

```bash
python3 tools/perf_acceptance.py \
  --binary target/release/ferric-lens \
  --mode standard \
  --output-dir target/perf-acceptance
```

The workflow can also be dispatched with `source-10x` (1,000,000 production Rust lines) or `history-10x` (2,000 history commits after baseline). Browser DOM/open cost and private-project precision checks remain explicit release-acceptance observations rather than hidden dependencies of the tool.

Pull-request CI also uploads the locked release executable from each Linux/macOS runner as a human-test artifact. See [HUMAN_TESTING.md](HUMAN_TESTING.md) for the Jeko/Kronicle validation protocol and finding scorecard.


## Output surfaces

Ferric Lens has three intentionally different presentation surfaces built from the same canonical result:

- **HTML for people** — starts with what deserves attention. Each finding explains what Ferric Lens noticed, why it matters, what could happen if the concern grows, what evidence supports it, what to investigate next, and where to look in source. Repository metrics without a finding are explicitly context, not warnings.
- **Default CLI for people and CI logs** — concise verdict plus the highest-priority active findings, using the same plain-language titles and next steps as HTML.
- **`--ai` for agents** — compact deterministic JSON with the actionable subset. Agents do not need the explanatory page layout or the full repository inventory, but they still receive evidence, source locations, rule identity, limitations, and the canonical result digest.

The full canonical JSON remains the lossless interface when a consumer needs every module, capability, imported observation, or structural fact.

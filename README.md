# Ferric Lens

Ferric Lens is a deterministic, zero-configuration structural analysis tool for Rust codebases.

The product contract is defined in [VISION.md](VISION.md), [PROJECT_SPEC.md](PROJECT_SPEC.md), and [ARCHITECTURE.md](ARCHITECTURE.md).

## Current V1 implementation

Ferric Lens currently:

- inventories production Rust sources from local Cargo metadata, with an explicit partial-coverage fallback,
- parses Rust syntax without compiling or executing the target project,
- measures module decision sites, declared public items, explicit imports, and resolvable local dependency breadth,
- identifies advisory current-snapshot structural outliers,
- resolves a Git target and analyzes the unique merge base under the same rules as the current tree,
- conservatively matches modules by stable identity, Git rename, then unique normalized structure,
- gates the documented `structure.coupled_complexity_growth` regression only when both independent signals materially worsen,
- reports incomplete macro/cfg evidence as `inconclusive` rather than pretending the gate passed,
- emits deterministic JSON,
- emits one self-contained HTML/CSS report with no JavaScript,
- supports fingerprinted, reasoned finding acceptances in a tool-managed repository file,
- keeps accepted findings visible while excluding only exact accepted gate evidence from failure,
- uses exit codes `0=pass`, `1=regression`, and `2=inconclusive/error`.

Existing unchanged debt does not fail a branch.

## Usage

Analyze a repository and write JSON plus HTML:

```bash
cargo run -- analyze /path/to/rust/repository --base origin/main
```

This writes:

```text
ferric-lens.json
ferric-lens-report.html
```

Run the CI-oriented path:

```bash
cargo run -- check /path/to/rust/repository --base origin/main --json ferric-lens.json
```

`--base` is optional. Without it, Ferric Lens tries the GitHub PR target, the local remote-default branch, then local `main`. It always compares against the unique merge base, not the moving target tip.

The initial blocking rule requires a baseline crate population of at least 20 production modules with complete required evidence. Smaller crates still receive descriptive/advisory output.

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

Ferric Lens does not expand macros or pretend mutually exclusive platform `cfg` branches coexist. A changed gate subject or required baseline population affected by unsupported evidence makes the relevant gate inconclusive.

Static findings are not runtime profiling claims.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

GitHub Actions runs verification on GitHub-hosted Linux and macOS runners with full Git history.

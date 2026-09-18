# Ferric Lens

Ferric Lens is a deterministic, zero-configuration structural analysis tool for Rust codebases.

The project is in early implementation. The product contract is defined in [VISION.md](VISION.md), [PROJECT_SPEC.md](PROJECT_SPEC.md), and [ARCHITECTURE.md](ARCHITECTURE.md).

## Current implementation slice

The initial executable can:

- inventory production Rust sources from local Cargo metadata, with a source-scan fallback,
- parse Rust syntax without compiling or executing the target project,
- measure module decision sites, declared public items, explicit imports, and resolvable local dependency breadth,
- identify advisory current-snapshot modules that are simultaneous repository-relative outliers,
- emit deterministic JSON,
- emit one self-contained HTML report using HTML/CSS only,
- expose `analyze` and `check` commands with the documented three-state gate model.

Baseline comparison and blocking regression rules are intentionally not enabled yet. Until the baseline slice lands, the gate verdict is `inconclusive` (exit code `2`) while advisory analysis remains available.

## Usage

```bash
cargo run -- analyze /path/to/rust/repository
```

This writes:

```text
ferric-lens.json
ferric-lens-report.html
```

For the CI-oriented path:

```bash
cargo run -- check /path/to/rust/repository --json ferric-lens.json
```

`check` currently returns `2` because baseline attribution is not implemented yet.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

CI runs the same verification on GitHub-hosted Linux and macOS runners.

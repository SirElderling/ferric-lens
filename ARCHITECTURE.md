# Ferric Lens architecture and decision record

Status: implemented for Ferric Lens V1 in PR #2. This document records the implemented architecture, decision rationale, deliberate V1 limits, and pre-release validation criteria. Behavioral requirements are in [PROJECT_SPEC.md](PROJECT_SPEC.md); product intent is in [VISION.md](VISION.md). Performance/resource targets and real-project precision remain release-acceptance measurements and are not implied by implementation status.

## 1. Architecture in one page

Use one synchronous Rust application with a library core and a thin CLI, initially in one Cargo package. Internal modules provide boundaries; separate crates are justified later by measured build or reuse needs. No async runtime, daemon, plugin framework, database, or frontend toolchain is required.

| Module | Owns | Must not do |
| --- | --- | --- |
| `input` | Immutable source inventory, manifests, configurations, baseline selection, input digests | Infer findings or mutate source |
| `git` | Native Git subprocesses, object reads, changes, bounded history | Run shell strings, fetch, or check out over user files |
| `extract` | Parsing and compact per-file facts with spans and capability status | Assume a name resolves or run project code |
| `model` | Stable IDs, indexes, typed edges, configuration-specific graphs | Store UI state or compiler ASTs indefinitely |
| `rules` | Pure fact-to-finding functions and capability requirements | Read files, launch processes, or render HTML |
| `compare` | Conservative movement matching, deltas, material fingerprints, acceptances, gate verdict | Infer semantic equivalence from fuzzy similarity |
| `report` | Canonical JSON, static HTML, compact CLI summary | Recompute rule semantics |
| `cache` | Disposable, versioned content-addressed fact storage | Become a required baseline or source of truth |

The orchestrator freezes inputs, extracts/reuses facts, builds graphs, evaluates and compares core rules, optionally enriches advisories, then serializes one immutable result. Both commands call this pipeline with different requested outputs/enrichment, not different analyzers. Keep the frozen gate result separate from advisory ranking.

Use ordinary structs, enums, vectors, and explicit module APIs. Add traits at the filesystem/process boundary for fixture testing; do not create a generic analysis framework. Prefer a mature stable Rust parser, CLI parser, serialization/TOML libraries, and a content hash implementation over custom equivalents. Select exact dependencies during implementation after checking supported Rust versions and transitive size. No compiler internals, rust-analyzer embedding, libgit2, or custom Cargo resolver in v1.

**Reason:** most complexity is evidence correctness and attribution. A small in-process design minimizes integration, deployment, memory, and failure surfaces without preventing later optimization.

## 2. Input and Rust capability boundary

Inventory reachable source files from Cargo targets and module declarations. Track raw path identity separately from escaped display text, including non-UTF-8 Unix paths. Do not follow paths or symlinks outside the selected repository; record external path dependencies as unavailable/shallow inputs. Ignore target output and tool caches. Generated source is not an ordinary refactor target.

Capture tracked/untracked content hashes and relevant manifest/configuration inputs. Verify them before publishing; concurrent edits invalidate the snapshot. Read baseline files from immutable Git objects. For baseline metadata requiring filesystem layout, materialize only the required repository inputs into a disposable directory and validate paths; never replace the user's checkout. Unavailable external path dependencies make that capability partial.

Use `cargo metadata --format-version 1 --offline --locked --filter-platform <target>` with explicit feature inputs when resolution is available. A no-dependencies metadata pass can help discovery, but cannot supply a resolved dependency graph. If Cargo fails, recover direct manifest and syntax facts; do not silently drop lockfile protection. Normalize local paths and opaque package identifiers into repository-scoped model IDs.

Capture stable rustc target cfg data and Cargo feature information. Evaluate known cfg predicates before building each graph. Unknown custom cfg or macro expansion introduces a capability gap in the affected scope. Build-dependency host configuration and production target configuration remain distinct. Declared visibility is not always effective external reachability; label it accordingly.

| Available without compilation | Requires stronger evidence or is deferred |
| --- | --- |
| Items, declared visibility, syntax metrics, module hierarchy | Complete effective public API and type/trait inference |
| Unambiguous explicit local paths/imports | Method dispatch, glob ambiguity, macro-generated edges, full call graph |
| Resolved Cargo dependencies/features where locally available | Invented resolution when packages or lockfile data are missing |
| Dependency reach and potential rebuild exposure | Actual compile duration, monomorphization, code size |
| Clone/allocation-like syntax candidates | Runtime allocation cost, execution frequency, bottlenecks |

**Reason:** stable/offline analysis and broken-project support are compatible with syntax and partial resolution. They do not justify pretending to have a compiler's semantic model. Source names alone are insufficient for performance claims.

## 3. Compact facts and bounded execution

Parse each distinct source content once, extract immutable facts, then release the AST and source buffer. Cache raw facts separately from cfg-filtered/linked facts. Use indexed vectors, interned repeated names, typed adjacency lists, and reverse edges. Store one fact and reference it from multiple findings rather than copying evidence text.

Compute cycles with a linear strongly-connected-component pass. Reuse summaries for dependency reach; avoid storing all-pairs reachability or an all-files co-change matrix. Large reach calculations are requested only for relevant subjects, with work accounting. A cold check still needs sufficient repository-wide facts for graph correctness and baseline populations: changed-file-only scanning is not a valid shortcut.

Start serial. Add a bounded parser worker pool only if profiling shows a benefit; cap workers at `min(available_parallelism, 4)` and in-flight file bytes. Never parallelize Cargo invocations. Keep at most one Cargo and one Git subprocess active, and do not overlap optional enrichment with the core memory peak. Sort merged facts before evaluation. Output must be invariant at worker counts 1 and 4.

Initial built-in limits below are proposed engineering defaults, not measured optimal values. They require no user tuning and change only with a tool release.

| Work | Initial bound | Exhaustion behavior |
| --- | --- | --- |
| One source file | 8 MiB | Mark file unsupported; dependent gates inconclusive |
| Aggregate source input per snapshot | 512 MiB | Stop acquisition with explicit partial coverage |
| Optional history | 2,000 recent reachable commits; 100,000 changed-path records | Stop at deterministic boundary; mark history truncated |
| One history commit | 200 changed paths for co-change | Exclude broad commit from co-change; report exclusion |
| Git rename fallback | Exact matches plus bounded native similarity detection (limit 1,000) | Unmatched movement remains explicit |
| One evidence import | 16 MiB | Reject oversized import with clear diagnostic |
| Fact cache | 256 MiB per repository | Evict disposable entries; recompute as needed |

Use bounded parsers, streaming process output, and explicit depth/record/work counters for every potentially expanding operation. Unsupported nesting and graph-work limits must affect capability status, not silently truncate evidence. Set subprocess deadlines and cancel/reap child processes on timeout or interruption; runtime deadlines yield operational incompleteness, never a different complete conclusion. Diagnostic elapsed time is noncanonical.

**Reason:** more CPU threads and more cached data are not automatically faster on small machines. Work limits protect resource usage while explicit coverage prevents misleading success.

## 4. Baseline and incremental reuse

Resolve one merge base using the target-selection contract in the spec. Analyze baseline and head under identical rules and concrete configurations. Reuse facts by content; do not reuse an older tool's finding verdicts.

Use native Git with argument arrays, NUL-delimited paths, explicit object IDs, no external diff/textconv, and bounded rename detection. Prefer a small number of batched requests: inventory/diff once, batch object reads through `git cat-file`, and bounded history enrichment once per snapshot. Never launch Git per function or per finding.

Record the Git version and effective diff/history options because rename correspondence can depend on them. Normalize supported locale-sensitive output and explicitly capture relevant Cargo/rustc configuration; never allow unrecorded machine defaults to affect a claimed identical-input run.

Build a bounded recent commit list before inspecting paths so targeting a rarely changed file cannot cause traversal of the entire history. Within that sample, retain complete changed-path sets for eligible commits that touch candidate paths. Exclude merge commits from co-change in v1, record that convention, and use the same deterministic ordering/tie-breaks on every run. Report support counts and the sampled denominator; do not extrapolate repository-wide probabilities. Churn and co-change from the same events are one history family.

Invalidation rules:

| Change | Recompute |
| --- | --- |
| Source bytes | That file's raw facts, affected module links, dependent rule facts |
| Module/import/public declaration | Containing crate resolution and affected reverse-dependency closure |
| Manifest, lockfile, feature, target, toolchain, cfg input | Relevant workspace resolution and configuration-dependent facts |
| Cohort membership/metrics | Population summary and every dependent outlier decision |
| Rule semantics/tool version | Derived findings for both sides |
| Imported evidence/history availability | Advisory enrichment only |
| Acceptance file | Acceptance application and gate verdict only |

For v1, cache per-file extraction plus complete immutable baseline fact snapshots. Rebuild linking, graphs, cohorts, and rule evaluation from compact facts unless measurements justify finer invalidation. When invalidation is uncertain, recompute the containing crate or workspace. This simpler strategy is the initial implementation, not an elaborate dependency-tracking engine.

Cache keys include content digests, parser/schema/tool versions, and every relevant semantic input; linked caches additionally include manifest/lockfile, toolchain, features, target, cfg, and dependency availability. A changed offline package cache must trigger capability re-evaluation. Never reuse failed metadata solely because source bytes are unchanged.

Write cache entries atomically with schema, length, and digest validation. Corrupt/missing entries become misses; read-only cache locations disable caching without changing results. Concurrent writers use unique temporary files and safe replacement. Eviction order cannot affect findings. Keep raw imported artifacts and source archives out of the cache.

**Reason:** native Git already stores content-addressed history. Compact disposable facts supply most reuse benefits without maintaining a second persistent knowledge database or risking stale decisions.

## 5. Concrete gate semantics and initial rule

Observation, concern, and gate eligibility are separate. A proven cycle is useful evidence but is not intrinsically forbidden. No proven-regression gate is enabled merely to populate that evidence class.

Start with one transparent structural quorum rule, `structure.coupled_complexity_growth`. This is a conservative proposal requiring fixture and real-project validation before release; its constants are not claimed to be empirically calibrated.

Subject: a production module in one concrete configuration. Required capabilities: complete parse/cfg selection, unambiguous subject correspondence or genuinely new subject, and complete explicit workspace-import resolution for that module, on each applicable side. Unsupported macro contents or ambiguous imports in the subject make this rule inconclusive there. It evaluates explicit imports, not the complete semantic dependency graph.

Metrics:

- `B`: source decision sites summed across the module's own production function/method/closure bodies, excluding child modules. Count each `if`, `for`, `while`, `loop`, non-wildcard match arm, match guard, `&&`, and `||` once by AST identity. This is a syntax metric, not a claim about executed branches or cyclomatic complexity.
- `D`: distinct other repository-owned modules targeted by resolved explicit imports in that module, excluding self, child-module declarations, and third-party modules. Multiple imports of one module count once. Unused imports are still structural declarations, not runtime calls.

Reference population: baseline production modules in the same crate and configuration with complete comparable facts; at least 20 subjects. Sort each integer metric and use nearest-rank p90, index `ceil(0.90 * n) - 1`. Freeze this population for head comparison. Missing/small populations make the percentile component not applicable, with descriptive advisory output only. Incomplete required observations in an otherwise eligible population are inconclusive, not a reason to shrink the cohort until it passes.

Distinguish not applicable from missing evidence. A fully observed small crate can complete its gate evaluation without this rule applying; show the zero-applicable-rule count prominently. A pass certifies only the enabled applicable checks, never the absence of all architectural risk.

Report a gate regression only when all conditions hold:

1. `B_head > p90(B_baseline)` and `D_head > p90(D_baseline)`.
2. `B_head - B_base >= max(3, ceil(B_base / 4))`.
3. `D_head - D_base >= max(2, ceil(D_base / 4))`.
4. The observations represent two distinct evidence families: decision-site growth and explicit dependency-surface growth. Neither percentile is an extra vote.
5. The finding is not covered by a valid material-evidence acceptance.

For a genuinely new module, use zero previous metrics but the existing crate's baseline population; for an unmatched potential move/split, mark attribution unknown and do not treat it as proven new. A new crate has no eligible baseline cohort in v1 and receives advisory findings. This is an explicit limitation, not a claim of regression-free architecture. Detect pure renames before applying the rule. Fully unmatched attribution that could affect this gate yields inconclusive, with candidate findings for investigation.

Example: an existing module goes from `B=12,D=4` to `B=18,D=7`, above baseline p90 values `15,5`. Both material predicates pass and the rule reports a strong regression. Growth from `12,4` to `13,7` does not pass the quorum. Moving the unchanged `12,4` module does not pass either. Changing only the reference population cannot create material growth.

For an existing finding, emit `worsened` only if the same predicates hold against its baseline metrics. Suppression never deletes evidence. Present the exact counts, thresholds, source spans, population size, and limitations alongside the recommendation to inspect responsibility/dependency growth. Keep other v1 rules advisory until they have equally explicit contracts and demonstrated useful precision.

**Reason:** the original phrases “quorum” and “material” permit inconsistent implementations. One inspectable rule establishes the contract without pretending every structural observation justifies blocking development. The deliberate trade-off is lower recall, especially for tiny/new crates, in exchange for lower gate noise.

## 6. Identity, movement, and material evidence

Maintain three separate identities:

1. Entity identity: crate/target/module path, item kind, qualified name, and deterministic disambiguator. Locations are evidence, not identity.
2. Movement correspondence: unique structural matches between baseline and head, assisted by Git file movement. Ignore trivia and moved container paths; require compatible item kind/signature/body structure and uniqueness. No all-pairs fuzzy AST comparison. Splits and rewrites may remain unknown.
3. Finding fingerprint: rule semantic revision bundled in the tool, configuration, stable matched entity, and material core evidence. Exclude line offsets, optional history, presentation, and timestamps.

Rule revisions are internal fingerprint inputs, not a separately pinned user ruleset. Formatting and unambiguous movement preserve acceptances. Changed metrics or rule meaning invalidate them. Ambiguity must not transfer an acceptance to an unrelated subject. Deterministic identity is not a promise to recognize arbitrary rewrites.

**Reason:** using one hash for identity, evidence, and acceptance either loses move attribution or suppresses real regressions. Separating them keeps the user-facing acceptance file small and tool-managed.

## 7. Output and optional imports

Finalize a versioned result containing input provenance, capability coverage, facts referenced by findings, comparisons, acceptances, and gate verdict. Serialize JSON and HTML from it, with deterministic escaping, key/array ordering, and integer/rational metrics. Keep process diagnostics separate. Use temporary files and atomic replacement for outputs; embed one result digest in each artifact so mixed generations can be detected after an interrupted multi-file write.

The HTML report opens with the gate verdict, changed findings, and coverage. Follow with crate/module disclosure, linked findings, compact excerpts, and dependency tables. Use native anchors and `details`; no JavaScript is required for v1. All findings remain accessible, but raw sources, every graph edge, and duplicated evidence need not become DOM elements. Canonical JSON carries the complete modeled result.

Accept one normalized evidence envelope with version, producer, source/configuration identity, units, subject identifiers, and observations. Imported runtime samples remain measurements of their stated workload; they are not necessarily deterministic performance truths. Import parsing and provenance attachment are separate from core analysis. Vendor converters, interactive graph layouts, and feature-matrix orchestration can be added later only for demonstrated needs.

**Reason:** one static document and one stable exchange format deliver the desired output without a browser application or a permanent adapter maintenance burden.

## 8. Validation and performance acceptance

Implement behavior fixtures before enabling CI gates. The repository currently contains documentation only, so none of these runtime checks are claimed to have passed.

| Property | Required verification |
| --- | --- |
| Baseline correctness | Diverged target, shallow history, missing/ambiguous base, dirty tree, deletions, and unchanged pre-existing debt |
| Attribution | Formatting, file/module move, duplicate bodies, split/merge, ambiguous rewrite, acceptance invalidation |
| Gate precision | Above/below quorum boundaries, zero growth, cohort-only change, tiny/new crate, correlated duplicate signals |
| Semantic honesty | Unknown cfg, platform-exclusive modules, macros, glob ambiguity, missing dependencies, stale imports |
| Mode parity | Same gate findings/verdict under check/analyze; adding optional history/imports cannot flip the gate |
| Cache correctness | Cold/warm/disabled/corrupt cache produce identical canonical bytes; dependency availability changes invalidate coverage |
| Determinism | Repeated runs, reordered traversal, 1/4 workers, different checkout directories, fixed equivalent configuration on Linux/macOS |
| Resource failures | Oversized/nested input, bounded history, timeout, cancellation, read-only cache, interrupted output, child cleanup |
| Reporting | Escaped hostile paths/snippets, no external requests, JavaScript-disabled navigation, every finding reachable |

Measure release builds on a recorded 4-core, 8-GiB Linux runner with local SSD; repeat functional checks on macOS. Use a reproducible 100k-line/20-crate fixture with fixed history and offline dependency availability, plus 10x-source/history stress fixtures. Record toolchain, hardware, warm/cold state, subprocess time, wall time, peak process-tree memory, cache size, report size, and browser DOM/open cost. Keep cache-diagnostic timing outside canonical artifacts.

Initial performance targets: cold `analyze` within 10 seconds and 256 MiB peak process-tree RSS; warm `check` after a small local edit within 2 seconds and 128 MiB; HTML within 10 MiB on the standard fixture. These are engineering targets awaiting measurement, not user promises. Do not drop findings to meet report size. Investigate targets missed on the recorded runner; do not make noisy wall-clock thresholds correctness gates on arbitrary CI hosts.

Before optimizing, profile acquisition, parsing, linking, rules, history, and rendering separately. Demonstrate near-linear source extraction/graph storage as input grows, and that optional history work remains capped as repository history grows. Add fine-grained incremental graphs, parallelism, compression, or alternate data structures only when a measured bottleneck justifies their maintenance cost.

Use Jeko and Kronicle as private real-project validation inputs when available, without assuming their current structure or committing their source into this repository. Review every proposed gate finding for useful precision before enabling the initial rule in a release. If it is too noisy, keep it advisory and revise the documented predicate; do not ask users to compensate with thresholds.

## 9. Decision summary and deferred scope

| Decision | Benefit | Cost / revisit trigger |
| --- | --- | --- |
| One process and one package | Simple deployment and low idle footprint | Split only for demonstrated reuse/build isolation |
| Syntax plus explicit resolution | Stable/offline, handles broken projects | Less semantic coverage; disclose uncertainty |
| Existing crate/module boundaries | Explainable architecture map | No automatic responsibility discovery |
| Core-only v1 gate | Fast/full parity and predictable offline verdict | History/import corroboration remains advisory |
| One conservative quorum rule first | Reviewable materiality and low tuning burden | Limited recall until more rules are validated |
| Cached extraction, rebuilt compact graphs | Straightforward invalidation | Some repeated graph work, optimize only if costly |
| Bounded native Git and unique movement matches | Controlled history/rename cost | Deep history and complex rewrites can be incomplete |
| One normalized import schema | Minimal adapter/dependency surface | External producers need a conversion step |
| Native HTML hierarchy | Portable report with no frontend runtime | No live graph layout or advanced client filtering |

## 10. Primary implementation references

These establish supported tool behavior, not Ferric Lens performance claims or validation of the proposed thresholds.

- [Cargo metadata](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html): format version, offline/locked modes, no-dependencies limitations, platform filtering, and opaque package identifiers.
- [Git diff](https://git-scm.com/docs/git-diff): NUL-delimited output, rename detection limits, and disabling external diff/text conversion.
- [Git cat-file](https://git-scm.com/docs/git-cat-file): batched object access without repeated process startup.

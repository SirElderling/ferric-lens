# Human testing Ferric Lens V1

This phase validates usefulness, precision, and report usability on real Rust repositories. It is not a substitute for automated correctness tests, which remain enforced by CI.

## 1. Get a release binary

Every pull-request CI run publishes one release artifact per runner:

- `ferric-lens-Linux-X64`
- `ferric-lens-macOS-<runner architecture>`

Download the artifact matching the machine used for testing and make the executable runnable if needed:

```bash
chmod +x ferric-lens
./ferric-lens --version
```

A locally built release binary is equivalent:

```bash
cargo build --release --locked
```

## 2. Run a full analysis

From any directory, analyze the target repository against its intended PR baseline:

```bash
/path/to/ferric-lens analyze /path/to/project \
  --base origin/main \
  --json /tmp/ferric-lens.json \
  --html /tmp/ferric-lens.html
```

Use the repository's actual target branch if it is not `origin/main`. Do not change thresholds or project configuration to make the output look better.

Open the generated HTML report in a normal browser. Keep the JSON beside it when exact evidence needs inspection.

## 3. First validation projects

Use Jeko and Kronicle as the initial private real-project inputs. Their source or reports do not need to be committed to this repository.

For each project, review every emitted finding in these sections:

1. Gate findings
2. Refactoring candidates
3. Structural advisories
4. Runtime-risk candidates
5. Build-efficiency candidates

Also identify important architecture/refactoring concerns already known to the human reviewer that Ferric Lens did not surface.

## 4. Review each finding

Record one row per finding using this scorecard:

| Field | Allowed answer |
| --- | --- |
| Finding fingerprint | Exact fingerprint from report |
| Project | Jeko / Kronicle / other |
| Correct evidence? | yes / no / uncertain |
| Useful to investigate? | yes / no |
| Priority appropriate? | yes / too high / too low |
| Direction useful? | yes / no / partly |
| Noise classification | none / false positive / technically true but irrelevant |
| Understandable within ~30 seconds? | yes / no |
| Notes | Plain-English reason |

For a refactoring candidate, explicitly check that the independent signals really support the same concern. The suggested direction should help investigation without pretending Ferric Lens knows the final architecture.

## 5. Check for missed issues

Before changing Ferric Lens, list known meaningful issues in the project that were not reported. Classify each as:

- expected V1 limitation,
- evidence Ferric Lens already owns but failed to combine,
- evidence Ferric Lens does not yet collect.

This distinguishes low recall from an actual implementation defect.

## 6. Gate-specific acceptance

The initial blocking rule is `structure.coupled_complexity_growth`.

For every gate finding, verify manually that:

- the module actually changed,
- both decision complexity and dependency surface materially worsened,
- the baseline comparison is the intended merge base,
- the finding would be useful enough to block a real PR.

If any valid gate finding feels too noisy to block development, treat that as rule-calibration evidence. Do not add project-specific thresholds as a workaround.

## 7. Report usability

After reviewing a project, answer these separately from finding correctness:

- Could the first screen tell you what deserves attention?
- Were priority and evidence strength understandable?
- Could you navigate from a finding to the affected module quickly?
- Was the distinction between gate, refactor, structural, runtime-risk, and build findings clear?
- Was anything important buried or repeated?
- Did the report make any claim stronger than its evidence justified?

## 8. Completion criteria for human testing

V1 can move from human testing toward release acceptance when:

- no deterministic evidence is materially wrong,
- the blocking rule has acceptable precision on the reviewed real projects,
- refactor candidates are usually useful rather than merely technically true,
- report navigation and terminology are understandable without reading the implementation,
- known missed issues are either deliberate V1 limits or have an explicit follow-up decision.

After that, run the documented 10x source/history stress fixtures, browser DOM/open measurement, and recorded release-environment performance acceptance.

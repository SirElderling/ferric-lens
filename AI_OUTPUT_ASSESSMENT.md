# AI Output Assessment

Status: **assessment only — no implementation is authorized by this document**

Date: 2026-09-20

## Purpose

This document assesses Ferric Lens's current `--ai` output specifically as input to an AI coding or review model.

The question is not whether the output is pleasant for a human to read. The question is:

> Does Ferric Lens give an AI model a small, trustworthy set of deterministic facts that helps it decide where to inspect next without encouraging speculative changes?

The current output is useful, but it can be made materially more effective for AI consumption. The main opportunity is not adding more analysis. It is improving **relevance ordering, evidence framing, boundedness, and the separation between observed fact and Ferric Lens interpretation**.

## Current state

The current `ferric_lens_ai` schema already has several strong properties:

- it is deterministic JSON;
- it does not include the full raw repository/module/function/type inventory;
- it separates actionable findings from Observe-level observations;
- it includes priority, evidence strength, delta state, gate status, rule identity, evidence, and source contexts;
- it exposes incomplete/unavailable analysis capabilities;
- it includes exact source excerpts when Ferric Lens has truthful source-local evidence;
- it does not change canonical analysis semantics or gate behavior.

Those choices make the output meaningfully more useful than giving an AI the canonical JSON directly.

The current shape is approximately:

```text
verdict
verdict_reason
summary
findings[]
observations[]
analysis_limits[]
```

Each AI finding currently contains:

```text
priority
evidence_strength
delta
gate
title
subject
path
why_care
next_step
rule
evidence[]
source_contexts[]
```

The weaknesses below are therefore refinements to an already useful interface, not a recommendation to replace it with free-form model-oriented prose.

---

## Assessment

### 1. Make change attribution the primary AI ordering

**Recommendation: highest priority**

The AI output should make these categories explicit and order them before ordinary priority:

1. introduced by the current change;
2. worsened by the current change;
3. existing actionable issue;
4. context-only observation.

Today Ferric Lens orders findings by priority first and delta second. That is reasonable for a general report, but it is not the best default for an AI reviewing a branch or pull request.

### Why this matters

A coding model reviewing a change needs to distinguish:

- something the proposed change caused;
- something the proposed change materially worsened;
- pre-existing technical debt that happens to be nearby.

Without that distinction dominating the presentation, an AI can spend tokens and implementation effort fixing unrelated existing code.

This is particularly important because AI coding agents are action-biased: when given a plausible warning, they often try to remediate it even when the warning is not relevant to the requested change.

### Proposed direction

Represent change relevance as a first-class grouping or machine-readable category, for example:

```json
{
  "change_findings": [],
  "existing_findings": [],
  "context": {}
}
```

or retain a single list but add an explicit field such as:

```json
"change_relevance": "introduced"
```

The exact schema should be decided during implementation. The important contract is that branch-local regressions are impossible to confuse with existing debt.

### Tradeoff

This makes same-head/no-baseline analysis slightly less uniform because there may be no meaningful change grouping. That is acceptable. In that case Ferric Lens should explicitly say change attribution is unavailable rather than pretending all findings have equal review relevance.

---

## 2. Separate deterministic facts from Ferric Lens interpretation

**Recommendation: highest priority**

The current AI record combines raw evidence with fields such as `title`, `why_care`, and `next_step`. These are useful, but they do not make the boundary between observation and inference explicit enough.

### Why this matters

For AI consumption, this distinction is critical.

Consider:

```text
Observed:
- World is cloned.
- selected.events is subsequently retained.

Interpretation:
- The whole aggregate copy may be unnecessary.
```

The first two statements are source facts. The third is an inference.

If all three are serialized as one finding narrative, the consuming model may treat the interpretation as proven. This is especially risky for static runtime-risk signals where Ferric Lens deliberately does not know ownership constraints, object size, path frequency, or measured runtime cost.

### Proposed direction

AI findings should explicitly separate:

```json
{
  "facts": [],
  "interpretation": "...",
  "limitations": []
}
```

`facts` should contain only claims Ferric Lens can directly support from its deterministic analysis.

`interpretation` should explain why those facts may be worth inspection.

`limitations` should state what the evidence does not establish.

### Tradeoff

This requires every AI-facing rule to define its factual claim carefully. That is extra schema and test work, but it is aligned with Ferric Lens's core product principle: explicit uncertainty is preferable to false confidence.

---

## 3. Replace prescriptive fixes with bounded inspection questions

**Recommendation: high priority**

The AI output should tell the model what to verify next, not what code transformation to perform.

The current `next_step` is backed by `Finding.direction`. This is generally conservative already, but the AI contract should make the distinction explicit.

### Why this matters

Ferric Lens has repository-wide deterministic context. It does not have complete semantic knowledge of:

- ownership requirements;
- performance characteristics;
- architectural intent;
- API compatibility constraints;
- hidden runtime invariants.

An AI model can inspect those details once Ferric Lens points it at the right code.

The safest division of responsibility is:

> Ferric Lens identifies evidence and poses a focused inspection question. The AI decides whether and how the code should change.

### Proposed direction

Prefer a field such as:

```json
"recommended_inspection": [
  "Determine whether ownership of World is required after this call.",
  "Check whether the filtered event view can be built without copying the entire aggregate."
]
```

Avoid fields that imply a deterministic fix such as:

```json
"fix": "replace clone with references"
```

### Tradeoff

Inspection questions are slightly more verbose than terse directions, but they reduce the chance of an AI mechanically applying an unsafe rewrite.

---

## 4. Bound the number of findings and observations sent to the model

**Recommendation: high priority**

The current `--ai` output serializes all active findings and all observations.

That should change.

### Why this matters

AI context is not improved by unlimited low-confidence evidence.

Every additional observation has three costs:

1. token cost;
2. attention dilution;
3. increased probability that the model rationalizes a weak signal into unnecessary work.

The AI output should optimize for **precision per token**, not completeness. Completeness belongs in canonical JSON.

### Proposed direction

Use deterministic caps, for example:

- up to 5 actionable findings;
- up to 5 observations;
- up to 3 source contexts per finding;
- counts of omitted records;
- deterministic ordering.

Example:

```json
"context_summary": {
  "observations_included": 5,
  "observations_omitted": 27
}
```

The exact limits should be measured against real repositories before becoming contract.

### Tradeoff

A bounded AI view can omit a finding an agent might have found useful. That is acceptable if the canonical JSON remains complete and the AI output clearly reports omission counts.

The desired contract is not “all information.” It is “the most decision-relevant information.”

---

## 5. Prefer source-local evidence over statistical explanation

**Recommendation: high priority**

When exact source evidence exists, the AI output should prioritize the source window before percentile/reference context.

### Why this matters

An AI model can reason directly about code.

For example, this:

```json
{
  "path": "src/render/history.rs",
  "start_line": 155,
  "end_line": 160,
  "focus_line": 155,
  "excerpt": "..."
}
```

is usually more useful than first explaining that a metric value is above a population reference.

Population statistics are valuable for explaining why Ferric Lens selected the area, but they should be supporting metadata rather than the center of the AI record.

### Proposed direction

Each AI finding should lead with one or more bounded source locations when available.

Statistical evidence should remain available under a secondary field such as:

```json
"selection_evidence": [...]
```

If no truthful source-local evidence exists, Ferric Lens should retain module/file-level navigation and say so explicitly.

### Tradeoff

This does not reduce the importance of the statistical rule. It only changes presentation priority for the consumer.

---

## 6. Add finding-specific limitations, not only analysis-wide capability limits

**Recommendation: high priority**

The current output exposes global `analysis_limits`, which is good. It should additionally expose local limits on individual findings.

### Why this matters

A global capability limitation such as unresolved cfg is different from a finding-specific caveat such as:

- clone cost was not measured;
- receiver type was not resolved;
- reverse dependency reach is not measured compile time;
- structural concentration is not proof of poor design.

AI models benefit from these constraints directly beside the finding they qualify.

### Proposed direction

Example:

```json
{
  "limitations": [
    "Runtime cost was not measured.",
    "The detector does not establish that borrowing is valid here."
  ]
}
```

These should be deterministic rule metadata, not dynamically generated prose.

### Tradeoff

Some limitation text will repeat across findings from the same rule. That repetition is worthwhile because it prevents a finding from being consumed without its uncertainty boundary.

---

## 7. De-emphasize human-oriented explanatory prose in `--ai`

**Recommendation: medium priority**

Fields such as `why_care` are valuable in HTML. They are less valuable in a compact AI channel when the same information can be represented as facts, interpretation, and inspection questions.

### Why this matters

Human reports need education and persuasion:

> Why should I care?

An AI model mainly needs:

> What was observed, how certain is it, where is it, and what should I inspect?

Long human guidance competes with source evidence for context.

### Proposed direction

Keep rich explanatory prose in HTML.

For `--ai`, prefer compact fields:

```text
facts
interpretation
recommended_inspection
limitations
```

A short plain-language title can remain useful for orientation.

### Tradeoff

Removing all natural-language guidance would make the output unnecessarily cryptic. The goal is compression, not an opaque metric protocol.

---

## 8. Keep rule identity, but make it secondary

**Recommendation: medium priority**

The current output includes internal rule identity, which should remain because it provides deterministic provenance and is useful for automation.

It should not be the primary semantic label shown to the AI.

### Why this matters

An AI should reason from evidence, not from a rule name that sounds authoritative.

`runtime.clone_then_mutate_candidate` is useful provenance. It should not substitute for the source facts that triggered it.

### Proposed direction

Keep:

```json
"rule": "runtime.clone_then_mutate_candidate"
```

but place it after the human-readable evidence fields or under a provenance object.

---

## 9. Preserve observations, but explicitly prevent them from competing with findings

**Recommendation: medium priority**

Observe-level records are useful context. They should remain intentionally subordinate.

### Why this matters

A weak architecture observation can help an AI understand why a nearby change is sensitive. The same observation becomes harmful if it appears equivalent to an actionable finding.

### Proposed direction

Observations should:

- be emitted after actionable records;
- be capped;
- carry explicit language that no change is currently justified;
- never strengthen an AI instruction on their own;
- be summarized when numerous.

Example:

```json
{
  "context": {
    "observations": [...],
    "additional_observations_omitted": 18
  }
}
```

---

## 10. Do not turn `--ai` into a second canonical schema

**Recommendation: architectural constraint**

The compact AI output should remain a derived presentation over the canonical `AnalysisResult`.

### Why this matters

Ferric Lens should have one analysis truth.

If the AI schema begins inventing analysis state not represented by canonical findings/evidence, the project risks semantic divergence between:

- JSON;
- HTML;
- CLI;
- AI output;
- gate behavior.

The AI layer may organize, label, cap, and explain canonical evidence. It should not silently introduce a second detector or scoring model.

---

# Proposed target shape

The following is illustrative, not an implementation contract:

```json
{
  "format": "ferric_lens_ai",
  "schema_version": 2,
  "result_digest": "...",
  "verdict": "PASS",
  "verdict_reason": "...",

  "change_findings": [
    {
      "subject": "demo::render",
      "path": "src/render/history.rs",
      "change_relevance": "introduced",
      "priority": "investigate",
      "evidence_strength": "strong",

      "facts": [
        "Repository dependency breadth increased from 4 to 17."
      ],

      "source": [
        {
          "path": "src/render/history.rs",
          "start_line": 40,
          "end_line": 67,
          "excerpt": "..."
        }
      ],

      "interpretation": "The change materially broadened this module's repository dependency surface.",

      "recommended_inspection": [
        "Check whether the new dependencies belong behind an existing boundary."
      ],

      "limitations": [
        "This is structural evidence and does not measure compile time."
      ],

      "rule": "..."
    }
  ],

  "existing_findings": [
    {
      "subject": "demo::history",
      "path": "src/render/history.rs",
      "change_relevance": "existing",
      "priority": "investigate",
      "evidence_strength": "candidate",

      "facts": [
        "A whole aggregate is cloned into a mutable local.",
        "A nested collection on that clone is subsequently filtered."
      ],

      "source": [
        {
          "path": "src/render/history.rs",
          "start_line": 155,
          "end_line": 160,
          "excerpt": "..."
        }
      ],

      "interpretation": "The whole aggregate copy may be larger than the owned subset required by this operation.",

      "recommended_inspection": [
        "Determine whether ownership of the entire aggregate is required.",
        "Check whether the filtered event set can be constructed without copying unrelated fields."
      ],

      "limitations": [
        "Runtime cost was not measured.",
        "The detector does not establish that borrowing is valid."
      ],

      "rule": "runtime.clone_then_mutate_candidate"
    }
  ],

  "context": {
    "observations": [],
    "additional_observations_omitted": 14
  },

  "analysis_limits": []
}
```

# Recommended implementation order

If this assessment is approved, implementation should proceed in this order:

1. **Define and test the AI-output contract before changing serialization.**
   Establish what counts as fact, interpretation, finding-specific limitation, and inspection guidance.

2. **Add explicit change-relevance grouping/ordering.**
   This delivers the largest practical gain for PR review.

3. **Introduce rule-owned facts, interpretation, limitations, and inspection metadata.**
   Avoid generating these heuristically at serialization time.

4. **Change source evidence presentation priority.**
   Preserve current bounded source extraction and reuse it.

5. **Add deterministic output caps and omission counts.**
   Measure candidate limits against Ferric Lens, Jeko, Kronicle, and at least one larger Rust repository before freezing values.

6. **Remove/reduce duplicate human-oriented `why_care` prose from AI output.**

7. **Version the AI schema.**
   This is a meaningful contract change and should not silently alter schema version 1.

# Acceptance criteria for a future implementation

A future AI-output change should not be considered complete merely because the JSON shape matches a new schema.

It should demonstrate all of the following:

- the same analysis result still drives HTML, canonical JSON, CLI, gate, and AI output;
- `--ai` remains deterministic;
- exact source evidence remains bounded;
- introduced/worsened findings are unmistakable from existing findings;
- factual claims are mechanically distinguishable from interpretation;
- every rule emitted to AI has explicit uncertainty/limitation text where the detector cannot prove impact;
- Observe-level output is bounded and cannot visually or structurally compete with actionable findings;
- omitted records are counted rather than silently disappearing;
- same-head/no-baseline analysis remains coherent;
- accepted findings remain excluded unless a future use case explicitly justifies including them;
- no AI-output field prescribes a code rewrite that Ferric Lens cannot deterministically justify;
- the compact output is materially smaller than canonical JSON on representative repositories;
- real-model evaluation shows that the new form reduces irrelevant remediation without hiding branch-local problems.

# Evaluation method

The most important validation should be behavioral, not just snapshot testing.

For each representative repository, give an AI model:

1. the repository/change plus the current `--ai` output;
2. the same repository/change plus the proposed output;
3. the same task with no Ferric Lens output.

Compare:

- whether the model inspects the genuinely relevant files first;
- whether it distinguishes branch regressions from existing debt;
- how often it proposes unnecessary code changes;
- how many source files it needs to inspect before reaching a useful conclusion;
- token consumption;
- whether it repeats Ferric Lens's heuristic interpretation as if it were proven;
- whether it correctly preserves intentional code after inspection.

The desired outcome is not “the model fixes more findings.”

The desired outcome is:

> The model reaches a correct, evidence-grounded decision with less search, less context, and fewer unnecessary changes.

# What should not change yet

This assessment does **not** recommend changing:

- canonical `AnalysisResult`;
- finding fingerprints;
- acceptance identity;
- gate thresholds;
- detector thresholds;
- HTML semantics;
- default CLI semantics;
- source-context extraction bounds;
- the principle that missing evidence must remain explicit.

Those areas should remain stable while AI serialization is improved.

# Conclusion

Ferric Lens's AI output is already useful because it converts repository-wide deterministic analysis into a smaller machine-readable view with source evidence and explicit analysis limits.

The largest remaining weakness is that it still behaves too much like a compact human report.

For AI consumption, Ferric Lens should optimize for a narrower contract:

> **Give the model the smallest trustworthy set of facts needed to decide where to inspect next, clearly distinguish those facts from interpretation, and make change relevance impossible to miss.**

That direction strengthens Ferric Lens's role as a deterministic evidence provider without asking it to become an autonomous code reviewer or a second AI reasoning system.

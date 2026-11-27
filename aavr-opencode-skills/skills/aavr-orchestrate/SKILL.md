---
name: aavr-orchestrate
description: Coordinate an evidence-driven AAVR security assessment of a C or C++ repository. Use when asked to audit, assess, review, fuzz, or investigate an unfamiliar native-code project end to end. Select the appropriate AAVR specialist skills, maintain workflow state under .aavr, enforce evidence and stopping requirements, and prevent unsupported vulnerability claims.
---

# AAVR Orchestrator

Coordinate the assessment. Do not duplicate specialist procedures when a focused AAVR skill exists.

## Operating principles

- Run deterministic discovery and validation before speculative reasoning.
- Treat scanner output as candidates, never as confirmed vulnerabilities.
- Preserve exact commands, tool versions, logs, and target revision.
- Keep generated artifacts beneath `.aavr/` unless the repository already defines a safer location.
- Do not modify production source during initial assessment.
- Do not push, publish, disclose, or delete repository content.
- Ask before using elevated privileges, changing the host, or starting an unbounded campaign.
- Stop when evidence is insufficient; record the missing evidence instead of guessing.

## Initialize state

Create this structure only when needed:

```text
.aavr/
├── state.json
├── baseline/
├── recon/
├── surfaces/
├── runs/
├── findings/
├── harnesses/
├── corpora/
├── crashes/
├── variants/
└── reports/
```

Record at minimum in `.aavr/state.json`:

```json
{
  "schema_version": 1,
  "target_revision": "",
  "scope": [],
  "completed_phases": [],
  "active_candidates": [],
  "blocked_on": [],
  "next_phase": "repository-recon"
}
```

Do not overwrite existing AAVR artifacts without preserving prior results.

## Route work

When the `srcwranglr` MCP tools are available, load `aavr-srcwranglr` first and use those tools in every phase in place of grep and whole-file reads.

Use this default sequence and skip a phase only with an explicit reason:

1. Load `aavr-repository-recon`.
2. Load `aavr-build-baseline`.
3. Load `aavr-attack-surface`.
4. Load `aavr-static-analysis`.
5. Load `aavr-semgrep-database` when a private Semgrep corpus is configured.
6. Load `aavr-finding-triage`.
7. Load `aavr-sanitizer-validation` for candidates requiring runtime proof.
8. Load `aavr-fuzz-target-selection` when fuzzing is justified.
9. Load `aavr-harness-generation` for the selected target.
10. Load `aavr-fuzz-campaign` for a bounded campaign.
11. Load `aavr-crash-triage` for every crash or hang.
12. Load `aavr-variant-analysis` after a root cause is understood.
13. Load `aavr-report` to produce the current assessment.

## Optional specialist passes

Insert these when the target warrants them; each produces candidates that feed
`aavr-finding-triage` like any other analysis step. Skip them when nothing triggers them.

- `aavr-differential-review` — when the assessment is scoped to a change (a diff, commit
  range, or PR) rather than the whole tree. It can lead the workflow: review the change,
  then drop into the default sequence for anything it surfaces.
- `aavr-insecure-defaults` — during or alongside static analysis, when the target has
  configuration, secrets, auth toggles, or startup code that could fail open.
- `aavr-constant-time-analysis` — during analysis, when recon found cryptographic or
  secret-handling code that could leak through timing.
- `aavr-property-based-testing` — alongside target selection and harness generation, for
  parsers, serializers, and normalizers where a roundtrip or invariant property strengthens
  a harness.

## Phase gates

Do not advance unless the gate is satisfied:

- Recon gate: identify the build path and at least one externally influenced input surface, or document why neither can be established.
- Baseline gate: establish whether the unmodified project builds and tests.
- Analysis gate: preserve raw tool output and normalize candidate findings.
- Triage gate: establish attacker control, reachability, constraints, and a validation plan.
- Harness gate: identify target API, initialization, input model, and reset strategy.
- Crash gate: reproduce and minimize before assessing security impact.
- Reporting gate: separate confirmed, likely, speculative, rejected, and blocked items.

## Progress response

After each phase, report:

```text
Completed:
Evidence produced:
Strongest current candidates:
Blocked or uncertain:
Recommended next phase:
```

Never claim a phase completed if the required commands or evidence were not produced.

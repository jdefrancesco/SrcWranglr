---
name: aavr-differential-review
description: Security-focused review of a code change (a diff, commit, commit range, or PR) in a C or C++ repository. Use when the assessment is scoped to what changed rather than the whole tree — to catch regressions, removed checks, and newly introduced unsafe code. Scales depth to codebase size and produces an evidence-backed report.
---

# AAVR Differential Review

Review a change for security impact, not the whole repository. Classify by risk, not by diff
size — a two-line change removed a bounds check in Heartbleed. Every finding is backed by the
diff, the surrounding code, git history, and a concrete attack scenario.

When SrcWranglr is available, its commit-range audit (History → Changes, the "Only changed
code" findings filter, and gutter marks) is the fastest way to see what changed and which
findings fall inside the range; load `aavr-srcwranglr` and use it.

## Scope the effort

| Codebase size     | Strategy  | Approach                                   |
| ----------------- | --------- | ------------------------------------------ |
| SMALL (<20 files) | DEEP      | read all touched deps, full `git blame`    |
| MEDIUM (20–200)   | FOCUSED   | one-hop deps, priority files               |
| LARGE (200+)      | SURGICAL  | critical paths only                        |

Rate each changed hunk HIGH / MEDIUM / LOW:

- **HIGH** — auth, crypto, memory management, parsing of untrusted input, bounds/length
  checks, integer arithmetic on sizes, a removed or weakened validation.
- **MEDIUM** — business logic, state changes, a new public API or entry point.
- **LOW** — comments, tests, logging, formatting.

Treat "just a refactor" as HIGH until proven otherwise: refactors silently break invariants.

## Workflow

Build baseline context first (load `aavr-repository-recon` for an unfamiliar tree), then:

1. **Triage** the diff — list changed files, assign risk, pick what to read deeply.
2. **Analyse** the HIGH/MEDIUM hunks in the context of the functions they sit in.
3. **Git history** — `git blame` and log on any security-relevant line that was changed or
   removed; a deleted check is a regression signal.
4. **Blast radius** — enumerate the transitive callers of a changed function; a local change
   can widen an interface or invalidate a caller's assumption.
5. **Test coverage** — missing tests on a changed security path raise the severity.
6. **Adversarial pass** on HIGH-risk changes — model the attacker and a concrete exploit.
7. **Report** — write a Markdown report under `.aavr/reports/` with per-finding file:line,
   the commit, the attack scenario, and an explicit statement of coverage and confidence.

Read the matching bundled guide for depth: `methodology.md` (phases and triage),
`adversarial.md` (attacker modelling and exploitability), `patterns.md` (regressions,
access control, overflow, and other recurring change-level bugs), `reporting.md` (report
structure).

## Discipline

- State what you did not review and your confidence; never imply full coverage you did not do.
- A removed check is a finding until you prove the invariant is enforced elsewhere.
- Keep findings concrete — real inputs and line numbers, not generic warnings.

## Handoff

Hand strong candidates to `aavr-finding-triage`, and once a root cause is understood load
`aavr-variant-analysis` to find the same mistake elsewhere in the tree.

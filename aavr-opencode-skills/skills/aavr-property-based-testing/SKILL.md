---
name: aavr-property-based-testing
description: Guidance for property-based testing of C or C++ code — roundtrip, idempotence, and invariant properties over generated inputs, for parsers, serializers, normalizers, and pure algorithms. Use when example tests miss edge cases, or to turn a property into a fuzz harness. Complements aavr-harness-generation and aavr-fuzz-campaign.
---

# AAVR Property-Based Testing

Use properties over generated inputs where they give stronger coverage than example tests.
In an AAVR assessment this serves two ends: exercising a function hard enough to surface a
bug, and expressing an invariant that a fuzz harness can then drive at scale.

## When it pays off

Reach for a property when you see:

- **Serialization pairs** — `encode`/`decode`, `serialize`/`deserialize`, `pack`/`unpack`.
- **Parsers** — protocol, config, URL, or string-to-struct code (prime AAVR targets).
- **Normalizers** — `normalize`, `sanitize`, `canonicalize`, `format`.
- **Validators**, especially paired with a normalizer.
- **Pure algorithms** — sorting, comparison, arithmetic, custom containers.

| Pattern            | Property              | Priority |
| ------------------ | --------------------- | -------- |
| encode/decode pair | roundtrip             | HIGH     |
| pure function      | several               | HIGH     |
| validator          | valid after normalize | MEDIUM   |
| sort / order       | idempotence + order   | MEDIUM   |
| normalization      | idempotence           | MEDIUM   |

Skip simple CRUD, UI, throwaway scripts, and code whose side effects can't be isolated.

## Property catalog

| Property     | Formula                   | Use for                         |
| ------------ | ------------------------- | ------------------------------- |
| Roundtrip    | `decode(encode(x)) == x`  | serialization, conversion pairs |
| Inverse      | `f(g(x)) == x`            | encrypt/decrypt, compress       |
| Idempotence  | `f(f(x)) == f(x)`         | normalization, formatting, sort |
| Invariant    | holds before/after        | any transformation              |
| Oracle       | `fast(x) == reference(x)` | optimisation, refactor          |
| No crash     | no UB/abort on valid input| baseline for any parser         |

Strength, weakest to strongest: no-crash → type preservation → invariant → idempotence →
roundtrip.

## In a C/C++ assessment

- Prefer a C/C++ PBT library (RapidCheck, or proptest/quickcheck for Rust components). The
  `references/libraries.md` guide lists tools per language.
- The **no-crash** and **roundtrip** properties map directly onto a libFuzzer/AFL++ harness:
  the property body becomes the harness body, and the generator becomes structure-aware input.
  When a property is worth fuzzing, hand it to `aavr-harness-generation` and run it under
  `aavr-fuzz-campaign` with ASan/UBSan.
- Build every property under sanitizers so a violated invariant and a memory error surface
  together.

Read the task-specific guide as needed: `references/generating.md` (writing properties and
generators), `references/strategies.md` (complex input generation), `references/design.md`
(property-driven design), `references/refactoring.md` (making stubborn code testable),
`references/reviewing.md` (quality checklist), `references/interpreting-failures.md`
(reading a shrunk counterexample and classifying the bug).

## Discipline

- Offer PBT, don't force it; if the user wants example tests, write good ones instead.
- Limit suggestions to the highest-value candidates; don't propose properties for trivial
  getters.
- A shrunk failing case is a bug report: capture the minimal input and route it to
  `aavr-crash-triage` if it reproduces a memory error.

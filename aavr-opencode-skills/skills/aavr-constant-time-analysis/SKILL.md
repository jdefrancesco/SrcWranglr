---
name: aavr-constant-time-analysis
description: Detect timing side-channel vulnerabilities in cryptographic C or C++ code (and other languages the analyzer supports). Use when the target handles secret keys, tokens, or signatures, when division or branches depend on a secret, or when a finding concerns constant-time behaviour. Complements aavr-static-analysis and aavr-finding-triage.
---

# AAVR Constant-Time Analysis

Find operations whose execution time depends on secret data, which can leak keys or tokens
to an attacker who can measure timing. This is a focused pass, not a whole-repository scan:
run it on the crypto and secret-handling code that `aavr-repository-recon` and
`aavr-attack-surface` identified.

## When to run

- A target implements signing, verification, encryption, decryption, key derivation, MAC
  comparison, or padding checks.
- Secret-derived values flow into `/` or `%`, into a branch condition, into an array index,
  or into an early `return` / `break`.
- A candidate from another pass mentions "constant-time", "timing", "side-channel", or a
  named issue such as KyberSlash.

Skip non-cryptographic code, public-data processing, and high-level API use where a vetted
library already handles timing.

## Run the analyzer

The skill ships a disassembly/bytecode analyzer under `ct_analyzer/`. Paths below use
`{baseDir}` for this skill's own directory; if your runner does not substitute it, use the
installed path (for a global OpenCode install, `~/.config/opencode/skills/aavr-constant-time-analysis`).
It needs `uv`.

```bash
# C / C++ source (also Go, Rust, Swift): assembly-level analysis
uv run {baseDir}/ct_analyzer/analyzer.py crypto.c
uv run {baseDir}/ct_analyzer/analyzer.py --warnings crypto.c      # include branch warnings
uv run {baseDir}/ct_analyzer/analyzer.py --func 'sign|verify' crypto.c
uv run {baseDir}/ct_analyzer/analyzer.py --json crypto.c          # machine-readable, for CI

# Native languages: vary architecture and optimisation — a leak can appear only at some
uv run {baseDir}/ct_analyzer/analyzer.py --arch x86_64 crypto.c
uv run {baseDir}/ct_analyzer/analyzer.py --arch arm64  crypto.c
uv run {baseDir}/ct_analyzer/analyzer.py --opt-level O0 crypto.c
uv run {baseDir}/ct_analyzer/analyzer.py --opt-level O3 crypto.c
```

The analyzer inspects compiled output (native assembly, or JVM/CIL/Swift bytecode for the VM
languages), so a secret-dependent `idiv`, conditional move that is actually a branch, or
variable-latency instruction shows up even when the source looks constant-time. Match a
language to its reference guide before interpreting results:

| Language        | Guide                         |
| --------------- | ----------------------------- |
| C, C++, Go, Rust| `references/compiled.md`      |
| Swift           | `references/swift.md`         |
| Java, C#        | `references/vm-compiled.md`   |
| Kotlin          | `references/kotlin.md`        |
| PHP             | `references/php.md`           |
| JavaScript / TS | `references/javascript.md`    |
| Python          | `references/python.md`        |
| Ruby            | `references/ruby.md`          |

## Discipline

- A flagged instruction is a candidate, not a confirmed leak. Confirm the operand is actually
  secret-derived (trace it) and the code is reachable before raising severity.
- Separate the root cause (e.g. table lookup indexed by a secret) from the observable effect.
- Prefer the fixes the language guide documents (masked select, constant-time compare,
  Barrett/Montgomery reduction) over ad-hoc branch removal.

## Handoff

Record confirmed leaks as candidates and load `aavr-finding-triage` for attacker-control and
reachability, and `aavr-sanitizer-validation` or a timing harness when runtime proof is
wanted. Keep raw analyzer output under `.aavr/runs/<run-id>/`.

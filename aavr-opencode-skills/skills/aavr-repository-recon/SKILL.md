---
name: aavr-repository-recon
description: Inspect an unfamiliar C or C++ repository and produce structured security reconnaissance. Use before building, scanning, editing, or fuzzing to identify languages, build systems, targets, tests, dependencies, parsers, privilege boundaries, externally influenced inputs, existing fuzzers, and security-relevant components.
---

# AAVR Repository Reconnaissance

Inspect before changing or building the repository.

## Establish identity

Record:

- absolute repository root
- current commit and dirty state
- submodules and vendored dependencies
- primary languages and generated code
- target platforms and architectures
- build systems and package managers
- CI workflows that reveal canonical commands

Prefer repository-native evidence such as `README`, contribution guides, CI files, build manifests, and test scripts.

## Inventory security-relevant structure

Locate:

- executable and library entry points
- network listeners and protocol dispatch
- file, archive, image, audio, and document parsing
- IPC, RPC, plugin, device, and kernel interfaces
- authentication and authorization boundaries
- command execution and dynamic loading
- memory ownership boundaries and custom allocators
- length-prefixed buffers, integer-controlled allocations, and state machines
- existing tests, fuzzers, sanitizer jobs, and security tooling

Use targeted searches rather than dumping the entire tree into context.

## Do not over-interpret names

Verify candidate surfaces through callers and data flow. A function named `parse` may consume trusted configuration; a generic helper may receive remote input.

## Build deep context on the hot components

For the few components that recon marks as the real attack surface, go deeper than a summary
before any bug hunting: read the key functions closely, line by line, and record the data and
control flow that actually matters — who owns each buffer, where a length comes from, which
checks constrain an index, and what state a parser carries between calls. This architectural
context is what later phases (triage, harness generation, differential review) build on; a
shallow recon produces shallow findings. Capture it in `summary.md` with precise file:line
references rather than prose.

## Output

Write `.aavr/recon/repository.json`:

```json
{
  "target_revision": "",
  "dirty": false,
  "languages": [],
  "build_systems": [],
  "canonical_build_evidence": [],
  "targets": [],
  "tests": [],
  "dependencies": [],
  "entry_points": [],
  "input_surfaces": [],
  "parsers": [],
  "privilege_boundaries": [],
  "existing_fuzzers": [],
  "existing_security_tooling": [],
  "candidate_components": [],
  "unknowns": []
}
```

Also write `.aavr/recon/summary.md` with concise evidence links to files and line ranges.

## Stop condition

Stop and report blockers when the repository root, likely build path, or scope cannot be determined. Do not invent a build command.

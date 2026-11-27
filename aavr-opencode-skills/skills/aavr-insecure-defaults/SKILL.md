---
name: aavr-insecure-defaults
description: Detect fail-open insecure defaults — hardcoded secrets, fallback credentials, weak crypto, and permissive security settings that let a program run insecurely when configuration is missing. Use during a static-analysis or configuration pass. Complements aavr-static-analysis and aavr-finding-triage.
---

# AAVR Insecure Defaults

Find **fail-open** code: a program that keeps running with a weak or default secret when
configuration is absent, instead of refusing to start. Distinguish it from fail-secure code
that crashes safely.

- **Fail-open (finding):** `key = getenv("JWT_SECRET"); if (!key) key = "default";` — runs
  with a guessable secret.
- **Fail-secure (safe):** the program aborts when the variable is missing.

## Where to look

Search production-reachable configuration, auth, crypto, and startup code — not tests,
examples, or dev-only tooling. Patterns worth grepping, adapted to the project's language:

- Fallback secrets: `getenv(...) ? ... : "literal"`, `X || "literal"`, `fetch(..., default:)`.
- Hardcoded credentials: literal passwords, API keys, or tokens assigned to a variable.
- Weak toggles: debug on by default, auth disabled by default, CORS or permissions `*`/`0777`.
- Weak crypto in a security context: MD5, SHA1, DES, RC4, ECB used for passwords, tokens, or
  encryption (not for checksums).

## Confirm before reporting

For each match, trace the code path:

1. **Behaviour** — does the program actually run with the default, or does it abort when the
   config is missing? Only the former is fail-open.
2. **Reachability** — is this startup or runtime code that production actually executes?
3. **Production impact** — if the deployed config always supplies the value, lower the
   severity, but it is still a code-level defect; if the default can reach production, it is
   critical.

Reject the usual rationalizations ("just a dev default", "prod overrides it", "it's behind
auth") unless you can prove the secure path with a trace.

Report with evidence: the file:line, the pattern, how the default is reached, and the
concrete exploit (e.g. an attacker forges tokens with the known default). See
`references/examples.md` for worked fail-open vs fail-secure cases.

## Handoff

Record confirmed issues as candidates and load `aavr-finding-triage`. Any exposed real secret
must be reported for rotation, not just noted.

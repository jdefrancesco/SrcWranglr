---
description: Analyze cryptographic code for timing side-channel leaks
---

Load `aavr-constant-time-analysis`. Run the constant-time analyzer on the crypto and secret-handling code identified by reconnaissance and the attack surface. $ARGUMENTS

Treat each flagged instruction as a candidate: confirm the operand is secret-derived and the code is reachable before raising severity, and hand confirmed leaks to `aavr-finding-triage`.

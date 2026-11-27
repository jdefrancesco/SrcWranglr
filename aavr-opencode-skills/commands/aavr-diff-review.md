---
description: Security-focused review of a diff, commit range, or PR
---

Load `aavr-differential-review`. Review the specified change for security impact, classifying each hunk by risk and backing findings with git history, blast radius, and concrete attack scenarios. $ARGUMENTS

When SrcWranglr is available, use its commit-range audit to see what changed. Write the report under `.aavr/reports/` and hand strong candidates to `aavr-finding-triage`.

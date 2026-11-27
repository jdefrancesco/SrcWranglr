# AAVR OpenCode Skills

Model-agnostic Agentic Augmented Vulnerability Research workflows for OpenCode. The bundle contains **20 skills** and does not select or require a particular model.

## Install globally

From the SrcWranglr repository:

```bash
cd aavr-opencode-skills
./install.sh                 # add --skills-only to skip the slash commands
```

Restart OpenCode, open a C or C++ repository, and run:

```text
/aavr
```

You can also ask naturally: `Use the AAVR workflow to assess this repository.`

## Install for one project

```bash
./install.sh --project /path/to/repository
```

This creates `/path/to/repository/.opencode/skills/` and `.opencode/commands/`.

## Included commands

- `/aavr` — complete coordinated workflow
- `/aavr-recon` — reconnaissance and attack-surface mapping
- `/aavr-baseline` — normal build and test baseline
- `/aavr-triage` — candidate finding triage
- `/aavr-fuzz` — target selection, harness generation, and smoke test
- `/aavr-report` — report from existing artifacts
- `/aavr-semgrep` — index, select, and run the private Semgrep corpus
- `/aavr-semgrep-author` — author and test a targeted Semgrep rule
- `/aavr-constant-time` — analyze cryptographic code for timing side-channels
- `/aavr-diff-review` — security-focused review of a diff, commit range, or PR

## Included skills

- `aavr-orchestrate`
- `aavr-repository-recon`
- `aavr-build-baseline`
- `aavr-attack-surface`
- `aavr-static-analysis`
- `aavr-insecure-defaults`
- `aavr-constant-time-analysis`
- `aavr-finding-triage`
- `aavr-sanitizer-validation`
- `aavr-fuzz-target-selection`
- `aavr-harness-generation`
- `aavr-property-based-testing`
- `aavr-fuzz-campaign`
- `aavr-crash-triage`
- `aavr-variant-analysis`
- `aavr-differential-review`
- `aavr-report`
- `aavr-semgrep-database`
- `aavr-semgrep-rule-authoring`

## Model selection

Choose any model through OpenCode as usual. The skills never hard-code a model. For example:

```bash
opencode --model <provider>/<model>
```

## Private Semgrep database

Set the external corpus path, then run `/aavr-semgrep`:

```bash
export AAVR_SEMGREP_RULES=/absolute/path/to/aavr-semgrep-rules
```

Global installation places `aavr-semgrep-index`, `aavr-semgrep-select`, and `aavr-semgrep-run` in `~/.local/bin` by default. Project installation places them in `.opencode/bin`.

## Permissions

Set `AAVR_SEMGREP_RULES` or copy `aavr.config.example.json` to `~/.config/aavr/config.json` when using a private Semgrep corpus. The rule database remains external to this package.

Review `opencode.permissions.example.jsonc` before merging it into your own configuration. It allows AAVR skill loading, prompts for unknown shell commands, and denies common destructive operations.

## Uninstall global installation

```bash
./uninstall.sh
```

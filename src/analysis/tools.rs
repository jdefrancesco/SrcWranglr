//! External analyzers (semgrep, cppcheck, flawfinder): the command that runs each on a
//! project, and reading its report into findings. Commands run without a
//! shell; the project path is passed as an argument.

use std::path::Path;

use serde::Deserialize;

use super::{Finding, Severity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Semgrep,
    Cppcheck,
    Flawfinder,
}

/// semgrep rules to use instead of the default `p/c`.
const SEMGREP_CONFIG_VAR: &str = "SRCWRANGLR_SEMGREP_CONFIG";
/// SrcWranglr's own semgrep rules, written next to yours before each run.
pub const BUNDLED_RULES: &str = include_str!("semgrep_rules.yaml");
pub const BUNDLED_RULES_FILE: &str = "srcwranglr-builtin.yaml";
/// cppcheck ids about cppcheck itself (code it couldn't parse, macros or
/// includes it couldn't see), not about the code's behavior.
const CPPCHECK_SELF_REPORTS: &[&str] = &[
    "syntaxError",
    "internalAstError",
    "internalError",
    "unknownMacro",
    "preprocessorErrorDirective",
    "missingInclude",
    "missingIncludeSystem",
    "toomanyconfigs",
    "checkersReport",
    "normalCheckLevelMaxBranches",
];
/// cppcheck's report line: fields separated by tabs, message last.
const CPPCHECK_TEMPLATE: &str = "{file}\\t{line}\\t{severity}\\t{id}\\t{cwe}\\t{message}";

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Tool::Semgrep => "semgrep",
            Tool::Cppcheck => "cppcheck",
            Tool::Flawfinder => super::flawfinder::TOOL,
        }
    }

    /// How to install it, for when it isn't found.
    pub fn install_hint(self) -> &'static str {
        match self {
            Tool::Semgrep => "brew install semgrep, or pipx install semgrep",
            Tool::Cppcheck => "brew install cppcheck, or apt install cppcheck",
            Tool::Flawfinder => "brew install flawfinder, or pipx install flawfinder",
        }
    }

    /// The program and arguments that run this tool on `root`; semgrep
    /// with each of `semgrep_rules` (files or folders of rules). Nothing
    /// is fetched online.
    pub fn command(self, root: &Path, semgrep_rules: &[String]) -> (String, Vec<String>) {
        let root = root.to_string_lossy().into_owned();
        let args = match self {
            Tool::Semgrep => [
                "scan".into(),
                "--json".into(),
                "--quiet".into(),
                "--metrics=off".into(),
                "--disable-version-check".into(),
                // Scan everything: tests, ignored and large files included
                // (untrusted code may hide in any of them).
                "--no-git-ignore".into(),
                "--x-ignore-semgrepignore-files".into(),
                "--max-target-bytes=0".into(),
                // The code may silence findings with `// nosemgrep`.
                "--disable-nosem".into(),
            ]
            .into_iter()
            .chain(
                semgrep_rules
                    .iter()
                    .flat_map(|rules| ["--config".to_string(), rules.clone()]),
            )
            .chain([root])
            .collect(),
            // Without --inline-suppr: the code may silence findings with
            // `// cppcheck-suppress`.
            Tool::Cppcheck => vec![
                "--enable=warning,portability".into(),
                "--quiet".into(),
                format!("--template={CPPCHECK_TEMPLATE}"),
                root,
            ],
            // Symlinks aren't followed (flawfinder's default); dot folders
            // are scanned, since untrusted code may hide in any of them; and
            // `// Flawfinder: ignore` comments in the code are not obeyed.
            Tool::Flawfinder => vec![
                "--csv".into(),
                "--quiet".into(),
                "--columns".into(),
                "--followdotdir".into(),
                "--neverignore".into(),
                root,
            ],
        };
        (self.name().into(), args)
    }

    /// Findings from the tool's output (stdout for semgrep and flawfinder,
    /// stderr for cppcheck), with paths made relative to `root`.
    pub fn parse(self, stdout: &str, stderr: &str, root: &Path) -> Result<Vec<Finding>, String> {
        match self {
            Tool::Semgrep => parse_semgrep(stdout, root),
            Tool::Cppcheck => Ok(parse_cppcheck(stderr, root)),
            Tool::Flawfinder => super::flawfinder::parse(stdout, root),
        }
    }
}

#[derive(Deserialize)]
struct SemgrepReport {
    results: Vec<SemgrepResult>,
    /// What went wrong: with the rules (which stops the run), or with
    /// some files (which doesn't).
    #[serde(default)]
    errors: Vec<serde_json::Value>,
}

/// Rule errors reported at most, each cut to `MAX_RULE_ERROR_CHARS`.
const MAX_RULE_ERRORS: usize = 3;
const MAX_RULE_ERROR_CHARS: usize = 600;

#[derive(Deserialize)]
struct SemgrepResult {
    check_id: String,
    path: String,
    start: SemgrepPosition,
    end: SemgrepPosition,
    extra: SemgrepExtra,
}

#[derive(Deserialize)]
struct SemgrepPosition {
    line: u32,
    /// 1-based, in bytes.
    #[serde(default)]
    col: u32,
}

/// The semgrep rules to run: `SRCWRANGLR_SEMGREP_CONFIG` if set, else
/// `rules_dir` (yours plus the bundled ones, refreshed in it now).
pub fn prepare_semgrep_rules(rules_dir: Option<&Path>) -> Result<String, String> {
    if let Some(config) = std::env::var(SEMGREP_CONFIG_VAR)
        .ok()
        .filter(|c| !c.trim().is_empty())
    {
        return Ok(config);
    }
    let dir = rules_dir.ok_or("No home folder to keep semgrep rules in")?;
    write_bundled_rules(dir)?;
    Ok(dir.to_string_lossy().into_owned())
}

/// Writes SrcWranglr's rules into `dir`, leaving other files alone.
fn write_bundled_rules(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(dir.join(BUNDLED_RULES_FILE), BUNDLED_RULES))
        .map_err(|e| format!("Could not write semgrep rules to {}: {e}", dir.display()))
}

#[derive(Deserialize)]
struct SemgrepExtra {
    #[serde(default)]
    message: String,
    #[serde(default)]
    severity: String,
    /// Rule metadata; `cwe` is a string or a list of them.
    #[serde(default)]
    metadata: serde_json::Value,
}

fn parse_semgrep(json: &str, root: &Path) -> Result<Vec<Finding>, String> {
    let report: SemgrepReport =
        serde_json::from_str(json).map_err(|e| format!("unexpected semgrep output: {e}"))?;
    let rule_errors = rule_errors(&report.errors);
    if report.results.is_empty() && !rule_errors.is_empty() {
        return Err(format!(
            "semgrep couldn't use the rules: {}",
            rule_errors.join("\n")
        ));
    }
    let findings = report
        .results
        .into_iter()
        .filter_map(|r| {
            let cwe = match &r.extra.metadata["cwe"] {
                serde_json::Value::String(cwe) => cwe_number(cwe),
                serde_json::Value::Array(list) => list.iter().find_map(|c| cwe_number(c.as_str()?)),
                _ => None,
            };
            Some(Finding {
                tool: Tool::Semgrep.name().into(),
                rule: rule_id(&r.check_id).to_string(),
                severity: match r.extra.severity.as_str() {
                    "ERROR" => Severity::High,
                    "WARNING" => Severity::Medium,
                    _ => Severity::Low,
                },
                message: r.extra.message,
                fix: None,
                cwe,
                file: project_relative(&r.path, root)?,
                line: r.start.line.max(1),
                columns: semgrep_columns(&r.start, &r.end),
                symbol: None,
            })
        })
        .collect::<Vec<Finding>>();
    // Overlapping patterns (`$P->$F` and `*$P`) can report one spot twice.
    let mut seen = std::collections::HashSet::new();
    Ok(findings
        .into_iter()
        .filter(|f| {
            let start = f.columns.map(|(start, _)| start);
            seen.insert((f.rule.clone(), f.file.clone(), f.line, start))
        })
        .collect())
}

/// What semgrep said was wrong with the rules (not with the files it
/// read, which it reports too).
fn rule_errors(errors: &[serde_json::Value]) -> Vec<String> {
    errors
        .iter()
        .filter(|e| e["level"].as_str() == Some("error"))
        .filter(|e| {
            let kind = e["type"].as_str().unwrap_or("");
            kind.contains("Rule") || kind.contains("Pattern") || kind == "SemgrepError"
        })
        .filter_map(|e| {
            let text = ["long_msg", "message", "short_msg"]
                .iter()
                .find_map(|key| e[*key].as_str().filter(|t| !t.trim().is_empty()))?;
            Some(text.trim().chars().take(MAX_RULE_ERROR_CHARS).collect())
        })
        .take(MAX_RULE_ERRORS)
        .collect()
}

fn parse_cppcheck(report: &str, root: &Path) -> Vec<Finding> {
    report
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.splitn(6, '\t').collect();
            let [file, line, severity, id, cwe, message] = fields.as_slice() else {
                return None;
            };
            if CPPCHECK_SELF_REPORTS.contains(id) {
                return None;
            }
            let severity = match *severity {
                "error" => Severity::High,
                "warning" => Severity::Medium,
                "style" | "performance" | "portability" => Severity::Low,
                // "information" is about cppcheck itself (missing includes).
                _ => return None,
            };
            Some(Finding {
                tool: Tool::Cppcheck.name().into(),
                rule: (*id).into(),
                severity,
                message: (*message).into(),
                fix: None,
                cwe: cwe.parse().ok().filter(|c| *c > 0),
                file: project_relative(file, root)?,
                line: line.parse().ok().filter(|l| *l > 0)?,
                columns: None,
                symbol: None,
            })
        })
        .collect()
}

/// 0-based byte columns of a result on its first line (to the end of the
/// line if it spans several).
fn semgrep_columns(start: &SemgrepPosition, end: &SemgrepPosition) -> Option<(u32, u32)> {
    let first = start.col.checked_sub(1)?;
    let last = match end.line == start.line {
        true => end.col.checked_sub(1)?,
        false => u32::MAX,
    };
    (first < last).then_some((first, last))
}

/// A semgrep rule id without the dotted path of the folder it was loaded
/// from (`Users.me..config.srcwranglr.semgrep.my-rule` → `my-rule`), so
/// ids, and verdicts keyed by them, don't depend on where rules live.
fn rule_id(check_id: &str) -> &str {
    const RULES_DIR: &str = "srcwranglr.semgrep.";
    if let Some(i) = check_id.find(RULES_DIR) {
        return &check_id[i + RULES_DIR.len()..];
    }
    match check_id.find("srcwranglr.") {
        Some(i) => &check_id[i..],
        None => check_id,
    }
}

/// `CWE-676: …` → 676.
pub(super) fn cwe_number(text: &str) -> Option<u32> {
    let digits: String = text
        .strip_prefix("CWE-")?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// `path` relative to `root` with `/` separators, if it is inside it.
pub(super) fn project_relative(path: &str, root: &Path) -> Option<String> {
    let path = Path::new(path);
    let relative = if path.is_absolute() {
        path.strip_prefix(root).ok()?
    } else {
        path
    };
    let parts: Vec<&str> = relative
        .components()
        .map(|c| match c {
            std::path::Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/src/proj";

    #[test]
    fn rules_semgrep_cant_use_are_explained_not_taken_for_no_matches() {
        let json = r#"{"results": [], "errors": [
            {"code": 4, "level": "error", "type": "InvalidRuleSchemaError",
             "long_msg": "One of these properties is missing: 'severity'"},
            {"code": 7, "level": "error", "type": "SemgrepError",
             "message": "invalid configuration file found (1 configs were invalid)"},
            {"code": 3, "level": "warn", "type": "Syntax error", "message": "in a.c"}]}"#;
        let error = parse_semgrep(json, Path::new(ROOT)).expect_err("rule errors");
        assert!(
            error.starts_with("semgrep couldn't use the rules:"),
            "{error}"
        );
        assert!(error.contains("'severity'"), "{error}");
        assert!(!error.contains("a.c"), "{error}");
    }

    #[test]
    fn files_semgrep_couldnt_read_dont_stop_the_findings() {
        let json = r#"{"results": [], "errors": [
            {"code": 3, "level": "warn", "type": "Syntax error", "message": "in a.c"}]}"#;
        assert_eq!(parse_semgrep(json, Path::new(ROOT)).expect("parse"), []);
    }

    #[test]
    fn reads_semgrep_results_relative_to_the_project() {
        let json = r#"{"results": [
            {"check_id": "c.lang.security.insecure-use-gets-fn",
             "path": "/src/proj/net/read.c",
             "start": {"line": 12, "col": 5}, "end": {"line": 12, "col": 9},
             "extra": {"message": "Avoid gets()", "severity": "ERROR",
                       "metadata": {"cwe": ["CWE-676: Use of Potentially Dangerous Function"]}}},
            {"check_id": "c.lang.style", "path": "util.c",
             "start": {"line": 3, "col": 1}, "end": {"line": 4, "col": 2},
             "extra": {"message": "style", "severity": "INFO"}},
            {"check_id": "elsewhere", "path": "/other/x.c",
             "start": {"line": 1, "col": 1}, "end": {"line": 1, "col": 2},
             "extra": {"message": "outside", "severity": "WARNING"}}
        ], "errors": []}"#;
        let findings = parse_semgrep(json, Path::new(ROOT)).expect("parse");
        let summary: Vec<(&str, &str, u32, Severity, Option<u32>)> = findings
            .iter()
            .map(|f| (f.rule.as_str(), f.file.as_str(), f.line, f.severity, f.cwe))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "c.lang.security.insecure-use-gets-fn",
                    "net/read.c",
                    12,
                    Severity::High,
                    Some(676)
                ),
                ("c.lang.style", "util.c", 3, Severity::Low, None),
            ]
        );
        assert!(findings.iter().all(|f| f.tool == "semgrep"));
        assert_eq!(findings[0].columns, Some((4, 8)));
        let twice = format!(
            "{{\"results\": [{0}, {0}]}}",
            r#"{"check_id": "r", "path": "a.c",
            "start": {"line": 1, "col": 1}, "end": {"line": 1, "col": 3},
            "extra": {"message": "m", "severity": "INFO"}}"#
        );
        assert_eq!(
            parse_semgrep(&twice, Path::new(ROOT)).expect("parse").len(),
            1
        );
        // Spans lines: highlighted to the end of the first.
        assert_eq!(findings[1].columns, Some((0, u32::MAX)));
    }

    #[test]
    fn bundled_rules_are_written_without_touching_yours() {
        let dir = tempfile::tempdir().expect("tmp");
        let rules = dir.path().join("semgrep");
        std::fs::create_dir(&rules).expect("mkdir");
        std::fs::write(rules.join("mine.yaml"), "rules: []\n").expect("write");
        write_bundled_rules(&rules).expect("write");
        let bundled = std::fs::read_to_string(rules.join(BUNDLED_RULES_FILE)).expect("read");
        assert!(bundled.contains("srcwranglr.use-after-free"));
        assert_eq!(
            std::fs::read_to_string(rules.join("mine.yaml")).expect("read"),
            "rules: []\n"
        );
    }

    /// Runs the real semgrep on planted bugs: `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs semgrep installed; slow"]
    fn bundled_rules_find_the_planted_bugs() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(dir.path().join("bugs.c"), PLANTED_BUGS).expect("write");
        let rules = dir.path().join("rules");
        write_bundled_rules(&rules).expect("rules");
        let (program, args) =
            Tool::Semgrep.command(dir.path(), &[rules.to_string_lossy().into_owned()]);
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .expect("semgrep");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let found = parse_semgrep(&stdout, dir.path()).expect("parse");
        let mut hits: Vec<(String, u32)> = found
            .iter()
            .map(|f| (f.rule.trim_start_matches("rules.").to_string(), f.line))
            .collect();
        hits.sort();
        let ids: Vec<String> = hits
            .iter()
            .map(|(rule, line)| format!("{}:{line}", rule.rsplit('.').next().unwrap_or(rule)))
            .collect();
        assert_eq!(
            ids,
            [
                "double-free:7",
                "realloc-leak:16",
                "return-stack-buffer:20",
                "snprintf-return-as-length:24",
                "strncpy-not-terminated:27",
                "unchecked-malloc:11",
                "use-after-free:4",
            ],
            "{stdout}"
        );
    }

    /// One planted bug per bundled rule (plus safe twins that mustn't fire).
    const PLANTED_BUGS: &str = "#include <stdlib.h>
#include <string.h>
#include <stdio.h>
struct node { int v; }; void uaf(struct node *n) { free(n); n->v = 1; }
void uaf_ok(struct node *n) { free(n); n = NULL; }
void dfree(char *p) {
  free(p);
  free(p);
}
int unchecked(void) { struct node *p = NULL; p = malloc(sizeof *p);
  return p->v; }
int checked(void) { struct node *p = NULL; p = malloc(sizeof *p);
  if (p == NULL) return -1;
  return p->v; }
void grow(char *buf, size_t n) {
  buf = realloc(buf, n); }
char *stack(void) {
  char name[32];
  strcpy(name, \"x\");
  return name; }
void trunc_len(char *buf, size_t size, const char *s) {
  int n;
  n = snprintf(buf, size, \"%s\", s);
  buf[n] = 0; }
void noterm(const char *s) {
  char d[16];
  strncpy(d, s, sizeof(d)); puts(d); }
void term(const char *s) {
  char d[16];
  strncpy(d, s, sizeof(d)); d[sizeof(d) - 1] = 0; puts(d); }
";

    #[test]
    fn rule_ids_do_not_depend_on_the_rules_folder() {
        assert_eq!(
            rule_id("Users.me..config.srcwranglr.semgrep.srcwranglr.use-after-free"),
            "srcwranglr.use-after-free"
        );
        assert_eq!(
            rule_id("Users.me..config.srcwranglr.semgrep.my-rule"),
            "my-rule"
        );
        assert_eq!(
            rule_id("tmp.rules.srcwranglr.double-free"),
            "srcwranglr.double-free"
        );
        assert_eq!(rule_id("c.lang.security.gets"), "c.lang.security.gets");
    }

    #[test]
    fn bad_semgrep_output_is_an_error() {
        assert!(parse_semgrep("Traceback (most recent call last)", Path::new(ROOT)).is_err());
    }

    #[test]
    fn reads_cppcheck_template_lines() {
        let report = "/src/proj/a.c\t7\terror\tbufferAccessOutOfBounds\t788\tBuffer is accessed out of bounds: buf\n\
                      Checking /src/proj/b.c ...\n\
                      /src/proj/b.c\t2\tportability\tinvalidPointerCast\t0\tCasting\tbetween types\n\
                      nofile\t0\tinformation\tmissingInclude\t0\tCannot find all includes\n\
                      /src/proj/t.cc\t14\terror\tsyntaxError\t0\tsyntax error\n";
        let findings = parse_cppcheck(report, Path::new(ROOT));
        let summary: Vec<(&str, u32, Severity, Option<u32>, &str)> = findings
            .iter()
            .map(|f| {
                (
                    f.file.as_str(),
                    f.line,
                    f.severity,
                    f.cwe,
                    f.message.as_str(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "a.c",
                    7,
                    Severity::High,
                    Some(788),
                    "Buffer is accessed out of bounds: buf"
                ),
                ("b.c", 2, Severity::Low, None, "Casting\tbetween types"),
            ]
        );
    }

    #[test]
    fn commands_pass_the_project_as_an_argument() {
        let (program, args) = Tool::Cppcheck.command(Path::new("/p q/r"), &[]);
        assert_eq!(program, "cppcheck");
        assert_eq!(args.last().map(String::as_str), Some("/p q/r"));
        let (_, args) = Tool::Semgrep.command(Path::new("/p"), &["/rules".to_string()]);
        for offline in ["--metrics=off", "--disable-version-check", "/rules"] {
            assert!(args.contains(&offline.to_string()), "{args:?}");
        }
        let (program, args) = Tool::Flawfinder.command(Path::new("/p q/r"), &[]);
        assert_eq!(program, "flawfinder");
        assert_eq!(args.last().map(String::as_str), Some("/p q/r"));
        assert!(args.contains(&"--csv".to_string()));
    }

    #[test]
    fn the_code_cannot_silence_the_analyzers() {
        let has = |tool: Tool, flag: &str| {
            let (_, args) = tool.command(Path::new("/p"), &["/rules".to_string()]);
            args.iter().any(|a| a == flag)
        };
        assert!(has(Tool::Semgrep, "--disable-nosem"));
        assert!(has(Tool::Flawfinder, "--neverignore"));
        assert!(!has(Tool::Cppcheck, "--inline-suppr"));
    }
}

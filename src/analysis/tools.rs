//! External analyzers (semgrep, cppcheck): the command that runs each on a
//! project, and reading its report into findings. Commands run without a
//! shell; the project path is passed as an argument.

use std::path::Path;

use serde::Deserialize;

use super::{Finding, Severity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Semgrep,
    Cppcheck,
}

/// semgrep rules to use instead of the default `p/c`.
const SEMGREP_CONFIG_VAR: &str = "SLURCETRAIL_SEMGREP_CONFIG";
const SEMGREP_DEFAULT_CONFIG: &str = "p/c";
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
        }
    }

    /// Program and arguments that analyze `root`.
    pub fn command(self, root: &Path) -> (String, Vec<String>) {
        let root = root.to_string_lossy().into_owned();
        let args = match self {
            Tool::Semgrep => {
                let config = std::env::var(SEMGREP_CONFIG_VAR)
                    .ok()
                    .filter(|c| !c.trim().is_empty())
                    .unwrap_or_else(|| SEMGREP_DEFAULT_CONFIG.into());
                vec![
                    "scan".into(),
                    "--json".into(),
                    "--quiet".into(),
                    "--metrics=off".into(),
                    "--config".into(),
                    config,
                    root,
                ]
            }
            Tool::Cppcheck => vec![
                "--enable=warning,portability".into(),
                "--inline-suppr".into(),
                "--quiet".into(),
                format!("--template={CPPCHECK_TEMPLATE}"),
                root,
            ],
        };
        (self.name().into(), args)
    }

    /// Findings from the tool's output (stdout for semgrep, stderr for
    /// cppcheck), with paths made relative to `root`.
    pub fn parse(self, stdout: &str, stderr: &str, root: &Path) -> Result<Vec<Finding>, String> {
        match self {
            Tool::Semgrep => parse_semgrep(stdout, root),
            Tool::Cppcheck => Ok(parse_cppcheck(stderr, root)),
        }
    }
}

#[derive(Deserialize)]
struct SemgrepReport {
    results: Vec<SemgrepResult>,
}

#[derive(Deserialize)]
struct SemgrepResult {
    check_id: String,
    path: String,
    start: SemgrepPosition,
    extra: SemgrepExtra,
}

#[derive(Deserialize)]
struct SemgrepPosition {
    line: u32,
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
                rule: r.check_id,
                severity: match r.extra.severity.as_str() {
                    "ERROR" => Severity::High,
                    "WARNING" => Severity::Medium,
                    _ => Severity::Low,
                },
                message: r.extra.message,
                cwe,
                file: project_relative(&r.path, root)?,
                line: r.start.line.max(1),
                symbol: None,
            })
        })
        .collect();
    Ok(findings)
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
                cwe: cwe.parse().ok().filter(|c| *c > 0),
                file: project_relative(file, root)?,
                line: line.parse().ok().filter(|l| *l > 0)?,
                symbol: None,
            })
        })
        .collect()
}

/// `CWE-676: …` → 676.
fn cwe_number(text: &str) -> Option<u32> {
    let digits: String = text
        .strip_prefix("CWE-")?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// `path` relative to `root` with `/` separators, if it is inside it.
fn project_relative(path: &str, root: &Path) -> Option<String> {
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
    fn reads_semgrep_results_relative_to_the_project() {
        let json = r#"{"results": [
            {"check_id": "c.lang.security.insecure-use-gets-fn",
             "path": "/src/proj/net/read.c",
             "start": {"line": 12, "col": 5},
             "extra": {"message": "Avoid gets()", "severity": "ERROR",
                       "metadata": {"cwe": ["CWE-676: Use of Potentially Dangerous Function"]}}},
            {"check_id": "c.lang.style", "path": "util.c",
             "start": {"line": 3, "col": 1},
             "extra": {"message": "style", "severity": "INFO"}},
            {"check_id": "elsewhere", "path": "/other/x.c",
             "start": {"line": 1, "col": 1},
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
        let (program, args) = Tool::Cppcheck.command(Path::new("/p q/r"));
        assert_eq!(program, "cppcheck");
        assert_eq!(args.last().map(String::as_str), Some("/p q/r"));
        let (_, args) = Tool::Semgrep.command(Path::new("/p"));
        assert!(args.contains(&"--metrics=off".to_string()));
    }
}

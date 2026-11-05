//! Runs an external analyzer on a background thread, so the UI stays
//! responsive while semgrep or cppcheck works through the project.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use super::dangerous_lists::semgrep_rules_dir;
use crate::analysis::tools::{prepare_semgrep_rules, Tool};
use crate::analysis::Finding;
use crate::model::{CodeGraph, SymbolId, SymbolKind};

/// How much of a failed tool's error output to show.
const MAX_ERROR_CHARS: usize = 300;

pub struct ToolJob {
    tool: Tool,
    results: Receiver<Result<Vec<Finding>, String>>,
}

impl ToolJob {
    pub fn spawn(tool: Tool, root: PathBuf, ctx: egui::Context) -> Self {
        let (tx, results) = channel();
        std::thread::spawn(move || {
            let rules = match tool {
                Tool::Semgrep => prepare_semgrep_rules(semgrep_rules_dir().as_deref()),
                Tool::Cppcheck => Ok(String::new()),
            };
            let result = rules.and_then(|rules| {
                let (program, args) = tool.command(&root, &rules);
                run(&program, &args, |out, err| tool.parse(out, err, &root))
            });
            // The app may be gone (closed) by now; nothing to report to then.
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        Self { tool, results }
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// The findings (or why there are none) once the tool is done.
    pub fn poll(&self) -> Option<Result<Vec<Finding>, String>> {
        match self.results.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err(format!("{} stopped unexpectedly", self.tool.name())))
            }
        }
    }
}

/// Runs `program` and parses its output; an unparseable report from a
/// failing run is explained with the start of its error output.
fn run(
    program: &str,
    args: &[String],
    parse: impl Fn(&str, &str) -> Result<Vec<Finding>, String>,
) -> Result<Vec<Finding>, String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => format!("{program} is not installed (or not on PATH)"),
            _ => format!("Could not run {program}: {e}"),
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    parse(&stdout, &stderr).map_err(|error| {
        let detail: String = stderr.trim().chars().take(MAX_ERROR_CHARS).collect();
        match output.status.success() {
            true => error,
            false => format!("{program} failed ({}): {detail}", output.status),
        }
    })
}

/// `findings` with each one's enclosing function (the last one declared
/// at or above its line) filled in. (Byte columns that don't fall on a
/// character boundary are ignored when drawn.)
pub fn with_symbols(findings: Vec<Finding>, graph: &CodeGraph) -> Vec<Finding> {
    findings
        .into_iter()
        .map(|finding| Finding {
            symbol: enclosing_function(graph, &finding.file, finding.line),
            ..finding
        })
        .collect()
}

fn enclosing_function(graph: &CodeGraph, file: &str, line: u32) -> Option<SymbolId> {
    graph
        .symbols
        .values()
        .filter(|s| s.file == file && s.line <= line)
        .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
        .max_by_key(|s| (s.line, s.id))
        .map(|s| s.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Severity;

    #[test]
    fn a_missing_program_says_it_is_not_installed() {
        let error =
            run("slurcetrail-no-such-tool", &[], |_, _| Ok(Vec::new())).expect_err("missing");
        assert!(error.contains("not installed"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_run_explains_itself_with_its_error_output() {
        let args = ["-c".to_string(), "echo boom >&2; exit 3".to_string()];
        let error = run("/bin/sh", &args, |_, _| Err("bad report".into())).expect_err("fails");
        assert!(error.contains("boom") && error.contains('3'), "{error}");
    }

    #[test]
    fn findings_get_their_enclosing_function() {
        let graph = crate::demo::demo_codebase().expect("demo");
        let finding = Finding {
            tool: "t".into(),
            rule: "r".into(),
            severity: Severity::Low,
            message: String::new(),
            fix: None,
            cwe: None,
            file: "main.cpp".into(),
            line: 10,
            columns: None,
            symbol: None,
        };
        let found = with_symbols(vec![finding], &graph);
        let name = found[0]
            .symbol
            .map(|id| graph.symbols[&id].qualified_name.as_str());
        assert_eq!(name, Some("main"));
    }

    #[test]
    fn tool_byte_columns_are_kept() {
        let graph = crate::demo::demo_codebase().expect("demo");
        let finding = Finding {
            tool: "semgrep".into(),
            rule: "r".into(),
            severity: Severity::Low,
            message: String::new(),
            fix: None,
            cwe: None,
            file: "u.c".into(),
            line: 1,
            columns: Some((8, 15)),
            symbol: None,
        };
        let converted = with_symbols(vec![finding], &graph);
        // semgrep reports byte columns, which the code view uses as they are.
        assert_eq!(converted[0].columns, Some((8, 15)));
    }

    #[test]
    fn job_reports_a_missing_tool_through_poll() {
        // `poll` before the thread finishes is `None`; eventually it's done.
        let job = ToolJob::spawn(
            Tool::Cppcheck,
            PathBuf::from("/nonexistent-slurcetrail-root"),
            egui::Context::default(),
        );
        let result = (0..200).find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(25));
            job.poll()
        });
        assert!(result.is_some());
        assert_eq!(job.tool(), Tool::Cppcheck);
    }
}

//! Runs an external analyzer on a background thread, so the UI stays
//! responsive while semgrep, cppcheck or flawfinder works through the
//! project. The program is looked for where Settings says (see
//! `programs`), and runs with that search as its `PATH`.

use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use super::dangerous_lists::semgrep_rules_dir;
use crate::analysis::tools::{prepare_semgrep_rules, Tool};
use crate::analysis::Finding;
use crate::model::{CodeGraph, SymbolId, SymbolKind};
use crate::programs::SearchPath;

/// How much of a failed tool's error output to show.
const MAX_ERROR_CHARS: usize = 300;

pub struct ToolJob {
    tool: Tool,
    results: Receiver<Result<Vec<Finding>, String>>,
}

/// Which semgrep rules a run uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemgrepRules {
    /// Yours and SrcWranglr's, plus the project's own folder of them (if
    /// you trust it).
    Yours { project: Option<PathBuf> },
    /// Just this rule file, removed once the run is done.
    Only(PathBuf),
}

impl ToolJob {
    /// Runs `tool` on `root`; semgrep with your rules (and `project_rules`,
    /// a folder of the project's own, when given).
    pub fn spawn(
        tool: Tool,
        root: PathBuf,
        search: SearchPath,
        project_rules: Option<PathBuf>,
        ctx: egui::Context,
    ) -> Self {
        let rules = SemgrepRules::Yours {
            project: project_rules,
        };
        Self::start(tool, root, search, rules, ctx)
    }

    /// Runs semgrep on `root` with only the rule in `rule_file`, which is
    /// removed afterwards.
    pub fn spawn_rule_test(
        rule_file: PathBuf,
        root: PathBuf,
        search: SearchPath,
        ctx: egui::Context,
    ) -> Self {
        Self::start(
            Tool::Semgrep,
            root,
            search,
            SemgrepRules::Only(rule_file),
            ctx,
        )
    }

    fn start(
        tool: Tool,
        root: PathBuf,
        search: SearchPath,
        rules: SemgrepRules,
        ctx: egui::Context,
    ) -> Self {
        let (tx, results) = channel();
        std::thread::spawn(move || {
            let yours = || prepare_semgrep_rules(semgrep_rules_dir().as_deref());
            let result = rules_for(tool, &rules, yours).and_then(|rules| {
                let (program, args) = tool.command(&root, &rules);
                let found = locate(tool, &program, &search)?;
                let parse = |out: &str, err: &str| tool.parse(out, err, &root);
                run(&found, &args, &search.joined(), parse)
            });
            if let SemgrepRules::Only(file) = &rules {
                // Ours to remove; one left behind is only a stray draft.
                let _ = std::fs::remove_file(file);
            }
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

/// The rules `tool` runs with: for semgrep, what `rules` says, `yours`
/// giving (and refreshing) your folder of them.
fn rules_for(
    tool: Tool,
    rules: &SemgrepRules,
    yours: impl FnOnce() -> Result<String, String>,
) -> Result<Vec<String>, String> {
    if tool != Tool::Semgrep {
        return Ok(Vec::new());
    }
    let path = |p: &Path| p.to_string_lossy().into_owned();
    match rules {
        SemgrepRules::Only(file) => Ok(vec![path(file)]),
        SemgrepRules::Yours { project } => Ok(std::iter::once(yours()?)
            .chain(project.as_deref().map(path))
            .collect()),
    }
}

/// Where `tool`'s `program` is, or how to get it found.
fn locate(tool: Tool, program: &str, search: &SearchPath) -> Result<PathBuf, String> {
    search.find(program).ok_or_else(|| {
        format!(
            "{program} is not installed, or not in a folder SrcWranglr searches. \
             Install it ({}), or add its folder in Settings (⌘,).",
            tool.install_hint()
        )
    })
}

/// Runs `program` with `path_var` as its `PATH` and parses its output; an
/// unparseable report from a failing run is explained with the start of
/// its error output.
fn run(
    program: &Path,
    args: &[String],
    path_var: &OsStr,
    parse: impl Fn(&str, &str) -> Result<Vec<Finding>, String>,
) -> Result<Vec<Finding>, String> {
    let shown = program.display();
    let output = Command::new(program)
        .args(args)
        .env("PATH", path_var)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => format!("{shown} is not installed (or not on PATH)"),
            _ => format!("Could not run {shown}: {e}"),
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    parse(&stdout, &stderr).map_err(|error| {
        let detail: String = stderr.trim().chars().take(MAX_ERROR_CHARS).collect();
        match output.status.success() {
            true => error,
            false => format!("{shown} failed ({}): {detail}", output.status),
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
    fn a_missing_program_says_how_to_get_it_found() {
        let search = SearchPath::new(&[], None, None);
        let error =
            locate(Tool::Flawfinder, "srcwranglr-no-such-tool", &search).expect_err("missing");
        assert!(error.contains("not installed"), "{error}");
        assert!(error.contains("pipx install flawfinder"), "{error}");
        assert!(error.contains("Settings"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_run_explains_itself_with_its_error_output() {
        let args = ["-c".to_string(), "echo boom >&2; exit 3".to_string()];
        let parse = |_: &str, _: &str| Err("bad report".into());
        let error =
            run(Path::new("/bin/sh"), &args, OsStr::new("/usr/bin"), parse).expect_err("fails");
        assert!(error.contains("boom") && error.contains('3'), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn the_program_runs_with_the_search_as_its_path() {
        let args = ["-c".to_string(), "echo \"$PATH\" >&2; exit 1".to_string()];
        let parse = |_: &str, _: &str| Err("no report".into());
        let error = run(
            Path::new("/bin/sh"),
            &args,
            OsStr::new("/my/tools:/usr/bin"),
            parse,
        )
        .expect_err("fails");
        assert!(error.contains("/my/tools:/usr/bin"), "{error}");
    }

    /// With the short PATH an app opened from the Dock gets, flawfinder
    /// is still found (in the usual places) and its report read. Skipped
    /// where flawfinder isn't installed.
    #[test]
    fn flawfinder_is_found_and_read_with_a_short_path() {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let search = SearchPath::new(&[], Some("/usr/bin:/bin".into()), home.as_deref());
        let Some(program) = search.find("flawfinder") else {
            return;
        };
        let project = tempfile::tempdir().expect("dir");
        let code = "void f(char *s) {\n  char b[8];\n  strcpy(b, s);\n}\n";
        std::fs::write(project.path().join("a.c"), code).expect("write");
        let (_, args) = Tool::Flawfinder.command(project.path(), &[]);
        let root = project.path();
        let parse = |out: &str, err: &str| Tool::Flawfinder.parse(out, err, root);
        let findings = run(&program, &args, &search.joined(), parse).expect("ran");
        let strcpy = findings
            .iter()
            .find(|f| f.rule == "strcpy")
            .expect("strcpy flagged");
        assert_eq!((strcpy.file.as_str(), strcpy.line), ("a.c", 3));
        assert_eq!(strcpy.columns, Some((2, 8)));
    }

    #[test]
    fn semgrep_runs_your_rules_and_the_projects_or_only_a_draft() {
        let yours = || Ok("/me/rules".to_string());
        let project = Some(PathBuf::from("/p/.srcwranglr/semgrep"));
        assert_eq!(
            rules_for(Tool::Semgrep, &SemgrepRules::Yours { project }, yours),
            Ok(vec!["/me/rules".into(), "/p/.srcwranglr/semgrep".into()])
        );
        let untrusted = SemgrepRules::Yours { project: None };
        assert_eq!(
            rules_for(Tool::Semgrep, &untrusted, yours),
            Ok(vec!["/me/rules".to_string()])
        );
        let draft = SemgrepRules::Only(PathBuf::from("/tmp/draft.yaml"));
        let unused = || Err("not asked for".to_string());
        assert_eq!(
            rules_for(Tool::Semgrep, &draft, unused),
            Ok(vec!["/tmp/draft.yaml".to_string()])
        );
        assert_eq!(rules_for(Tool::Cppcheck, &draft, unused), Ok(Vec::new()));
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
            PathBuf::from("/nonexistent-srcwranglr-root"),
            SearchPath::new(&[], None, None),
            None,
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

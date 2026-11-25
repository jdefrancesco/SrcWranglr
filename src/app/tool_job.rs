//! Runs an external analyzer on a background thread, so the UI stays
//! responsive while semgrep, cppcheck or flawfinder works through the
//! project. The program is looked for where Settings says (see
//! `programs`), and runs with that search as its `PATH`.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::dangerous_lists::semgrep_rules_dir;
use crate::analysis::tools::{prepare_semgrep_rules, Tool};
use crate::analysis::Finding;
use crate::model::{CodeGraph, SymbolId, SymbolKind};
use crate::programs::SearchPath;

/// How much of a failed tool's error output to show.
const MAX_ERROR_CHARS: usize = 300;
/// How much of a tool's latest progress line to show in the status bar.
const MAX_DETAIL_CHARS: usize = 90;
/// How long a single progress segment may grow before it is cut, so a tool
/// that never emits a newline can't grow the buffer without bound. Findings
/// (cppcheck's come on stderr) sit on their own lines, well under this.
const MAX_SEGMENT_BYTES: usize = 64 * 1024;

/// The latest progress line a running tool has printed, shared with the UI.
type Detail = Arc<Mutex<Option<String>>>;

pub struct ToolJob {
    tool: Tool,
    /// The subfolder being scanned (relative to the project), or `None` for
    /// the whole project — shown in the running status line.
    scope: Option<String>,
    /// When the run began, for the elapsed time shown while it works.
    started: Instant,
    results: Receiver<Result<Vec<Finding>, String>>,
    /// The latest progress line from the tool's stderr (best effort).
    detail: Detail,
    /// The empty folder a check ran in, removed once it is done.
    empty: Option<PathBuf>,
}

impl Drop for ToolJob {
    fn drop(&mut self) {
        if let Some(empty) = &self.empty {
            // Ours and empty; a run still going only reads it.
            let _ = std::fs::remove_dir(empty);
        }
    }
}

/// A new, empty folder of SrcWranglr's own in the temp folder.
fn empty_folder() -> std::io::Result<PathBuf> {
    // Hard to guess, so another user of a shared temp folder can't take
    // the name first; create_dir fails if they did.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let dir = std::env::temp_dir().join(format!(
        "srcwranglr-check-{}-{}-{nanos}",
        std::process::id(),
        CHECK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir)?;
    Ok(dir)
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

/// Tells apart the empty folders of checks started by this process.
static CHECK_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl ToolJob {
    /// Runs `tool` on `target` (the whole project `root`, or a subfolder of
    /// it); semgrep with your rules (and `project_rules`, a folder of the
    /// project's own, when given). `scope` names the subfolder for the status
    /// line, or is `None` for the whole project. Findings stay relative to
    /// `root` either way.
    pub fn spawn(
        tool: Tool,
        root: PathBuf,
        target: PathBuf,
        scope: Option<String>,
        search: SearchPath,
        project_rules: Option<PathBuf>,
        ctx: egui::Context,
    ) -> Self {
        let rules = SemgrepRules::Yours {
            project: project_rules,
        };
        Self::start(tool, root, target, scope, search, rules, ctx)
    }

    /// Runs semgrep on `root` with only the rule in `rule_file`, which is
    /// removed afterwards.
    pub fn spawn_rule_test(
        rule_file: PathBuf,
        root: PathBuf,
        search: SearchPath,
        ctx: egui::Context,
    ) -> Self {
        let target = root.clone();
        Self::start(
            Tool::Semgrep,
            root,
            target,
            None,
            search,
            SemgrepRules::Only(rule_file),
            ctx,
        )
    }

    /// Has semgrep read the rule in `rule_file` (removed afterwards) and
    /// say whether it can use it: a run on an empty folder of its own, so
    /// nothing of the project is read.
    pub fn spawn_rule_check(
        rule_file: PathBuf,
        search: SearchPath,
        ctx: egui::Context,
    ) -> Result<Self, String> {
        let empty = empty_folder().map_err(|e| {
            let _ = std::fs::remove_file(&rule_file);
            format!("Could not make a folder to check the rule in: {e}")
        })?;
        let rules = SemgrepRules::Only(rule_file);
        let target = empty.clone();
        let mut job = Self::start(
            Tool::Semgrep,
            empty.clone(),
            target,
            None,
            search,
            rules,
            ctx,
        );
        job.empty = Some(empty);
        Ok(job)
    }

    fn start(
        tool: Tool,
        root: PathBuf,
        target: PathBuf,
        scope: Option<String>,
        search: SearchPath,
        rules: SemgrepRules,
        ctx: egui::Context,
    ) -> Self {
        let (tx, results) = channel();
        let detail: Detail = Arc::new(Mutex::new(None));
        let worker_detail = Arc::clone(&detail);
        std::thread::spawn(move || {
            let yours = || prepare_semgrep_rules(semgrep_rules_dir().as_deref());
            let result = rules_for(tool, &rules, yours).and_then(|rules| {
                let (program, args) = tool.command(&target, &rules);
                let found = locate(tool, &program, &search)?;
                let parse = |out: &str, err: &str| tool.parse(out, err, &root);
                run(&found, &args, &search.joined(), &worker_detail, parse)
            });
            if let SemgrepRules::Only(file) = &rules {
                // Ours to remove; one left behind is only a stray draft.
                let _ = std::fs::remove_file(file);
            }
            // The app may be gone (closed) by now; nothing to report to then.
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        Self {
            tool,
            scope,
            started: Instant::now(),
            results,
            detail,
            empty: None,
        }
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// The latest progress line the tool has printed, if any.
    pub fn detail(&self) -> Option<String> {
        self.detail.lock().ok().and_then(|d| d.clone())
    }

    /// A one-line summary for the status bar while the tool runs: where it is
    /// scanning, how long it has taken, and its latest progress line.
    pub fn status_line(&self, project_name: &str) -> String {
        let where_ = self.scope.as_deref().unwrap_or(project_name);
        let elapsed = format_elapsed(self.started.elapsed());
        match self.detail() {
            Some(detail) => {
                let detail: String = detail.chars().take(MAX_DETAIL_CHARS).collect();
                format!("{} on {where_} — {elapsed} · {detail}", self.tool.name())
            }
            None => format!("{} on {where_} — {elapsed}", self.tool.name()),
        }
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

/// `45s`, or `3m 12s` once past a minute.
fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
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

/// Runs `program` with `path_var` as its `PATH`, streaming its stderr to
/// `detail` as a live progress line while reading all of stdout; then parses
/// the output. An unparseable report from a failing run is explained with the
/// start of its error output.
fn run(
    program: &Path,
    args: &[String],
    path_var: &OsStr,
    detail: &Detail,
    parse: impl Fn(&str, &str) -> Result<Vec<Finding>, String>,
) -> Result<Vec<Finding>, String> {
    let shown = program.display();
    let mut child = Command::new(program)
        .args(args)
        .env("PATH", path_var)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => format!("{shown} is not installed (or not on PATH)"),
            _ => format!("Could not run {shown}: {e}"),
        })?;
    let mut out_pipe = child.stdout.take().expect("stdout was piped");
    let err_pipe = child.stderr.take().expect("stderr was piped");

    // Drain stdout on its own thread — findings come from here (semgrep,
    // flawfinder), and reading both pipes at once avoids a full-pipe deadlock.
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf);
        buf
    });

    let stderr = stream_stderr(err_pipe, detail);

    let stdout_bytes = stdout_reader.join().unwrap_or_default();
    let status = child
        .wait()
        .map_err(|e| format!("Could not run {shown}: {e}"))?;
    // The run is over; stop showing a stale progress line.
    if let Ok(mut d) = detail.lock() {
        *d = None;
    }
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    parse(&stdout, &stderr).map_err(|error| {
        let head: String = stderr.trim().chars().take(MAX_ERROR_CHARS).collect();
        match status.success() {
            true => error,
            false => format!("{shown} failed ({status}): {head}"),
        }
    })
}

/// Reads `pipe` to the end, splitting on CR and LF so a progress bar that
/// redraws in place with `\r` still updates `detail` with its latest line.
/// Returns all the lines it collected (full, since cppcheck reports its
/// findings on stderr), newline-joined.
fn stream_stderr(mut pipe: impl Read, detail: &Detail) -> String {
    let mut text = String::new();
    let mut segment: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                for &byte in &chunk[..n] {
                    if byte == b'\r' || byte == b'\n' {
                        record_segment(&mut segment, &mut text, detail);
                    } else if segment.len() < MAX_SEGMENT_BYTES {
                        segment.push(byte);
                    }
                }
            }
        }
    }
    record_segment(&mut segment, &mut text, detail);
    text
}

/// Keeps a finished stderr line: appends it to `text` and makes it the current
/// progress `detail`. Blank lines are dropped.
fn record_segment(segment: &mut Vec<u8>, text: &mut String, detail: &Detail) {
    if segment.is_empty() {
        return;
    }
    let line = String::from_utf8_lossy(segment).trim().to_string();
    segment.clear();
    if line.is_empty() {
        return;
    }
    text.push_str(&line);
    text.push('\n');
    if let Ok(mut d) = detail.lock() {
        *d = Some(line);
    }
}

/// `findings` with each one's enclosing function (the last one declared
/// at or above its line) filled in. (Byte columns that don't fall on a
/// character boundary are ignored when drawn.)
pub fn with_symbols(findings: Vec<Finding>, graph: &CodeGraph) -> Vec<Finding> {
    // Each file's functions by line, once: a run can report many places.
    let mut functions: HashMap<&str, Vec<(u32, SymbolId)>> = HashMap::new();
    let all = graph
        .symbols
        .values()
        .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method));
    for symbol in all {
        functions
            .entry(symbol.file.as_str())
            .or_default()
            .push((symbol.line, symbol.id));
    }
    for list in functions.values_mut() {
        list.sort_unstable();
    }
    findings
        .into_iter()
        .map(|finding| Finding {
            symbol: enclosing_function(&functions, &finding.file, finding.line),
            ..finding
        })
        .collect()
}

/// The last function declared at or above `line` of `file`.
fn enclosing_function(
    functions: &HashMap<&str, Vec<(u32, SymbolId)>>,
    file: &str,
    line: u32,
) -> Option<SymbolId> {
    let list = functions.get(file)?;
    let at = list.partition_point(|(start, _)| *start <= line);
    list.get(at.checked_sub(1)?).map(|(_, id)| *id)
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
        let detail: Detail = Arc::new(Mutex::new(None));
        let error = run(
            Path::new("/bin/sh"),
            &args,
            OsStr::new("/usr/bin"),
            &detail,
            parse,
        )
        .expect_err("fails");
        assert!(error.contains("boom") && error.contains('3'), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn the_program_runs_with_the_search_as_its_path() {
        let args = ["-c".to_string(), "echo \"$PATH\" >&2; exit 1".to_string()];
        let parse = |_: &str, _: &str| Err("no report".into());
        let detail: Detail = Arc::new(Mutex::new(None));
        let error = run(
            Path::new("/bin/sh"),
            &args,
            OsStr::new("/my/tools:/usr/bin"),
            &detail,
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
        let detail: Detail = Arc::new(Mutex::new(None));
        let findings = run(&program, &args, &search.joined(), &detail, parse).expect("ran");
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
    fn elapsed_reads_as_seconds_then_minutes() {
        assert_eq!(format_elapsed(Duration::from_secs(45)), "45s");
        assert_eq!(format_elapsed(Duration::from_secs(60)), "1m 00s");
        assert_eq!(format_elapsed(Duration::from_secs(192)), "3m 12s");
    }

    #[test]
    fn job_reports_a_missing_tool_through_poll() {
        // `poll` before the thread finishes is `None`; eventually it's done.
        let root = PathBuf::from("/nonexistent-srcwranglr-root");
        let job = ToolJob::spawn(
            Tool::Cppcheck,
            root.clone(),
            root,
            None,
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

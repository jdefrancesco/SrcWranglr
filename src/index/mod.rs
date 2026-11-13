//! C/C++ project indexer (tree-sitter based).
//!
//! Pipeline: `discover` source files → `extract` per-file facts →
//! `resolve` names into a [`CodeGraph`]. [`job::IndexJob`] runs the whole
//! thing on a background thread for the UI.

mod comment_marks;
mod complexity;
mod discover;
mod extract;
pub mod job;
mod macro_strings;
mod placement;
mod resolve;
mod signature;
mod strings;
pub(crate) mod syntax;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::model::CodeGraph;
pub use comment_marks::MAX_MARKS_PER_FILE;

/// For each of `places` (a project-relative file and a 1-based line), the
/// innermost function holding it, if any. One sweep per file.
pub fn functions_holding_lines(
    graph: &crate::model::CodeGraph,
    places: &[(&str, u32)],
) -> Vec<Option<crate::model::SymbolId>> {
    let functions_in = placement::by_file(&graph.functions);
    let mut by_file: std::collections::HashMap<&str, Vec<usize>> = std::collections::HashMap::new();
    for (index, (file, _)) in places.iter().enumerate() {
        by_file.entry(file).or_default().push(index);
    }
    let mut holding = vec![None; places.len()];
    for (file, indices) in by_file {
        let functions = functions_in.get(file).map_or(&[][..], Vec::as_slice);
        let lines: Vec<u32> = indices.iter().map(|&i| places[i].1).collect();
        for (&index, function) in indices
            .iter()
            .zip(placement::functions_holding(&lines, functions))
        {
            holding[index] = function;
        }
    }
    holding
}
pub use strings::{MAX_STRINGS_PER_FILE, MAX_STRINGS_TOTAL};

pub use extract::Extractor;
pub use resolve::{build_graph, SourceFile};

/// The graph of `(path, contents)` sources, for tests.
#[cfg(test)]
pub fn graph_of_files(sources: &[(&str, &str)]) -> CodeGraph {
    let mut extractor = Extractor::new().expect("grammar");
    let files = sources
        .iter()
        .map(|(path, contents)| SourceFile {
            path: (*path).to_string(),
            contents: (*contents).to_string(),
            facts: extractor.extract(contents).expect("parse"),
        })
        .collect();
    build_graph(files)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexOptions {
    pub max_files: usize,
    /// Larger files (often generated or amalgamated sources) are skipped.
    pub max_file_bytes: u64,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            // Must exceed the largest tree people open. This was 20,000, but
            // the Linux kernel has ~65k .c/.h files, and discovery keeps only
            // the first `max_files` paths in sorted order, so most of the
            // kernel was silently left out (a "warnings" badge was the only
            // sign). 100,000 covers Linux with room to grow. A full Linux
            // index takes ~2 minutes and ~7.6 GB at peak on Apple silicon;
            // this limit guards against runaway trees, it is not a budget.
            max_files: 100_000,
            max_file_bytes: 2 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

#[derive(Debug)]
pub struct IndexOutcome {
    pub graph: CodeGraph,
    pub indexed_files: usize,
    /// Non-fatal problems (skipped files, etc.), already user-readable.
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub enum IndexError {
    NotADirectory(PathBuf),
    NoSourceFiles(PathBuf),
    Cancelled,
    /// The C++ grammar could not be loaded (version mismatch).
    Parser(String),
    /// The background worker died (panic) before reporting a result.
    WorkerCrashed,
    /// The background worker thread could not be started.
    Spawn(String),
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IndexError::NotADirectory(p) => write!(f, "{} is not a folder", p.display()),
            IndexError::NoSourceFiles(p) => {
                write!(f, "No C/C++ source files found in {}", p.display())
            }
            IndexError::Cancelled => write!(f, "Indexing was cancelled"),
            IndexError::Parser(e) => write!(f, "Could not load the C++ parser: {e}"),
            IndexError::WorkerCrashed => write!(f, "Indexing failed unexpectedly (worker crashed)"),
            IndexError::Spawn(e) => write!(f, "Could not start indexing: {e}"),
        }
    }
}

impl std::error::Error for IndexError {}

/// Indexes every C/C++ file under `root`. Checks `cancel` between files and
/// reports progress after each one.
pub fn index_project(
    root: &Path,
    options: IndexOptions,
    on_progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<IndexOutcome, IndexError> {
    let discovery = discover::discover_sources(root, options.max_files, cancel)?;
    if discovery.files.is_empty() {
        return Err(IndexError::NoSourceFiles(root.to_path_buf()));
    }
    let mut extractor = Extractor::new().map_err(IndexError::Parser)?;
    let mut warnings = discovery.warnings;
    let mut sources = Vec::with_capacity(discovery.files.len());
    let total = discovery.files.len();

    for (i, path) in discovery.files.iter().enumerate() {
        check_cancel(cancel)?;
        let display = relative_display(root, path);
        match read_source(path, options.max_file_bytes) {
            Ok(contents) => match extractor.extract(&contents) {
                Some(facts) => sources.push(SourceFile {
                    path: display,
                    contents,
                    facts,
                }),
                None => warnings.push(format!("Could not parse {display}; skipped.")),
            },
            Err(reason) => warnings.push(format!("Skipped {display}: {reason}")),
        }
        on_progress(Progress { done: i + 1, total });
    }

    // Re-checked around the final (potentially long) build so a late cancel
    // never replaces the user's current project.
    check_cancel(cancel)?;
    let indexed_files = sources.len();
    let graph = build_graph(sources);
    check_cancel(cancel)?;
    Ok(IndexOutcome {
        graph,
        indexed_files,
        warnings,
    })
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), IndexError> {
    if cancel.load(Ordering::Relaxed) {
        Err(IndexError::Cancelled)
    } else {
        Ok(())
    }
}

/// Project-relative path with `/` separators (falls back to the full path).
fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Reads a file as text. Invalid UTF-8 (e.g. Latin-1 comments) is replaced
/// rather than rejected, so such files still get indexed.
fn read_source(path: &Path, max_bytes: u64) -> Result<String, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > max_bytes {
        return Err(format!("file is {size} bytes (limit {max_bytes})"));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, contents).expect("write");
    }

    fn run(root: &Path, options: IndexOptions) -> Result<IndexOutcome, IndexError> {
        index_project(root, options, &mut |_| {}, &AtomicBool::new(false))
    }

    #[test]
    fn default_file_limit_fits_the_linux_kernel() {
        // Linux 7.x has ~65k .c/.h files. Discovery sorts then truncates, so a
        // smaller limit silently drops everything after `drivers/gpu/...`.
        const LINUX_SOURCE_FILES: usize = 65_100;
        assert!(IndexOptions::default().max_files > LINUX_SOURCE_FILES);
    }

    #[test]
    fn indexes_project_with_relative_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "include/shape.h",
            "class Shape { public: int sides; };\n",
        );
        write(
            dir.path(),
            "src/main.cpp",
            "int main() { Shape s; return s.sides; }\n",
        );

        let outcome = run(dir.path(), IndexOptions::default()).expect("index");
        assert_eq!(outcome.indexed_files, 2);
        let shape = outcome
            .graph
            .symbols
            .values()
            .find(|s| s.qualified_name == "Shape")
            .expect("Shape symbol");
        assert_eq!(shape.file, "include/shape.h");
        assert!(outcome.graph.file_contents.contains_key("src/main.cpp"));
        assert!(outcome.warnings.is_empty());
    }

    #[test]
    fn reports_progress_for_every_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..3 {
            write(dir.path(), &format!("f{i}.c"), "void f(void) {}\n");
        }
        let mut seen = Vec::new();
        index_project(
            dir.path(),
            IndexOptions::default(),
            &mut |p| seen.push(p),
            &AtomicBool::new(false),
        )
        .expect("index");
        assert_eq!(seen.last(), Some(&Progress { done: 3, total: 3 }));
        assert!(seen.windows(2).all(|w| w[0].done < w[1].done));
    }

    #[test]
    fn skips_oversized_and_tolerates_non_utf8_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "big.cpp", &"// filler\n".repeat(100));
        fs::write(
            dir.path().join("latin1.c"),
            b"void caf\xe9(void) {}\nvoid ok(void) {}\n",
        )
        .expect("write");
        let options = IndexOptions {
            max_file_bytes: 500,
            ..IndexOptions::default()
        };

        let outcome = run(dir.path(), options).expect("index");
        assert_eq!(outcome.indexed_files, 1);
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("big.cpp"));
        assert!(outcome
            .graph
            .symbols
            .values()
            .any(|s| s.qualified_name == "ok"));
    }

    #[test]
    fn fails_cleanly_on_empty_project() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "README.md", "# nothing here");
        assert!(matches!(
            run(dir.path(), IndexOptions::default()),
            Err(IndexError::NoSourceFiles(_))
        ));
    }

    #[test]
    fn honors_cancellation() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.c", "void a(void) {}\n");
        let cancel = AtomicBool::new(true);
        let result = index_project(dir.path(), IndexOptions::default(), &mut |_| {}, &cancel);
        assert!(matches!(result, Err(IndexError::Cancelled)));
    }

    /// Traces every risky call's arguments back; prints how many reach
    /// input, the slowest trace, and a few paths.
    fn print_traces(graph: &CodeGraph, findings: &[crate::analysis::Finding]) {
        use crate::analysis::trace::trace_finding;
        let start = std::time::Instant::now();
        let mut slowest = (std::time::Duration::ZERO, String::new());
        let mut traces = Vec::new();
        for finding in findings {
            let one = std::time::Instant::now();
            let trace = trace_finding(graph, finding);
            if one.elapsed() > slowest.0 {
                slowest = (one.elapsed(), format!("{}:{}", finding.file, finding.line));
            }
            traces.extend(trace);
        }
        let from_input: Vec<_> = traces
            .iter()
            .filter(|t| t.paths.iter().any(|p| p.endpoint.is_input()))
            .collect();
        let cut_short = traces.iter().filter(|t| t.is_cut_short).count();
        println!(
            "traced {} of {} risky calls in {:?} (slowest {:?} at {}); {} reach input, {} cut short",
            traces.len(),
            findings.len(),
            start.elapsed(),
            slowest.0,
            slowest.1,
            from_input.len(),
            cut_short
        );
        // SRCWRANGLR_TRACES=all prints every one, not just two.
        let shown = match std::env::var("SRCWRANGLR_TRACES").as_deref() {
            Ok("all") => from_input.len(),
            _ => 2,
        };
        for trace in from_input.iter().take(shown) {
            let path = trace
                .paths
                .iter()
                .find(|p| p.endpoint.is_input())
                .expect("one");
            println!(
                "  {} at {}:{} {}",
                trace.subject,
                trace.file,
                trace.line,
                path.endpoint.label()
            );
            for step in &path.steps {
                println!(
                    "      {}:{}  {}  — {}",
                    step.file, step.line, step.code, step.note
                );
            }
        }
    }

    /// Traces every `main`'s `argv` forward to the risky calls it reaches;
    /// prints how many it reaches, the slowest trace, and the paths.
    fn print_forward_traces(graph: &CodeGraph, findings: &[crate::analysis::Finding]) {
        use crate::analysis::trace::{trace_forward, Sink};
        let sinks: Vec<Sink> = findings.iter().map(Sink::of).collect();
        let start = std::time::Instant::now();
        let mut slowest = std::time::Duration::ZERO;
        let mains = graph.functions.iter().filter(|f| {
            graph
                .symbols
                .get(&f.symbol)
                .is_some_and(|s| s.name == "main")
        });
        for main in mains {
            let source = &graph.file_contents[&main.file];
            let from_line: usize = source
                .split('\n')
                .take(main.line.saturating_sub(1) as usize)
                .map(|l| l.len() + 1)
                .sum();
            let Some(found) = source[from_line..].find("argv") else {
                continue;
            };
            let at = from_line + found;
            let line = source[..at].matches('\n').count() as u32 + 1;
            let column = (at - source[..at].rfind('\n').map_or(0, |i| i + 1)) as u32;
            let one = std::time::Instant::now();
            let Some(trace) = trace_forward(graph, &sinks, &main.file, line, column) else {
                continue;
            };
            slowest = slowest.max(one.elapsed());
            let to_sinks: Vec<_> = trace
                .paths
                .iter()
                .filter(|p| p.endpoint.is_sink())
                .collect();
            println!(
                "forward from {}:{} argv ({:?}): {} paths to risky calls, {} stop{}",
                main.file,
                line,
                one.elapsed(),
                to_sinks.len(),
                trace.paths.len() - to_sinks.len(),
                if trace.is_cut_short {
                    ", cut short"
                } else {
                    ""
                }
            );
            if std::env::var("SRCWRANGLR_STOPS").is_ok() {
                for path in trace.paths.iter().filter(|p| !p.endpoint.is_sink()).take(8) {
                    let last = path.steps.last().map_or(0, |s| s.line);
                    println!("    STOP at {last}: {}", path.endpoint.label());
                    for step in path.steps.iter().rev().take(3).rev() {
                        println!(
                            "        {}:{} {} — {}",
                            step.file, step.line, step.code, step.note
                        );
                    }
                }
            }
            for path in to_sinks.iter().take(8) {
                println!("    {}", path.endpoint.label());
                for step in &path.steps {
                    println!(
                        "      {}:{}  {}  — {}",
                        step.file, step.line, step.code, step.note
                    );
                }
            }
        }
        println!(
            "forward traces took {:?} (slowest {slowest:?})",
            start.elapsed()
        );
    }

    /// Manual smoke test against a real project:
    /// `SRCWRANGLR_INDEX_PATH=/path/to/project cargo test --release index_real_project -- --ignored --nocapture`
    /// (add `SRCWRANGLR_HARNESS_OUT=/some/dir` to write the best fuzz targets' harnesses there)
    #[test]
    #[ignore = "needs SRCWRANGLR_INDEX_PATH"]
    fn index_real_project() {
        let Some(root) = std::env::var_os("SRCWRANGLR_INDEX_PATH") else {
            return;
        };
        let start = std::time::Instant::now();
        let outcome = run(Path::new(&root), IndexOptions::default()).expect("index");
        let graph = &outcome.graph;
        println!(
            "indexed {} files in {:?}: {} symbols, {} edges, {} occurrences, {} warnings",
            outcome.indexed_files,
            start.elapsed(),
            graph.symbols.len(),
            graph.edges.len(),
            graph.occurrences.values().map(Vec::len).sum::<usize>(),
            outcome.warnings.len()
        );
        let mut by_kind: std::collections::BTreeMap<&str, usize> = Default::default();
        for s in graph.symbols.values() {
            *by_kind.entry(s.kind.label()).or_default() += 1;
        }
        println!("symbols by kind: {by_kind:?}");
        let mut edges_by_kind: std::collections::BTreeMap<String, usize> = Default::default();
        for e in &graph.edges {
            *edges_by_kind.entry(format!("{:?}", e.kind)).or_default() += 1;
        }
        println!("edges by kind: {edges_by_kind:?}");
        let external: usize = graph.external_calls.values().map(Vec::len).sum();
        let in_tree: usize = graph.library_calls_in_tree.values().map(Vec::len).sum();
        let mut risky_by_rule: std::collections::BTreeMap<String, usize> = Default::default();
        let judging = std::time::Instant::now();
        let risky = crate::analysis::risky::risky_calls(graph, &Default::default());
        let judged_in = judging.elapsed();
        for finding in &risky {
            *risky_by_rule.entry(finding.rule.clone()).or_default() += 1;
        }
        println!(
            "external calls: {external}; to the project's own library functions: {in_tree}; \
             {} risky calls in {judged_in:?}, by rule: {risky_by_rule:?}",
            risky.len()
        );
        let looking = std::time::Instant::now();
        let stop = std::sync::atomic::AtomicBool::new(false);
        let wrappers = crate::analysis::wrappers::wrappers(graph, &stop).expect("not stopped");
        println!(
            "{} wrappers of risky calls in {:?}; most called: {:?}",
            wrappers.len(),
            looking.elapsed(),
            wrappers
                .iter()
                .take(8)
                .map(|w| (w.name.as_str(), w.callee.as_str(), w.passes, w.callers))
                .collect::<Vec<_>>()
        );
        for found in crate::analysis::capabilities::capabilities_in(graph) {
            let functions: Vec<(&str, usize)> = found
                .functions
                .iter()
                .map(|f| (f.name.as_str(), f.calls.len()))
                .collect();
            println!("capability {}: {functions:?}", found.category.label());
        }
        let findings = crate::analysis::risky::risky_calls(graph, &Default::default());
        let reach = crate::analysis::reach::Reach::of(graph);
        let reached: Vec<_> = findings
            .iter()
            .filter(|f| reach.reaches_finding(f))
            .collect();
        println!(
            "input reaches {} functions from {:?}; {} of {} risky calls are in them",
            reach.len(),
            reach.sources(),
            reached.len(),
            findings.len()
        );
        for finding in reached.iter().take(5) {
            if let Some(path) = reach.finding_path(finding) {
                println!(
                    "  {} {}:{} <- {}",
                    finding.rule,
                    finding.file,
                    finding.line,
                    crate::analysis::reach::describe(&path, graph)
                );
            }
        }
        print_traces(graph, &findings);
        print_forward_traces(graph, &findings);
        let scoring = std::time::Instant::now();
        let targets = crate::analysis::fuzz::fuzz_targets(graph, &findings);
        println!(
            "{} fuzz targets in {:?}; the best:",
            targets.len(),
            scoring.elapsed()
        );
        let harness_dir = std::env::var_os("SRCWRANGLR_HARNESS_OUT").map(std::path::PathBuf::from);
        for target in targets.iter().take(25) {
            let definition = graph.functions.iter().find(|f| {
                f.symbol == target.symbol && f.file == target.file && f.line == target.line
            });
            use crate::analysis::fuzz::{dictionary, harness, makefile};
            let harnesses = harness::Engine::ALL
                .iter()
                .filter_map(|engine| definition.and_then(|f| harness::draft(graph, f, *engine)));
            let dictionary = definition.and_then(|f| dictionary::draft(graph, f));
            let has_dictionary = dictionary.is_some();
            let makefile = definition.and_then(|f| makefile::draft(graph, f, has_dictionary));
            let drafted = harnesses
                .map(|h| (h.file_name, h.code))
                .chain(dictionary.map(|d| (d.file_name, d.text)))
                .chain(makefile.map(|m| (m.file_name, m.text)));
            for (file_name, text) in drafted {
                let Some(dir) = &harness_dir else { break };
                std::fs::write(dir.join(file_name), text).expect("write harness");
            }
            let reasons: Vec<String> = target
                .reasons
                .iter()
                .map(|r| format!("{:+} {}", r.points, r.text))
                .collect();
            println!(
                "  {:>3} {:<8} {} ({}:{}): {}",
                target.score,
                target.ease.label(),
                target.name,
                target.file,
                target.line,
                reasons.join("; ")
            );
        }
        let name = |id| {
            graph
                .symbols
                .get(&id)
                .map_or("?", |s| s.qualified_name.as_str())
        };
        let calls = graph
            .edges
            .iter()
            .filter(|e| e.kind == crate::model::EdgeKind::Calls);
        for e in calls.take(12) {
            println!("  {} -> {}", name(e.from), name(e.to));
        }
        outcome
            .warnings
            .iter()
            .take(5)
            .for_each(|w| println!("warning: {w}"));
    }

    #[test]
    fn cancel_after_last_file_still_cancels() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.c", "void a(void) {}\n");
        let cancel = AtomicBool::new(false);
        let mut cancel_at_end = |p: Progress| {
            if p.done == p.total {
                cancel.store(true, Ordering::Relaxed);
            }
        };
        let result = index_project(
            dir.path(),
            IndexOptions::default(),
            &mut cancel_at_end,
            &cancel,
        );
        assert!(matches!(result, Err(IndexError::Cancelled)));
    }

    #[test]
    fn errors_have_user_facing_messages() {
        let msg = IndexError::NoSourceFiles(PathBuf::from("/tmp/x")).to_string();
        assert!(msg.contains("/tmp/x"));
    }
}

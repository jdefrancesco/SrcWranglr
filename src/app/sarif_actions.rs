//! Importing a SARIF report (CodeQL's, or any tool's) into Findings: the
//! file is picked in a dialog, read and parsed off the window's thread,
//! then each of its tools' results replaces that tool's earlier ones.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::background::BackgroundJob;
use super::dialogs::{self, DialogState, PendingDialog};
use super::tool_job;
use super::{SrcWranglrApp, Status};
use crate::analysis::sarif::{self, Imported, MAX_SARIF_BYTES};
use crate::model::CodeGraph;

/// Tools named in the status line, at most.
const MAX_TOOLS_SAID: usize = 5;

/// Where importing a report is.
pub enum SarifImport {
    Picking(PendingDialog<PathBuf>),
    Reading(BackgroundJob<Imported>),
}

impl SrcWranglrApp {
    pub(super) fn import_sarif_dialog(&mut self, ctx: &egui::Context) {
        if !self.is_project_open || self.project.root.is_none() {
            self.status = Status::Info(
                "Open the project the report is about first, then import it into it.".into(),
            );
            return;
        }
        let dialog = dialogs::spawn(ctx, || {
            rfd::FileDialog::new()
                .set_title("Import SARIF Results")
                .add_filter("SARIF report", &["sarif", "json"])
                .pick_file()
        });
        match dialog {
            Ok(dialog) => self.sarif_import = Some(SarifImport::Picking(dialog)),
            Err(error) => self.status = Status::Error(error),
        }
    }

    pub(super) fn poll_sarif_import(&mut self, ctx: &egui::Context) {
        match &self.sarif_import {
            Some(SarifImport::Picking(dialog)) => match dialog.poll() {
                DialogState::Open => {}
                DialogState::Picked(path) => self.read_sarif(path, ctx),
                DialogState::Cancelled => self.sarif_import = None,
                DialogState::Failed(error) => {
                    self.sarif_import = None;
                    self.status = Status::Error(format!("Could not open the dialog: {error}"));
                }
            },
            Some(SarifImport::Reading(job)) => {
                if let Some(result) = job.poll() {
                    self.sarif_import = None;
                    self.take_sarif(result);
                }
            }
            None => {}
        }
    }

    /// Reads and parses the report at `path` in the background.
    fn read_sarif(&mut self, path: PathBuf, ctx: &egui::Context) {
        let Some(root) = self.project.root.clone() else {
            self.sarif_import = None;
            return;
        };
        let graph = Arc::clone(&self.project.graph);
        self.status = Status::Info(format!("Reading {}…", path.display()));
        let job = BackgroundJob::spawn(
            "Reading the SARIF report",
            move |_| read_report(&path, &root, &graph),
            ctx.clone(),
        );
        self.sarif_import = Some(SarifImport::Reading(job));
    }

    /// Puts what a report held (its findings' functions found) in
    /// Findings.
    pub(super) fn take_sarif(&mut self, result: Result<Imported, String>) {
        let imported = match result {
            Ok(imported) => imported,
            Err(error) => return self.status = Status::Error(error),
        };
        let mut said: Vec<String> = imported
            .tools
            .iter()
            .take(MAX_TOOLS_SAID)
            .map(|(tool, found)| format!("{tool}: {} finding(s)", found.len()))
            .collect();
        if imported.tools.len() > MAX_TOOLS_SAID {
            said.push(format!(
                "{} more tools",
                imported.tools.len() - MAX_TOOLS_SAID
            ));
        }
        let Imported {
            tools,
            outside,
            placeless,
            is_cut_short,
        } = imported;
        self.findings = self.findings.with_results_of(tools);
        self.refresh_verdicts();
        let imported = Imported {
            outside,
            placeless,
            is_cut_short,
            ..Imported::default()
        };
        if imported.outside > 0 {
            said.push(format!(
                "{} about files outside this project left out",
                imported.outside
            ));
        }
        if imported.placeless > 0 {
            said.push(format!(
                "{} with no place in a file left out",
                imported.placeless
            ));
        }
        if imported.is_cut_short {
            said.push(format!(
                "some left out (over {} in all, or {} on one line)",
                sarif::MAX_RESULTS,
                sarif::MAX_RESULTS_PER_LINE
            ));
        }
        self.status = Status::Info(format!("Imported {}. See Findings (⇧⌘R).", said.join("; ")));
    }
}

fn read_report(path: &Path, root: &Path, graph: &CodeGraph) -> Result<Imported, String> {
    let shown = path.display();
    let size = std::fs::metadata(path)
        .map_err(|e| format!("Could not read {shown}: {e}"))?
        .len();
    if size > MAX_SARIF_BYTES {
        return Err(format!(
            "{shown} is larger than {} MB; SrcWranglr doesn't read reports that big.",
            MAX_SARIF_BYTES / (1024 * 1024)
        ));
    }
    let json = std::fs::read_to_string(path).map_err(|e| format!("Could not read {shown}: {e}"))?;
    let imported = sarif::parse(&json, root, |file| {
        graph.file_contents.get(file).map(String::as_str)
    })
    .map_err(|e| format!("{shown}: {e}"))?;
    // Each finding's function, found here rather than on the window's thread.
    let tools = imported
        .tools
        .into_iter()
        .map(|(tool, found)| (tool, tool_job::with_symbols(found, graph)))
        .collect();
    Ok(Imported { tools, ..imported })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::launch::LaunchOptions;
    use crate::app::project::Project;
    use crate::index::graph_of_files;

    #[test]
    fn a_reports_results_go_to_findings_and_say_what_was_left_out() {
        let dir = tempfile::tempdir().expect("dir");
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let graph = graph_of_files(&[("a.c", "void f(char *b) {\n  gets(b);\n}\n")]);
        app.project = Project::new("p", Some(dir.path().to_path_buf()), graph);
        let report = r#"{"runs": [{"tool": {"driver": {"name": "CodeQL"}}, "results": [
            {"ruleId": "cpp/dangerous-function-overflow", "level": "error",
             "message": {"text": "gets"},
             "locations": [{"physicalLocation": {"artifactLocation": {"uri": "a.c"},
               "region": {"startLine": 2}}}]},
            {"ruleId": "r", "message": {"text": "x"},
             "locations": [{"physicalLocation": {"artifactLocation": {"uri": "b.c"}}}]}]}]}"#;
        let path = dir.path().join("codeql.sarif");
        std::fs::write(&path, report).expect("write");
        app.take_sarif(read_report(&path, dir.path(), &app.project.graph.clone()));
        let imported: Vec<_> = app
            .findings
            .all()
            .iter()
            .filter(|f| f.tool == "CodeQL")
            .collect();
        assert_eq!(imported.len(), 1);
        assert_eq!((imported[0].line, imported[0].symbol.is_some()), (2, true));
        let Status::Info(said) = &app.status else {
            panic!("not imported");
        };
        assert!(said.contains("CodeQL: 1 finding(s)"), "{said}");
        assert!(
            said.contains("1 about files outside this project"),
            "{said}"
        );
    }
}

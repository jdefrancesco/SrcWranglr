//! What the app does with findings: explaining one, running external
//! analyzers in the background, and folding their results in.

use super::dangerous_editor::{DangerousEditor, DangerousEditorOutcome};
use super::dangerous_lists::{DangerousLists, ListScope};
use super::finding_view::{FindingView, FindingViewAction};
use super::tool_job::{self, ToolJob};
use super::trust;
use super::{SlurcetrailApp, Status};
use crate::analysis;

impl SlurcetrailApp {
    /// Opens the editor for `name`'s dangerous-list entry.
    pub(super) fn mark_dangerous(&mut self, name: &str) {
        let existing = self.dangerous.merged().lookup(name).cloned();
        let can_use_project = self.dangerous.has_project_list();
        self.dangerous_editor = Some(DangerousEditor::new(
            name,
            existing.as_ref(),
            can_use_project,
        ));
    }

    /// Draws the list editor if open; saving re-flags the code.
    pub(super) fn show_dangerous_editor(&mut self, ctx: &egui::Context) {
        let Some(outcome) = self.dangerous_editor.as_mut().and_then(|e| e.show(ctx)) else {
            return;
        };
        let DangerousEditorOutcome::Save(entry, scope) = outcome else {
            self.dangerous_editor = None;
            return;
        };
        match self.dangerous.add(&entry, scope) {
            Ok(path) => {
                if scope == ListScope::Project {
                    self.trust_project_files_we_wrote();
                }
                self.dangerous_editor = None;
                self.findings = self
                    .findings
                    .with_own_results(&self.project.graph, self.dangerous.merged());
                self.refresh_verdicts();
                let in_effect = self.dangerous.merged().lookup(&entry.name);
                let overridden = scope == ListScope::Global
                    && in_effect.is_some_and(|e| e.reason != entry.reason);
                self.status = Status::Info(if overridden {
                    format!(
                        "Saved {} in {}, but this project's own list overrides it here.",
                        entry.name,
                        path.display()
                    )
                } else {
                    format!(
                        "Saved {} in {}; findings updated.",
                        entry.name,
                        path.display()
                    )
                });
            }
            Err(error) => {
                if let Some(editor) = self.dangerous_editor.as_mut() {
                    editor.show_error(error);
                }
            }
        }
    }

    /// Trusts this project's own verdicts and list, and applies them.
    pub(super) fn trust_project_files(&mut self) {
        let Some(root) = self.project.root.clone() else {
            self.status = Status::Info("The demo has no project files to trust.".into());
            return;
        };
        if let Err(error) = trust::trust(&root) {
            self.status = Status::Error(error);
            return;
        }
        self.are_project_files_trusted = true;
        self.dangerous = DangerousLists::load(Some(&root), true);
        self.findings = self
            .findings
            .with_own_results(&self.project.graph, self.dangerous.merged());
        self.refresh_verdicts();
        self.status = Status::Info("This project's verdicts and list are now applied.".into());
    }

    /// After writing a verdict or list entry into a project whose files are
    /// all yours, remembers that it's trusted (so it stays so once its
    /// files exist).
    pub(super) fn trust_project_files_we_wrote(&mut self) {
        let Some(root) = self.project.root.as_ref() else {
            return;
        };
        if self.are_project_files_trusted && !trust::is_trusted(root) {
            if let Err(error) = trust::trust(root) {
                eprintln!("could not remember this project as trusted: {error}");
            }
        }
    }

    /// Opens the list editor for the selected function, if one is selected.
    pub(super) fn mark_selected_dangerous(&mut self) {
        let name = self
            .graph_view
            .selected()
            .and_then(|id| self.project.graph.symbols.get(&id))
            .map(|s| s.qualified_name.clone());
        match name {
            Some(name) => self.mark_dangerous(&name),
            None => self.status = Status::Info("Select a function first.".into()),
        }
    }

    /// Adds the number of risky calls to the status.
    pub(super) fn report_findings(&mut self) {
        if let Some(problem) = self.dangerous.problems().first() {
            self.status = Status::Error(format!("Dangerous-functions list skipped: {problem}"));
            return;
        }
        if !self.are_project_files_trusted {
            let ignored = match self.dangerous.ignored_project_offs() {
                0 => String::new(),
                n => format!(" ({n} \"off\" entries ignored)"),
            };
            self.status = Status::Info(format!(
                "This project ships its own verdicts or dangerous list{ignored}: shown, \
                 not applied. Analyze → Trust This Project's Verdicts and List to apply them."
            ));
            return;
        }
        let total = self.findings.all().len();
        if let (Status::Info(text), true) = (&mut self.status, total > 0) {
            let high = self.findings.count(analysis::Severity::High);
            text.push_str(&format!(
                " {total} risky call(s), {high} high: Findings (⇧⌘R)."
            ));
        }
    }

    /// Opens the explanation of the finding at `id` in the findings.
    pub(super) fn explain_finding(&mut self, id: usize) {
        let Some(finding) = self.findings.all().get(id).cloned() else {
            return;
        };
        let function = finding
            .symbol
            .and_then(|s| self.project.graph.symbols.get(&s))
            .map(|s| s.qualified_name.clone());
        let verdict = self.findings.verdict_and_note(id);
        self.finding_view = Some(FindingView::new(finding, function, verdict));
    }

    pub(super) fn show_finding_view(&mut self, ctx: &egui::Context) {
        let Some(action) = self.finding_view.as_mut().and_then(|v| v.show(ctx)) else {
            return;
        };
        let view = self.finding_view.take();
        match action {
            FindingViewAction::Close => {}
            FindingViewAction::Explore(id) => self.graph_view.select(id),
            FindingViewAction::MarkDangerous(name) => self.mark_dangerous(&name),
            FindingViewAction::Triage(status, note) => {
                if let Some(finding) = view.as_ref().map(|v| v.finding().clone()) {
                    self.apply_verdict(&finding, status, note);
                }
            }
            FindingViewAction::OpenInEditor => {
                if let Some(f) = view.as_ref().map(FindingView::finding) {
                    let (symbol, file, line) = (f.symbol, f.file.clone(), f.line);
                    self.go_to_place(symbol, &file, line);
                    self.open_in_editor();
                }
            }
        }
    }

    /// Starts `tool` on the project in the background.
    pub(super) fn run_tool(&mut self, tool: analysis::tools::Tool, ctx: &egui::Context) {
        let Some(root) = self.project.root.clone() else {
            self.status = Status::Info(format!(
                "Open a project (⌘O) to run {} on it; the demo isn't on disk.",
                tool.name()
            ));
            return;
        };
        if self.tool_jobs.iter().any(|job| job.tool() == tool) {
            self.status = Status::Info(format!("{} is already running.", tool.name()));
            return;
        }
        self.status = Status::Info(format!(
            "Running {} on {}… results go to Findings.",
            tool.name(),
            self.project.name
        ));
        self.tool_jobs.push(ToolJob::spawn(tool, root, ctx.clone()));
    }

    /// Adds finished tools' findings.
    pub(super) fn poll_tool_jobs(&mut self) {
        let finished: Vec<(
            analysis::tools::Tool,
            Result<Vec<analysis::Finding>, String>,
        )> = self
            .tool_jobs
            .iter()
            .filter_map(|job| job.poll().map(|result| (job.tool(), result)))
            .collect();
        for (tool, result) in finished {
            self.tool_jobs.retain(|job| job.tool() != tool);
            self.status = match result {
                Ok(found) => {
                    let count = found.len();
                    let found = tool_job::with_symbols(found, &self.project.graph);
                    self.findings = self.findings.with_tool_results(tool.name(), found);
                    self.refresh_verdicts();
                    Status::Info(format!(
                        "{}: {count} finding(s), in Findings (⇧⌘R).",
                        tool.name()
                    ))
                }
                Err(error) => Status::Error(error),
            };
        }
    }
}

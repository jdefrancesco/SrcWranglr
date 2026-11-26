//! What the app does with findings: explaining one, running external
//! analyzers in the background, and folding their results in.

use std::path::PathBuf;

use super::dangerous_editor::{DangerousEditor, DangerousEditorOutcome};
use super::dangerous_lists::{DangerousLists, ListScope};
use super::dialogs::{self, DialogState, PendingDialog};
use super::finding_view::{FindingView, FindingViewAction};
use super::rule_files;
use super::tool_job::{self, ToolJob};
use super::trust;
use super::{SrcWranglrApp, Status};
use crate::analysis;
use crate::analysis::reach::describe;
use crate::analysis::tools::Tool;

/// A subfolder being picked for a scoped tool run.
pub(super) struct ToolFolderPick {
    tool: Tool,
    dialog: PendingDialog<PathBuf>,
}

impl SrcWranglrApp {
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
        let (entry, scope) = match outcome {
            DangerousEditorOutcome::Save(entry, scope) => (entry, scope),
            DangerousEditorOutcome::Remove(name) => {
                self.unmark_dangerous(&name);
                return;
            }
            DangerousEditorOutcome::Cancel => {
                self.dangerous_editor = None;
                return;
            }
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

    /// Unmarks `name`: removes its entry from the list(s), then re-flags.
    fn unmark_dangerous(&mut self, name: &str) {
        match self.dangerous.remove(name) {
            Ok(changed) if changed.is_empty() => {
                self.dangerous_editor = None;
                self.status = Status::Info(format!("{name} wasn't in your lists."));
            }
            Ok(changed) => {
                self.dangerous_editor = None;
                self.findings = self
                    .findings
                    .with_own_results(&self.project.graph, self.dangerous.merged());
                self.refresh_verdicts();
                let files = changed
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.status = Status::Info(format!(
                    "Unmarked {name} (removed from {files}); findings updated."
                ));
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
        self.status = Status::Info(
            "This project's verdicts and list are now applied, and semgrep runs use its rules."
                .into(),
        );
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
                "This project ships its own verdicts, dangerous list or semgrep rules{ignored}: \
                 not applied. Analyze > Trust This Project's Verdicts, List and Rules to apply \
                 them."
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
        let input = self
            .reach
            .finding_path(&finding)
            .map(|path| describe(&path, &self.project.graph));
        // For a use-after-free or double-free, show the pointer lifetimes
        // (the invariant matrix) in the function that led to it.
        let lifetimes = matches!(finding.rule.as_str(), "use-after-free" | "double-free")
            .then(|| {
                crate::analysis::memory::lifetimes(&self.project.graph, &finding.file, finding.line)
            })
            .flatten();
        self.finding_view =
            Some(FindingView::new(finding, function, input, verdict).with_lifetimes(lifetimes));
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
            FindingViewAction::Trace => {
                if let Some(finding) = view.as_ref().map(|v| v.finding().clone()) {
                    self.trace_finding(&finding);
                }
            }
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
            FindingViewAction::GoToLine(line) => {
                if let Some(f) = view.as_ref().map(FindingView::finding) {
                    let (symbol, file) = (f.symbol, f.file.clone());
                    self.go_to_place(symbol, &file, line);
                }
            }
        }
    }

    /// Starts `tool` on the project in the background.
    pub(super) fn run_tool(&mut self, tool: Tool, ctx: &egui::Context) {
        let Some(root) = self.project.root.clone() else {
            self.status = Status::Info(format!(
                "Open a project (⌘O) to run {} on it; the demo isn't on disk.",
                tool.name()
            ));
            return;
        };
        self.start_tool(tool, root.clone(), root, None, ctx);
    }

    /// Runs every external analyzer (semgrep, cppcheck, flawfinder) on the
    /// whole project at once; each streams into Findings as it finishes, and
    /// any already running (or not installed) is handled by `start_tool`.
    pub(super) fn run_all_analyzers(&mut self, ctx: &egui::Context) {
        let Some(root) = self.project.root.clone() else {
            self.status = Status::Info(
                "Open a project (⌘O) to run the analyzers on it; the demo isn't on disk.".into(),
            );
            return;
        };
        for tool in [Tool::Semgrep, Tool::Cppcheck, Tool::Flawfinder] {
            self.start_tool(tool, root.clone(), root.clone(), None, ctx);
        }
        let running: Vec<&str> = self.tool_jobs.iter().map(|job| job.tool().name()).collect();
        if !running.is_empty() {
            self.status = Status::Info(format!(
                "Running {}… results go to Findings (⇧⌘R).",
                running.join(", ")
            ));
        }
    }

    /// Asks for a subfolder, then runs `tool` on just that folder.
    pub(super) fn run_tool_on_folder(&mut self, tool: Tool, ctx: &egui::Context) {
        let Some(root) = self.project.root.clone() else {
            self.status = Status::Info(format!(
                "Open a project (⌘O) to run {} on a folder of it.",
                tool.name()
            ));
            return;
        };
        if self.tool_jobs.iter().any(|job| job.tool() == tool) {
            self.status = Status::Info(format!("{} is already running.", tool.name()));
            return;
        }
        let start = root;
        let picked = dialogs::spawn(ctx, move || {
            rfd::FileDialog::new()
                .set_title("Run on which folder")
                .set_directory(&start)
                .pick_folder()
        });
        match picked {
            Ok(dialog) => self.tool_folder = Some(ToolFolderPick { tool, dialog }),
            Err(error) => self.status = Status::Error(error),
        }
    }

    /// Picks up the folder for a scoped run once the dialog answers.
    pub(super) fn poll_tool_folder(&mut self, ctx: &egui::Context) {
        let Some(pick) = self.tool_folder.as_ref() else {
            return;
        };
        let tool = pick.tool;
        let state = pick.dialog.poll();
        if state == DialogState::Open {
            return;
        }
        self.tool_folder = None;
        match state {
            DialogState::Picked(folder) => self.run_tool_on(tool, folder, ctx),
            DialogState::Failed(error) => self.status = Status::Error(error),
            DialogState::Open | DialogState::Cancelled => {}
        }
    }

    /// Runs `tool` on `folder` (inside the project); findings stay relative to
    /// the project root so they line up with the index and source view.
    fn run_tool_on(&mut self, tool: Tool, folder: PathBuf, ctx: &egui::Context) {
        let Some(root) = self.project.root.clone() else {
            return;
        };
        let scope = folder
            .strip_prefix(&root)
            .ok()
            .map(|rel| rel.to_string_lossy().into_owned())
            .filter(|rel| !rel.is_empty())
            .or_else(|| folder.file_name().map(|n| n.to_string_lossy().into_owned()));
        self.start_tool(tool, root, folder, scope, ctx);
    }

    /// Spawns `tool` on `target` (relativised to `root`); `scope` names the
    /// subfolder for the status line, or `None` for the whole project.
    fn start_tool(
        &mut self,
        tool: Tool,
        root: PathBuf,
        target: PathBuf,
        scope: Option<String>,
        ctx: &egui::Context,
    ) {
        if self.tool_jobs.iter().any(|job| job.tool() == tool) {
            self.status = Status::Info(format!("{} is already running.", tool.name()));
            return;
        }
        let where_ = scope.as_deref().unwrap_or(self.project.name.as_str());
        self.status = Status::Info(format!(
            "Running {} on {where_}… results go to Findings.",
            tool.name()
        ));
        super::diag::record(
            super::diag::Level::Info,
            format!("{} started on {where_}", tool.name()),
        );
        let search = self.settings.search_path();
        // The project's own rules only once you trust its files, asked
        // now: rules may have appeared since the project was opened.
        let project_rules = trust::is_trusted(&root)
            .then(|| rule_files::project_rules(&root))
            .flatten();
        self.tool_jobs.push(ToolJob::spawn(
            tool,
            root,
            target,
            scope,
            search,
            project_rules,
            ctx.clone(),
        ));
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
                    super::diag::record(
                        super::diag::Level::Info,
                        format!("{} finished: {count} finding(s)", tool.name()),
                    );
                    let found = tool_job::with_symbols(found, &self.project.graph);
                    self.findings = self.findings.with_tool_results(tool.name(), found);
                    self.refresh_verdicts();
                    Status::Info(format!(
                        "{}: {count} finding(s), in Findings (⇧⌘R).",
                        tool.name()
                    ))
                }
                // A failed run's error is mirrored to the log by
                // `mirror_status_to_log`, so it isn't recorded twice here.
                Err(error) => Status::Error(error),
            };
        }
    }
}

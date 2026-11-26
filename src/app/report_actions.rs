//! What the app does for File › Generate Report…: builds the open
//! project's audit as Markdown and saves it where the user picks. The
//! report itself is built in [`super::report`].

use std::path::PathBuf;

use super::dangerous_lists::write_user_file;
use super::dialogs::{self, DialogState, PendingDialog};
use super::report::{self, Report};
use super::{SrcWranglrApp, Status};

/// A report "save as" dialog that is open, with the Markdown to write.
pub struct ReportDialog {
    dialog: PendingDialog<PathBuf>,
    text: String,
}

impl SrcWranglrApp {
    /// Asks where to save the open project's audit report.
    pub(super) fn generate_report_dialog(&mut self, ctx: &egui::Context) {
        if self.project.root.is_none() || !self.is_project_open {
            self.status = Status::Info("Open a project to generate its report.".into());
            return;
        }
        let text = self.audit_report().markdown();
        let name = report::file_name(&self.project.name);
        let dialog = dialogs::spawn(ctx, move || {
            rfd::FileDialog::new()
                .set_title("Generate Report")
                .set_file_name(name)
                .add_filter("Markdown", &["md"])
                .save_file()
        });
        match dialog {
            Ok(dialog) => self.report_dialog = Some(ReportDialog { dialog, text }),
            Err(error) => self.status = Status::Error(error),
        }
    }

    /// The open project's audit, gathered for the report.
    fn audit_report(&self) -> Report<'_> {
        Report {
            project: &self.project.name,
            generated: now(),
            findings_total: self.findings.all().len(),
            notes: self.notebook.notes().iter().collect(),
            todos: self.todos.items(),
            verdicts: self.triage.entries(),
            journal: self.journal.entries(),
        }
    }

    pub(super) fn poll_report_dialog(&mut self) {
        let Some(ReportDialog { dialog, .. }) = &self.report_dialog else {
            return;
        };
        let state = dialog.poll();
        if state == DialogState::Open {
            return;
        }
        let Some(ReportDialog { text, .. }) = self.report_dialog.take() else {
            return;
        };
        match state {
            DialogState::Picked(path) => {
                self.status = match write_user_file(&path, &text) {
                    Ok(saved) => Status::Info(format!("Report written to {}.", saved.display())),
                    Err(e) => Status::Error(format!("Could not save {}: {e}", path.display())),
                };
            }
            DialogState::Failed(error) => {
                self.status = Status::Error(format!("Could not open the dialog: {error}"));
            }
            DialogState::Open | DialogState::Cancelled => {}
        }
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::launch::LaunchOptions;
    use crate::app::notebook::Notebook;
    use crate::app::project::Project;
    use crate::index::graph_of_files;
    use crate::notes::triage::{TriageEntry, TriageStatus};
    use crate::notes::NoteTarget;

    const SOURCE: &str = "void f(char *d, char *s) {\nstrcpy(d, s);\n}\n";

    fn app_in(dir: &std::path::Path) -> SrcWranglrApp {
        std::fs::write(dir.join("a.c"), SOURCE).expect("source");
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let graph = graph_of_files(&[("a.c", SOURCE)]);
        app.project = Project::new("proj", Some(dir.to_path_buf()), graph);
        app.notebook = Notebook::open(&app.project);
        app.is_project_open = true;
        app.are_project_files_trusted = false;
        app
    }

    #[test]
    fn the_report_gathers_the_projects_audit() {
        let dir = tempfile::tempdir().expect("dir");
        let mut app = app_in(dir.path());
        app.notebook
            .set(
                &app.project,
                NoteTarget::Line {
                    file: "a.c".into(),
                    line: 2,
                },
                "length unchecked",
                true,
            )
            .expect("note");
        app.triage
            .set(TriageEntry {
                rule: "strcpy".into(),
                file: "a.c".into(),
                line: 2,
                column: 0,
                anchor: "strcpy(d, s);".into(),
                status: TriageStatus::ConfirmedBug,
                note: "attacker controls s".into(),
                updated: 1,
            })
            .expect("verdict");
        let md = app.audit_report().markdown();
        assert!(md.contains("# Audit report — proj"), "{md}");
        assert!(md.contains("## Confirmed bugs (1)"), "{md}");
        assert!(md.contains("**strcpy** — `a.c:2`"), "{md}");
        assert!(md.contains("length unchecked"), "{md}");
    }
}

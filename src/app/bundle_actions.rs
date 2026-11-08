//! What the app does with audit bundles: File › Export Audit Bundle…
//! writes the open project's audit to a file; Import Audit Bundle… reads
//! one, shows what merging it would add, and merges the parts you keep.

use std::path::{Path, PathBuf};

use super::bundle::{self, Bundle, Counts, Merged, BUNDLE_EXTENSION};
use super::dangerous_lists::write_user_file;
use super::dialogs::{self, DialogState, PendingDialog};
use super::visible::visible;
use super::{SrcWranglrApp, Status};
use crate::notes::triage;
use crate::notes::Notes;

const FILE_KIND: &str = "SrcWranglr audit bundle";

/// A bundle file dialog that is open.
pub enum BundleDialog {
    /// Saving this bundle's text where the user picks.
    Export {
        dialog: PendingDialog<PathBuf>,
        text: String,
    },
    /// Picking a bundle to import.
    Import(PendingDialog<PathBuf>),
}

/// A bundle read, waiting for the user to say which parts to import.
pub struct BundlePreview {
    /// The project it was read for: importing into another is refused.
    root: PathBuf,
    file_name: String,
    /// What starts the text added under a note you already had.
    heading: String,
    bundle: Bundle,
    /// What importing every part would add.
    counts: Counts,
    choices: Choices,
}

/// Which parts of a bundle to import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choices {
    pub notes: bool,
    pub todos: bool,
    pub bug_verdicts: bool,
    /// Reviewed and false-positive verdicts, which hide findings: off
    /// until you turn them on.
    pub dismissing_verdicts: bool,
    pub session: bool,
}

impl SrcWranglrApp {
    /// Asks where to save the open project's audit as a bundle.
    pub(super) fn export_bundle_dialog(&mut self, ctx: &egui::Context) {
        if self.project.root.is_none() || !self.is_project_open {
            self.status = Status::Info("Open a project to export its audit.".into());
            return;
        }
        let text = match self.audit_bundle().to_text() {
            Ok(text) => text,
            Err(error) => return self.status = Status::Error(error),
        };
        let name = format!("{}-audit.{BUNDLE_EXTENSION}", self.project.name);
        let dialog = dialogs::spawn(ctx, move || {
            rfd::FileDialog::new()
                .set_title("Export Audit Bundle")
                .set_file_name(name)
                .add_filter(FILE_KIND, &[BUNDLE_EXTENSION])
                .save_file()
        });
        match dialog {
            Ok(dialog) => self.bundle_dialog = Some(BundleDialog::Export { dialog, text }),
            Err(error) => self.status = Status::Error(error),
        }
    }

    /// The open project's audit: where you are, your notes, TODOs and
    /// verdicts.
    fn audit_bundle(&self) -> Bundle {
        Bundle {
            project: self.project.name.clone(),
            exported: now(),
            session: Some(self.capture_session()),
            notes: self.notebook.notes().iter().cloned().collect(),
            todos: self.todos.items().to_vec(),
            verdicts: self.triage.entries().to_vec(),
        }
    }

    /// Asks for a bundle to import into the open project.
    pub(super) fn import_bundle_dialog(&mut self, ctx: &egui::Context) {
        if self.project.root.is_none() || !self.is_project_open {
            self.status = Status::Info(
                "Open the project the bundle is for first, then import it into it.".into(),
            );
            return;
        }
        let dialog = dialogs::spawn(ctx, || {
            rfd::FileDialog::new()
                .set_title("Import Audit Bundle")
                .add_filter(FILE_KIND, &[BUNDLE_EXTENSION])
                .pick_file()
        });
        match dialog {
            Ok(dialog) => self.bundle_dialog = Some(BundleDialog::Import(dialog)),
            Err(error) => self.status = Status::Error(error),
        }
    }

    pub(super) fn poll_bundle_dialog(&mut self) {
        let state = match &self.bundle_dialog {
            Some(BundleDialog::Export { dialog, .. } | BundleDialog::Import(dialog)) => {
                dialog.poll()
            }
            None => return,
        };
        if state == DialogState::Open {
            return;
        }
        let Some(finished) = self.bundle_dialog.take() else {
            return;
        };
        match (state, finished) {
            (DialogState::Picked(path), BundleDialog::Export { text, .. }) => {
                self.status = match write_user_file(&path, &text) {
                    Ok(saved) => Status::Info(format!("Audit exported to {}.", saved.display())),
                    Err(e) => Status::Error(format!("Could not save {}: {e}", path.display())),
                };
            }
            (DialogState::Picked(path), BundleDialog::Import(_)) => self.preview_bundle(&path),
            (DialogState::Failed(error), _) => {
                self.status = Status::Error(format!("Could not open the dialog: {error}"));
            }
            _ => {}
        }
    }

    /// Reads the bundle at `path` and works out what importing it would
    /// add, for the user to confirm.
    pub(super) fn preview_bundle(&mut self, path: &Path) {
        let Some(root) = self.project.root.clone() else {
            return;
        };
        let bundle = match bundle::read(path) {
            Ok(bundle) => bundle,
            Err(error) => return self.status = Status::Error(error),
        };
        let file_name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into(),
        );
        // Line notes follow their lines' text to where it is in this checkout.
        let imported = Notes::try_from_notes(bundle.notes.clone())
            .map(|notes| notes.relocated(&self.project.graph.file_contents).0)
            .map(|notes| notes.iter().cloned().collect())
            .unwrap_or_else(|_| bundle.notes.clone());
        let bundle = Bundle {
            notes: imported,
            ..bundle
        };
        let heading = format!(
            "— From the audit bundle {} ({}):",
            visible(&file_name),
            crate::history::date(i64::try_from(bundle.exported).unwrap_or(0))
        );
        // What each part would add, if all were imported.
        let counts = self.merged(&bundle, bundle::Parts::ALL, &heading).counts;
        let choices = Choices {
            notes: counts.changes_notes(),
            todos: counts.todos_added > 0,
            bug_verdicts: counts.bugs_added > 0,
            // These hide findings: only when you say so.
            dismissing_verdicts: false,
            session: bundle.session.is_some(),
        };
        self.bundle_preview = Some(Box::new(BundlePreview {
            root,
            file_name,
            heading,
            bundle,
            counts,
            choices,
        }));
    }

    /// The open project's notes, TODOs and verdicts with `parts` of
    /// `bundle` merged in.
    fn merged(&self, bundle: &Bundle, parts: bundle::Parts, heading: &str) -> Merged {
        let current = (self.notebook.notes(), &self.todos, &self.triage);
        bundle::merge(bundle, current, parts, heading, now())
    }

    /// The import window, while a bundle waits to be imported.
    pub(super) fn show_bundle_preview(&mut self, ctx: &egui::Context) {
        let Some(preview) = self.bundle_preview.as_mut() else {
            return;
        };
        let project = self.project.name.clone();
        let response = egui::Modal::new(egui::Id::unique("bundle_preview"))
            .show(ctx, |ui| preview_contents(ui, preview, &project));
        let (import, cancel) = response.inner;
        if import {
            if let Some(preview) = self.bundle_preview.take() {
                self.import_bundle(*preview);
            }
        } else if cancel || response.should_close() {
            self.bundle_preview = None;
        }
    }

    /// Merges the chosen parts of `preview`'s bundle into the project as it
    /// is now, and saves them. Refused if another project opened since.
    fn import_bundle(&mut self, preview: BundlePreview) {
        let BundlePreview {
            root,
            file_name,
            heading,
            bundle,
            choices,
            ..
        } = preview;
        let file_name = visible(&file_name).into_owned();
        if self.project.root.as_deref() != Some(root.as_path()) {
            self.status = Status::Error(format!(
                "Not imported: {file_name} was read for another project. Import it again."
            ));
            return;
        }
        let parts = bundle::Parts {
            notes: choices.notes,
            todos: choices.todos,
            bug_verdicts: choices.bug_verdicts,
            dismissing_verdicts: choices.dismissing_verdicts,
        };
        let merged = self.merged(&bundle, parts, &heading);
        let counts = &merged.counts;
        let mut done = Vec::new();
        let mut failed = Vec::new();
        if counts.changes_notes() {
            match self.notebook.replace_all(&self.project, merged.notes) {
                Ok(()) => done.push("notes"),
                Err(error) => failed.push(error),
            }
        }
        if counts.todos_added > 0 && self.set_todos(merged.todos) {
            done.push("TODOs");
        }
        if counts.changes_verdicts() {
            match self.import_verdicts(merged.triage) {
                Ok(()) => done.push("verdicts"),
                Err(error) => failed.push(error),
            }
        }
        if let Some(session) = bundle.session.filter(|_| choices.session) {
            self.restore_session(&session);
        }
        self.status = match failed.is_empty() {
            true => Status::Info(format!(
                "Imported {} from {file_name}.",
                list_of(&done).unwrap_or("nothing new")
            )),
            false => Status::Error(format!("Importing {file_name}: {}", failed.join("; "))),
        };
    }

    fn import_verdicts(&mut self, merged: triage::Triage) -> Result<(), String> {
        if let Some(error) = &self.triage_error {
            return Err(format!("Verdicts are read-only: {error}"));
        }
        if let Some(root) = &self.project.root {
            triage::save(root, &merged)?;
        }
        self.triage = merged;
        self.trust_project_files_we_wrote();
        self.refresh_verdicts();
        Ok(())
    }
}

/// "notes, TODOs and verdicts".
fn list_of(parts: &[&str]) -> Option<&'static str> {
    match parts {
        [] => None,
        ["notes"] => Some("notes"),
        ["TODOs"] => Some("TODOs"),
        ["verdicts"] => Some("verdicts"),
        ["notes", "TODOs"] => Some("notes and TODOs"),
        ["notes", "verdicts"] => Some("notes and verdicts"),
        ["TODOs", "verdicts"] => Some("TODOs and verdicts"),
        _ => Some("notes, TODOs and verdicts"),
    }
}

/// The import window's contents. Whether Import or Cancel was clicked.
fn preview_contents(ui: &mut egui::Ui, preview: &mut BundlePreview, project: &str) -> (bool, bool) {
    let counts = &preview.counts;
    let bundle = &preview.bundle;
    ui.heading("Import Audit Bundle");
    ui.label(visible(&preview.file_name));
    let exported = crate::history::date(i64::try_from(bundle.exported).unwrap_or(0));
    ui.weak(format!(
        "Exported from the project “{}” on {exported}.",
        visible(&bundle.project)
    ));
    let warn = ui.visuals().warn_fg_color;
    if bundle.project != project {
        ui.colored_label(
            warn,
            format!(
                "This project is “{}”: check it's the same code.",
                visible(project)
            ),
        );
    }
    ui.add_space(6.0);
    ui.label("Merged into yours, never replacing them:");
    let choices = &mut preview.choices;
    let notes = format!(
        "Notes and bookmarks: {} new, {} added under yours, {} newly bookmarked, {} already here",
        counts.notes_added, counts.notes_appended, counts.notes_starred, counts.notes_already
    );
    ui.add_enabled(
        counts.changes_notes(),
        egui::Checkbox::new(&mut choices.notes, notes),
    );
    let todos = format!(
        "TODOs: {} new, {} already on your list",
        counts.todos_added, counts.todos_already
    );
    ui.add_enabled(
        counts.todos_added > 0,
        egui::Checkbox::new(&mut choices.todos, todos),
    );
    let bugs = format!("Confirmed-bug verdicts: {} new", counts.bugs_added);
    ui.add_enabled(
        counts.bugs_added > 0,
        egui::Checkbox::new(&mut choices.bug_verdicts, bugs),
    );
    let dismissing = format!(
        "Reviewed and false-positive verdicts: {} new",
        counts.dismissing_added
    );
    ui.add_enabled(
        counts.dismissing_added > 0,
        egui::Checkbox::new(&mut choices.dismissing_verdicts, dismissing),
    );
    if counts.dismissing_added > 0 {
        ui.colored_label(
            warn,
            "These stop highlighting the findings they mark. Turn them on only if you trust who \
             sent the bundle.",
        );
    }
    if counts.verdicts_kept > 0 {
        ui.weak(format!(
            "{} findings you already judged keep your verdict.",
            counts.verdicts_kept
        ));
    }
    if bundle.session.is_some() {
        ui.checkbox(
            &mut choices.session,
            "Go to where they were (symbol, line and trace)",
        );
    }
    if !counts.skipped.is_empty() {
        ui.weak(format!(
            "Not added: {}",
            visible(&counts.skipped.join("; "))
        ));
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let import = ui.button("Import").clicked();
        let cancel = ui.button("Cancel").clicked();
        (import, cancel)
    })
    .inner
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
    use crate::notes::todo::{self, Todo, Todos};
    use crate::notes::triage::{TriageEntry, TriageStatus};
    use crate::notes::{store, NoteTarget};

    const SOURCE: &str = "void f(char *d, char *s) {\nstrcpy(d, s);\n}\n";

    /// A project of one file in `dir`, its notes and lists saved there.
    fn app_in(dir: &Path) -> SrcWranglrApp {
        std::fs::write(dir.join("a.c"), SOURCE).expect("source");
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let graph = graph_of_files(&[("a.c", SOURCE)]);
        app.project = Project::new("proj", Some(dir.to_path_buf()), graph);
        app.notebook = Notebook::open(&app.project);
        app.is_project_open = true;
        // Keeps the test out of the user's list of trusted projects.
        app.are_project_files_trusted = false;
        app
    }

    fn line_two() -> NoteTarget {
        NoteTarget::Line {
            file: "a.c".into(),
            line: 2,
        }
    }

    /// An audit with a starred note, a TODO and a verdict, exported to a
    /// file in `dir`.
    fn exported(dir: &Path) -> PathBuf {
        let mut theirs = app_in(dir);
        theirs
            .notebook
            .set(&theirs.project, line_two(), "length unchecked", true)
            .expect("note");
        let todos = Todos::default()
            .with_added(Todo::new("fuzz f()", Some(("a.c".into(), 2))))
            .expect("todo");
        assert!(theirs.set_todos(todos));
        theirs
            .triage
            .set(TriageEntry {
                rule: "strcpy".into(),
                file: "a.c".into(),
                line: 2,
                column: 0,
                anchor: "strcpy(d, s);".into(),
                status: TriageStatus::ConfirmedBug,
                note: String::new(),
                updated: 1,
            })
            .expect("verdict");
        let path = dir.join("audit.srcwranglr-audit");
        let text = theirs.audit_bundle().to_text().expect("text");
        std::fs::write(&path, text).expect("bundle");
        path
    }

    #[test]
    fn an_exported_audit_is_imported_into_another_checkout_and_saved_there() {
        let (theirs, mine) = (
            tempfile::tempdir().expect("a"),
            tempfile::tempdir().expect("b"),
        );
        let path = exported(theirs.path());
        let mut app = app_in(mine.path());
        app.preview_bundle(&path);
        let preview = app.bundle_preview.take().expect("a preview");
        let counts = &preview.counts;
        assert_eq!(
            (counts.notes_added, counts.todos_added, counts.bugs_added),
            (1, 1, 1)
        );
        app.import_bundle(*preview);
        let text = match &app.status {
            Status::Info(text) => text,
            Status::Error(error) => panic!("expected info, got {error}"),
        };
        assert!(text.contains("notes, TODOs and verdicts"), "{text}");
        let note = app.notebook.notes().get(&line_two()).expect("imported");
        assert!(note.is_starred);
        let saved = store::load(mine.path()).expect("notes saved");
        assert_eq!(
            saved.get(&line_two()).expect("on disk").text,
            "length unchecked"
        );
        assert_eq!(todo::load(mine.path()).expect("todos").items().len(), 1);
        assert_eq!(
            triage::load(mine.path()).expect("verdicts").entries().len(),
            1
        );
    }

    #[test]
    fn importing_adds_under_your_note_and_skips_what_you_turn_off() {
        let (theirs, mine) = (
            tempfile::tempdir().expect("a"),
            tempfile::tempdir().expect("b"),
        );
        let path = exported(theirs.path());
        let mut app = app_in(mine.path());
        app.notebook
            .set(&app.project, line_two(), "mine first", false)
            .expect("note");
        app.preview_bundle(&path);
        let mut preview = app.bundle_preview.take().expect("a preview");
        preview.choices.todos = false;
        preview.choices.bug_verdicts = false;
        app.import_bundle(*preview);
        let text = &app.notebook.notes().get(&line_two()).expect("kept").text;
        assert!(text.starts_with("mine first\n\n— From the audit bundle audit.srcwranglr-audit ("));
        assert!(text.ends_with("):\nlength unchecked"), "{text}");
        assert!(app.todos.items().is_empty());
        assert!(app.triage.entries().is_empty());
    }

    #[test]
    fn a_preview_read_for_another_project_is_not_imported() {
        let (theirs, mine) = (
            tempfile::tempdir().expect("a"),
            tempfile::tempdir().expect("b"),
        );
        let path = exported(theirs.path());
        let mut app = app_in(mine.path());
        app.preview_bundle(&path);
        let preview = app.bundle_preview.take().expect("a preview");
        let other = tempfile::tempdir().expect("c");
        let switched = app_in(other.path());
        app.project = switched.project;
        app.notebook = switched.notebook;
        app.import_bundle(*preview);
        assert!(app.notebook.notes().is_empty());
        let Status::Error(text) = &app.status else {
            panic!("expected an error");
        };
        assert!(text.contains("another project"), "{text}");
    }

    #[test]
    fn verdicts_that_hide_findings_are_off_until_turned_on() {
        let (theirs, mine) = (
            tempfile::tempdir().expect("a"),
            tempfile::tempdir().expect("b"),
        );
        let mut source = app_in(theirs.path());
        source
            .triage
            .set(TriageEntry {
                rule: "strcpy".into(),
                file: "a.c".into(),
                line: 2,
                column: 0,
                anchor: "strcpy(d, s);".into(),
                status: TriageStatus::FalsePositive,
                note: String::new(),
                updated: 1,
            })
            .expect("verdict");
        let path = theirs.path().join("fp.srcwranglr-audit");
        std::fs::write(&path, source.audit_bundle().to_text().expect("text")).expect("bundle");
        let mut app = app_in(mine.path());
        app.preview_bundle(&path);
        let preview = app.bundle_preview.take().expect("a preview");
        assert_eq!(preview.counts.dismissing_added, 1);
        assert!(!preview.choices.dismissing_verdicts);
        app.import_bundle(*preview);
        assert!(app.triage.entries().is_empty());
        app.preview_bundle(&path);
        let mut preview = app.bundle_preview.take().expect("a preview");
        preview.choices.dismissing_verdicts = true;
        app.import_bundle(*preview);
        assert_eq!(app.triage.entries().len(), 1);
    }

    #[test]
    fn a_broken_bundle_is_refused_with_why() {
        let mine = tempfile::tempdir().expect("b");
        let path = mine.path().join("bad.srcwranglr-audit");
        std::fs::write(&path, "{\"srcwranglr_audit\": 1, \"project\": \"\"}").expect("bad");
        let mut app = app_in(mine.path());
        app.preview_bundle(&path);
        assert!(app.bundle_preview.is_none());
        let Status::Error(text) = &app.status else {
            panic!("expected an error");
        };
        assert!(text.contains("project name"), "{text}");
    }
}

//! What the app does with sessions: remembering where you were in each
//! project and putting you back there when it opens, and Save Session As…
//! / Open Session… for session files.

use std::path::{Path, PathBuf};

use super::dangerous_lists::write_user_file;
use super::dialogs::{self, DialogState, PendingDialog};
use super::session::{self, Panes, Place, Session, View, SESSION_EXTENSION};
use super::{SrcWranglrApp, Status};
use crate::graph::ViewMode;

/// A session file dialog that is open.
pub enum SessionDialog {
    /// Saving this session file's text where the user picks.
    Save {
        dialog: PendingDialog<PathBuf>,
        text: String,
    },
    /// Picking a session file to open.
    Open(PendingDialog<PathBuf>),
}

const FILE_KIND: &str = "SrcWranglr session";

impl SrcWranglrApp {
    /// Where you are in the open project.
    pub(super) fn capture_session(&self) -> Session {
        let graph = &self.project.graph;
        let selected = self.graph_view.selected();
        let symbol = selected
            .and_then(|id| graph.symbols.get(&id))
            .map(|s| s.qualified_name.clone());
        let place = self.code_panel.position(graph, selected).map(|p| Place {
            file: p.file,
            line: p.line,
        });
        let view = match self.graph_view.mode() {
            ViewMode::Trail => View::Trail,
            ViewMode::CallHierarchy => View::CallHierarchy,
        };
        Session {
            symbol,
            place,
            view,
            panes: Panes {
                notes: self.notes_panel.is_open(),
                findings: self.findings_panel.is_open(),
                todos: self.todo_panel.is_open(),
                strings: self.strings_panel.is_open(),
                trace: Some(self.trace_panel.is_open()),
            },
            trace: self.trace_panel.start().cloned(),
        }
    }

    /// Keeps where you are in the open project (not the demo) for when it
    /// opens again.
    pub(super) fn remember_session(&mut self) {
        if !self.is_project_open {
            return;
        }
        if let Some(root) = self.project.root.clone() {
            self.sessions = self.sessions.with(&root, self.capture_session());
        }
    }

    /// Puts you back where `session` was, in what of it the project still
    /// has.
    pub(super) fn restore_session(&mut self, session: &Session) {
        self.graph_view.set_mode(match session.view {
            View::Trail => ViewMode::Trail,
            View::CallHierarchy => ViewMode::CallHierarchy,
        });
        self.notes_panel.set_open(session.panes.notes);
        self.findings_panel.set_open(session.panes.findings);
        self.todo_panel.set_open(session.panes.todos);
        self.strings_panel.set_open(session.panes.strings);
        let symbol = session.symbol.as_deref().and_then(|n| self.project.find(n));
        if let Some(id) = symbol {
            self.graph_view.select(id);
            let place = session
                .place
                .as_ref()
                .filter(|p| self.project.graph.file_contents.contains_key(&p.file));
            if let Some(place) = place {
                self.place_source_at(Some(id), &place.file, place.line);
            }
        }
        if let Some(start) = &session.trace {
            self.rerun_trace(start);
        }
        if let Some(is_shown) = session.panes.trace {
            self.trace_panel.set_open(is_shown);
        }
    }

    /// The session to restore for the project just opened: the one from a
    /// session file if it is for this project, else the one remembered.
    pub(super) fn restore_session_on_open(&mut self) {
        let root = self.project.root.clone();
        let from_file = self
            .pending_session
            .take()
            .filter(|(r, _)| root.as_deref().is_some_and(|root| same_folder(r, root)))
            .map(|(_, session)| session);
        let remembered = || root.as_ref().and_then(|r| self.sessions.get(r).cloned());
        if let Some(session) = from_file.or_else(remembered) {
            self.restore_session(&session);
        }
    }

    /// Asks where to save the session of the open project.
    pub(super) fn save_session_dialog(&mut self, ctx: &egui::Context) {
        let Some(root) = self.project.root.clone() else {
            self.status = Status::Info(
                "The demo has no folder to come back to: open a project to save its session."
                    .into(),
            );
            return;
        };
        let text = match session::to_file(&root, &self.capture_session()) {
            Ok(text) => text,
            Err(error) => return self.status = Status::Error(error),
        };
        let name = format!("{}.{SESSION_EXTENSION}", self.project.name);
        let dialog = dialogs::spawn(ctx, move || {
            rfd::FileDialog::new()
                .set_title("Save Session As")
                .set_file_name(name)
                .add_filter(FILE_KIND, &[SESSION_EXTENSION])
                .save_file()
        });
        match dialog {
            Ok(dialog) => self.session_dialog = Some(SessionDialog::Save { dialog, text }),
            Err(error) => self.status = Status::Error(error),
        }
    }

    /// Asks for a session file to open.
    pub(super) fn open_session_dialog(&mut self, ctx: &egui::Context) {
        let dialog = dialogs::spawn(ctx, || {
            rfd::FileDialog::new()
                .set_title("Open Session")
                .add_filter(FILE_KIND, &[SESSION_EXTENSION])
                .pick_file()
        });
        match dialog {
            Ok(dialog) => self.session_dialog = Some(SessionDialog::Open(dialog)),
            Err(error) => self.status = Status::Error(error),
        }
    }

    pub(super) fn poll_session_dialog(&mut self, ctx: &egui::Context) {
        let state = match &self.session_dialog {
            Some(SessionDialog::Save { dialog, .. } | SessionDialog::Open(dialog)) => dialog.poll(),
            None => return,
        };
        if state == DialogState::Open {
            return;
        }
        let Some(finished) = self.session_dialog.take() else {
            return;
        };
        match (state, finished) {
            (DialogState::Picked(path), SessionDialog::Save { text, .. }) => {
                self.status = match write_user_file(&path, &text) {
                    Ok(saved) => Status::Info(format!("Session saved to {}.", saved.display())),
                    Err(e) => Status::Error(format!("Could not save {}: {e}", path.display())),
                };
            }
            (DialogState::Picked(path), SessionDialog::Open(_)) => self.open_session(&path, ctx),
            (DialogState::Failed(error), _) => {
                self.status = Status::Error(format!("Could not open the dialog: {error}"));
            }
            _ => {}
        }
    }

    /// Opens the session file at `path`: its project (indexed first, unless
    /// it is the one open) and where you were in it. A folder you haven't
    /// opened before is only indexed once you agree.
    pub(super) fn open_session(&mut self, path: &Path, ctx: &egui::Context) {
        let (root, session) = match session::read_file(path) {
            Ok(read) => read,
            Err(error) => return self.status = Status::Error(error),
        };
        let is_open = self.is_project_open
            && self
                .project
                .root
                .as_deref()
                .is_some_and(|open| same_folder(open, &root));
        if is_open {
            self.restore_session(&session);
            self.status = Status::Info(format!("Opened the session in {}.", path.display()));
            return;
        }
        if !root.is_dir() {
            self.status = Status::Error(format!(
                "The session's project, {}, isn't there any more.",
                root.display()
            ));
            return;
        }
        let is_known = self.recent.paths().iter().any(|p| same_folder(p, &root));
        if is_known {
            self.index_for_session(root, session, ctx);
        } else {
            self.session_to_confirm = Some((root, session));
        }
    }

    fn index_for_session(&mut self, root: PathBuf, session: Session, ctx: &egui::Context) {
        self.pending_session = Some((root.clone(), session));
        self.start_indexing(root, ctx);
    }

    /// Asks before indexing a session file's project you haven't opened.
    pub(super) fn show_session_confirm(&mut self, ctx: &egui::Context) {
        let Some((root, _)) = &self.session_to_confirm else {
            return;
        };
        let folder = root.display().to_string();
        let response = egui::Modal::new(egui::Id::unique("session_confirm")).show(ctx, |ui| {
            ui.heading("Open the session's project?");
            ui.label("The session is for a project you haven't opened here. It will be indexed:");
            ui.monospace(&folder);
            ui.horizontal(|ui| {
                let open = ui.button("Index and Open").clicked();
                let cancel = ui.button("Cancel").clicked();
                (open, cancel)
            })
            .inner
        });
        let (open, cancel) = response.inner;
        if open {
            if let Some((root, session)) = self.session_to_confirm.take() {
                self.index_for_session(root, session, ctx);
            }
        } else if cancel || response.should_close() {
            self.session_to_confirm = None;
        }
    }
}

/// Whether `a` and `b` are the same folder (`/p/` is `/p`, a symlink is
/// what it points to).
fn same_folder(a: &Path, b: &Path) -> bool {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    a == b || real(a) == real(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::launch::LaunchOptions;
    use crate::app::project::Project;
    use crate::index::graph_of_files;

    const CODE: &str = "void helper(char *p) {\n  char b[8];\n  strcpy(b, p);\n}\n\
        int main(int argc, char **argv) {\n  helper(argv[1]);\n  return 0;\n}\n";

    fn app_with_project(root: &std::path::Path) -> SrcWranglrApp {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let graph = graph_of_files(&[("a.c", CODE)]);
        app.project = Project::new("p", Some(root.to_path_buf()), graph);
        app.is_project_open = true;
        app
    }

    #[test]
    fn a_session_puts_you_back_where_you_were() {
        let dir = tempfile::tempdir().expect("dir");
        let mut app = app_with_project(dir.path());
        let helper = app.project.find("helper").expect("helper");
        app.graph_view.select(helper);
        app.graph_view.set_mode(ViewMode::CallHierarchy);
        app.todo_panel.set_open(true);
        app.trace_variable("a.c", 3, "  strcpy(b, ".len() as u32);
        let session = app.capture_session();
        assert_eq!(session.symbol.as_deref(), Some("helper"));
        assert!(session.trace.is_some() && session.panes.todos);

        let mut fresh = app_with_project(dir.path());
        fresh.restore_session(&session);
        assert_eq!(fresh.graph_view.selected(), Some(helper));
        assert_eq!(fresh.graph_view.mode(), ViewMode::CallHierarchy);
        assert!(fresh.todo_panel.is_open());
        assert_eq!(fresh.trace_panel.start(), session.trace.as_ref());
        assert_eq!(fresh.capture_session(), session);
    }

    #[test]
    fn a_hidden_trace_pane_stays_hidden_with_its_trace_kept() {
        let dir = tempfile::tempdir().expect("dir");
        let mut app = app_with_project(dir.path());
        app.trace_variable("a.c", 3, "  strcpy(b, ".len() as u32);
        app.trace_panel.set_open(false);
        let session = app.capture_session();
        let mut fresh = app_with_project(dir.path());
        fresh.restore_session(&session);
        assert!(!fresh.trace_panel.is_open());
        assert_eq!(fresh.trace_panel.start(), session.trace.as_ref());
    }

    #[test]
    fn a_session_saved_before_trace_visibility_shows_its_trace() {
        let dir = tempfile::tempdir().expect("dir");
        let mut app = app_with_project(dir.path());
        app.trace_variable("a.c", 3, "  strcpy(b, ".len() as u32);
        let old = Session {
            panes: Panes {
                trace: None,
                ..app.capture_session().panes
            },
            ..app.capture_session()
        };
        let mut fresh = app_with_project(dir.path());
        fresh.restore_session(&old);
        assert!(fresh.trace_panel.is_open());
        let untraced = Session::default();
        let mut other = app_with_project(dir.path());
        other.restore_session(&untraced);
        assert!(!other.trace_panel.is_open());
    }

    #[test]
    fn restoring_a_session_leaves_a_hidden_source_pane_hidden() {
        let dir = tempfile::tempdir().expect("dir");
        let mut app = app_with_project(dir.path());
        app.layout = app.layout.with_source_shown(false);
        let session = Session {
            symbol: Some("helper".into()),
            place: Some(Place {
                file: "a.c".into(),
                line: 3,
            }),
            ..Session::default()
        };
        app.restore_session(&session);
        assert!(!app.layout.is_source_shown);
    }

    #[test]
    fn the_session_is_remembered_per_project() {
        let dir = tempfile::tempdir().expect("dir");
        let mut app = app_with_project(dir.path());
        let main = app.project.find("main").expect("main");
        app.graph_view.select(main);
        app.remember_session();
        let mut reopened = app_with_project(dir.path());
        reopened.sessions = app.sessions.clone();
        reopened.restore_session_on_open();
        assert_eq!(reopened.graph_view.selected(), Some(main));
    }

    #[test]
    fn a_session_files_session_is_only_restored_in_its_own_project() {
        let dir = tempfile::tempdir().expect("dir");
        let mut app = app_with_project(dir.path());
        let todos_open = Session {
            panes: Panes {
                todos: true,
                ..Panes::default()
            },
            ..Session::default()
        };
        app.pending_session = Some((PathBuf::from("/elsewhere"), todos_open.clone()));
        app.restore_session_on_open();
        assert!(!app.todo_panel.is_open(), "another project's session");
        assert!(app.pending_session.is_none());

        app.pending_session = Some((dir.path().to_path_buf(), todos_open));
        app.open_demo(&egui::Context::default());
        assert!(!app.todo_panel.is_open(), "the demo has no session");
        assert!(app.pending_session.is_none());
    }

    #[test]
    fn a_session_for_a_folder_never_opened_waits_for_your_ok() {
        let (open, elsewhere) = (
            tempfile::tempdir().expect("dir"),
            tempfile::tempdir().expect("dir"),
        );
        let mut app = app_with_project(open.path());
        let text = session::to_file(elsewhere.path(), &Session::default()).expect("text");
        let file = open.path().join(format!("s.{SESSION_EXTENSION}"));
        std::fs::write(&file, text).expect("write");
        app.open_session(&file, &egui::Context::default());
        assert!(app.job.is_none(), "nothing indexed yet");
        let waiting = app
            .session_to_confirm
            .as_ref()
            .map(|(root, _)| root.clone());
        assert_eq!(waiting.as_deref(), Some(elsewhere.path()));
    }

    #[test]
    fn a_session_file_for_the_open_project_restores_in_place() {
        let dir = tempfile::tempdir().expect("dir");
        let mut app = app_with_project(dir.path());
        let main = app.project.find("main").expect("main");
        app.graph_view.select(main);
        let text = session::to_file(dir.path(), &app.capture_session()).expect("text");
        let file = dir.path().join(format!("p.{SESSION_EXTENSION}"));
        std::fs::write(&file, text).expect("write");

        let mut other = app_with_project(dir.path());
        other.open_session(&file, &egui::Context::default());
        assert_eq!(other.graph_view.selected(), Some(main));
        assert!(other.job.is_none(), "the open project isn't indexed again");
    }
}

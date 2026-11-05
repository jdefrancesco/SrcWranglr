//! Opening a project: the folder dialog, indexing in the background, and
//! switching the app over to the result.

use std::path::PathBuf;

use super::dialogs::{self, DialogState};
use super::welcome::WelcomeAction;
use super::*;

impl SrcWranglrApp {
    pub(super) fn start_indexing(&mut self, root: PathBuf, ctx: &egui::Context) {
        if let Some(previous) = self.job.take() {
            previous.cancel();
        }
        // Old warnings belong to the old project; don't show them next to
        // this run's result (which may be an error).
        self.warnings.clear();
        self.is_warnings_open = false;
        self.status = Status::Info(format!("Indexing {}…", root.display()));
        self.job = Some(IndexJob::spawn(root, IndexOptions::default(), ctx.clone()));
    }

    /// Asks for a project folder, off the UI code (see `dialogs`);
    /// `poll_open_dialog` starts indexing the one picked.
    pub(super) fn open_project_dialog(&mut self, ctx: &egui::Context) {
        if self.open_dialog.is_some() {
            return;
        }
        let start = self.project.root.clone();
        let dialog = dialogs::spawn(ctx, move || {
            let dialog = rfd::FileDialog::new().set_title("Open a C/C++ project folder");
            match start {
                Some(root) => dialog.set_directory(root),
                None => dialog,
            }
            .pick_folder()
        });
        match dialog {
            Ok(dialog) => self.open_dialog = Some(dialog),
            Err(error) => self.status = Status::Error(error),
        }
    }

    pub(super) fn poll_open_dialog(&mut self, ctx: &egui::Context) {
        let Some(state) = self.open_dialog.as_ref().map(dialogs::PendingDialog::poll) else {
            return;
        };
        if state != DialogState::Open {
            self.open_dialog = None;
        }
        match state {
            DialogState::Open | DialogState::Cancelled => {}
            DialogState::Picked(root) => self.start_indexing(root, ctx),
            DialogState::Failed(error) => {
                self.status = Status::Error(format!("Could not open the folder dialog: {error}"));
            }
        }
    }

    pub(super) fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = self.job.as_mut() else {
            return;
        };
        let Some(result) = job.poll() else {
            return;
        };
        let root = job.root().to_path_buf();
        self.job = None;
        match result {
            Ok(outcome) => self.load_project(root, outcome, ctx),
            Err(IndexError::Cancelled) => self.status = Status::Info("Indexing cancelled.".into()),
            Err(err) => self.status = Status::Error(err.to_string()),
        }
    }

    pub(super) fn load_project(
        &mut self,
        root: PathBuf,
        outcome: IndexOutcome,
        ctx: &egui::Context,
    ) {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        self.status = Status::Info(format!(
            "Indexed {} files: {} symbols, {} relationships.",
            outcome.indexed_files,
            outcome.graph.symbols.len(),
            outcome.graph.edges.len()
        ));
        self.are_project_files_trusted =
            !trust::has_project_files(&root) || trust::is_trusted(&root);
        self.dangerous = DangerousLists::load(Some(&root), self.are_project_files_trusted);
        let triage = match crate::notes::triage::load(&root) {
            Ok(triage) => (triage, None),
            Err(error) => (Default::default(), Some(error)),
        };
        self.recent = self.recent.with_opened(&root);
        let project = Project::new(name, Some(root), outcome.graph);
        self.switch_to(project, triage, outcome.warnings, ctx);
    }

    /// Opens the built-in demo (stopping any indexing under way).
    pub(super) fn open_demo(&mut self, ctx: &egui::Context) {
        let project = match Project::demo() {
            Ok(project) => project,
            Err(error) => {
                self.status = Status::Error(format!("Could not load the demo: {error}"));
                return;
            }
        };
        if let Some(job) = self.job.take() {
            job.cancel();
        }
        self.status = Status::Info(
            "Showing the built-in demo. File > Open Project… to index your own code.".into(),
        );
        self.are_project_files_trusted = true;
        self.dangerous = DangerousLists::load(None, true);
        self.switch_to(project, (Default::default(), None), Vec::new(), ctx);
    }

    /// Does what the start page asked.
    pub(super) fn handle_welcome(&mut self, action: WelcomeAction, ctx: &egui::Context) {
        match action {
            WelcomeAction::OpenProject => self.open_project_dialog(ctx),
            WelcomeAction::OpenRecent(root) => self.open_recent(root, ctx),
            WelcomeAction::Forget(root) => self.recent = self.recent.without(&root),
            WelcomeAction::OpenDemo => self.open_demo(ctx),
        }
    }

    /// Indexes a project opened before, if its folder is still there.
    pub(super) fn open_recent(&mut self, root: PathBuf, ctx: &egui::Context) {
        if root.is_dir() {
            self.start_indexing(root, ctx);
        } else {
            self.status = Status::Error(format!("{} isn't there any more.", root.display()));
        }
    }

    /// Switches the app over to `project`, dropping what belonged to the one
    /// before. Its dangerous lists are already loaded.
    fn switch_to(
        &mut self,
        project: Project,
        (triage, triage_error): (crate::notes::triage::Triage, Option<String>),
        warnings: Vec<String>,
        ctx: &egui::Context,
    ) {
        self.findings = Findings::for_graph(&project.graph, self.dangerous.merged());
        self.reach = Reach::of(&project.graph);
        // Their results belong to the old project.
        self.tool_jobs.clear();
        self.finding_view = None;
        self.survey_view.reset();
        self.harness_view = None;
        self.report_findings();
        let name = project.name.clone();
        self.project = project;
        self.notebook = Notebook::open(&self.project);
        self.note_editor = None;
        self.triage = triage;
        self.triage_error = triage_error;
        self.refresh_verdicts();
        self.report_notes_on_open();
        // A new project starts with a fresh selection, in the same view mode.
        self.graph_view = GraphView::with_mode(self.graph_view.mode());
        self.code_panel = CodePanel::default();
        // Palette results hold ids from the old project.
        self.palette = Palette::default();
        self.symbol_list.invalidate();
        self.warnings = warnings;
        self.is_project_open = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{APP_NAME} — {name}")));
        self.apply_pending_symbol();
        self.apply_pending_triage(ctx);
    }

    pub(super) fn apply_pending_triage(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.is_triage_pending) {
            self.run_command(Command::Triage(SurveyTab::Overview), ctx);
        }
    }

    /// Adds what's worth knowing about the project's notes to the status.
    pub(super) fn report_notes_on_open(&mut self) {
        if let Some(error) = self.notebook.load_error() {
            self.status = Status::Error(format!("Notes are read-only: {error}"));
            return;
        }
        if let Some(error) = self.notebook.open_save_error() {
            self.status = Status::Error(format!("Line notes moved, but {error}"));
            return;
        }
        let moved = self.notebook.moved_on_open();
        if let (Status::Info(text), true) = (&mut self.status, moved > 0) {
            text.push_str(&format!(" {moved} line note(s) followed their code."));
        }
    }

    pub(super) fn apply_pending_symbol(&mut self) {
        let Some(name) = self.pending_symbol.take() else {
            return;
        };
        match self.project.find(&name) {
            Some(id) => self.graph_view.select(id),
            None => {
                self.status =
                    Status::Error(format!("Symbol {name} not found in {}", self.project.name))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_app() -> (SrcWranglrApp, egui::Context) {
        let ctx = egui::Context::default();
        let app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        (app, ctx)
    }

    /// Indexes `root` and waits for the result to load.
    fn index_and_load(app: &mut SrcWranglrApp, root: &std::path::Path, ctx: &egui::Context) {
        app.start_indexing(root.to_path_buf(), ctx);
        for _ in 0..2000 {
            app.poll_job(ctx);
            if app.job.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("indexing never finished");
    }

    fn project_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.c"), "int main(void) { return 0; }\n").expect("write");
        dir
    }

    #[test]
    fn the_app_starts_on_the_start_page_without_the_demo() {
        let (app, _) = fresh_app();
        assert!(!app.is_project_open);
        assert!(app.project.graph.symbols.is_empty());
    }

    #[test]
    fn the_demo_opens_from_the_start_page() {
        let (mut app, ctx) = fresh_app();
        app.handle_welcome(WelcomeAction::OpenDemo, &ctx);
        assert!(app.is_project_open);
        assert!(app.project.find("main").is_some());
    }

    #[test]
    fn a_symbol_asked_for_without_a_folder_is_looked_up_in_the_demo() {
        let ctx = egui::Context::default();
        let options = LaunchOptions {
            symbol: Some("main".into()),
            ..LaunchOptions::default()
        };
        let app = SrcWranglrApp::new(&ctx, None, options);
        assert!(app.is_project_open);
        assert_eq!(app.graph_view.selected(), app.project.find("main"));
    }

    #[test]
    fn an_opened_project_is_remembered_and_shown() {
        let (mut app, ctx) = fresh_app();
        let dir = project_dir();
        index_and_load(&mut app, dir.path(), &ctx);
        assert!(app.is_project_open);
        assert_eq!(
            app.recent.paths().first().map(PathBuf::as_path),
            Some(dir.path())
        );
    }

    #[test]
    fn a_recent_project_reopens_from_the_start_page() {
        let (mut app, ctx) = fresh_app();
        let dir = project_dir();
        app.handle_welcome(WelcomeAction::OpenRecent(dir.path().to_path_buf()), &ctx);
        assert!(app.job.is_some(), "indexing didn't start");
    }

    #[test]
    fn a_recent_project_that_is_gone_says_so() {
        let (mut app, ctx) = fresh_app();
        let gone = PathBuf::from("/no/such/srcwranglr-project");
        app.handle_welcome(WelcomeAction::OpenRecent(gone), &ctx);
        assert!(app.job.is_none());
        assert!(
            status_text(&app).contains("isn't there"),
            "{}",
            status_text(&app)
        );
    }

    #[test]
    fn the_start_page_opens_the_demo_when_asked() {
        use egui_kittest::kittest::Queryable;

        let (app, _) = fresh_app();
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1200.0, 800.0))
            .build_ui_state(
                |ui, app: &mut SrcWranglrApp| {
                    let ctx = ui.ctx().clone();
                    app.show_start_page(ui, &ctx);
                },
                app,
            );
        harness.run();
        harness.get_by_label("Try the built-in demo").click();
        harness.run();
        assert!(harness.state().is_project_open);
    }

    #[test]
    fn a_recent_project_can_be_forgotten() {
        let (mut app, ctx) = fresh_app();
        let path = PathBuf::from("/old/project");
        app.recent = app.recent.with_opened(&path);
        app.handle_welcome(WelcomeAction::Forget(path), &ctx);
        assert!(app.recent.paths().is_empty());
    }

    fn status_text(app: &SrcWranglrApp) -> &str {
        match &app.status {
            Status::Info(text) | Status::Error(text) => text,
        }
    }

    /// Polls the open-folder dialog until it has closed.
    fn wait_for_open_dialog(app: &mut SrcWranglrApp, ctx: &egui::Context) {
        for _ in 0..1000 {
            app.poll_open_dialog(ctx);
            if app.open_dialog.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the dialog never answered");
    }

    #[test]
    fn a_folder_picked_in_the_dialog_is_indexed() {
        let (mut app, ctx) = fresh_app();
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.c"), "int main(void) { return 0; }\n").expect("write");
        let root = dir.path().to_path_buf();
        app.open_dialog = Some(dialogs::spawn(&ctx, move || Some(root)).expect("spawn"));
        wait_for_open_dialog(&mut app, &ctx);
        assert!(app.job.is_some(), "indexing didn't start");
    }

    #[test]
    fn cancelling_the_dialog_changes_nothing() {
        let (mut app, ctx) = fresh_app();
        let before = status_text(&app).to_string();
        app.open_dialog = Some(dialogs::spawn(&ctx, || None::<PathBuf>).expect("spawn"));
        wait_for_open_dialog(&mut app, &ctx);
        assert!(app.job.is_none());
        assert_eq!(status_text(&app), before);
    }

    #[test]
    fn only_one_folder_dialog_opens_at_a_time() {
        let (mut app, ctx) = fresh_app();
        let (release, wait) = std::sync::mpsc::channel::<()>();
        app.open_dialog = Some(
            dialogs::spawn(&ctx, move || {
                let _ = wait.recv();
                None::<PathBuf>
            })
            .expect("spawn"),
        );
        // Would open a real dialog if one weren't already open.
        app.run_command(Command::OpenProject, &ctx);
        assert!(app.open_dialog.is_some());
        release.send(()).expect("release");
        wait_for_open_dialog(&mut app, &ctx);
    }
}

//! The window showing a drafted fuzz harness: edit it in place, copy it,
//! or save it where you choose. Nothing is written unless you pick a path.

use std::path::{Path, PathBuf};

use super::dialogs::{self, DialogState, PendingDialog};
use super::survey_view::{plural, ERROR_COLOR};
use super::{SrcWranglrApp, Status};
use crate::analysis::fuzz::harness::{draft, todo_count, Harness};
use crate::model::SymbolId;
use crate::safe_fs;

const DEFAULT_SIZE: [f32; 2] = [720.0, 560.0];
const CODE_ROWS: usize = 28;

/// What the last save did.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Saved {
    To(PathBuf),
    Failed(String),
}

pub struct HarnessView {
    function: String,
    file_name: String,
    /// The harness text, as edited.
    code: String,
    saved: Option<Saved>,
    /// The Save As… dialog, while it is open.
    save_dialog: Option<PendingDialog<PathBuf>>,
}

impl HarnessView {
    pub fn new(harness: Harness, function: &str) -> Self {
        Self {
            function: function.to_string(),
            file_name: harness.file_name,
            code: harness.code,
            saved: None,
            save_dialog: None,
        }
    }

    /// Draws the window; returns `false` once it is closed. `root` is where
    /// Save As… starts.
    pub fn show(&mut self, ctx: &egui::Context, root: Option<&Path>) -> bool {
        let mut is_open = true;
        egui::Window::new(format!("Fuzz Harness: {}", self.function))
            .id(egui::Id::unique("fuzz_harness"))
            .open(&mut is_open)
            .default_size(DEFAULT_SIZE)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| self.contents(ui, root));
        is_open
    }

    fn contents(&mut self, ui: &mut egui::Ui, root: Option<&Path>) {
        self.poll_save_dialog();
        let heading = ui.horizontal(|ui| {
            let heading = ui.strong(&self.file_name);
            let todos = todo_count(&self.code);
            ui.weak(format!("{todos} {} to look at", plural(todos, "TODO")));
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(self.code.clone());
            }
            let can_save = self.save_dialog.is_none();
            if ui
                .add_enabled(can_save, egui::Button::new("Save As…"))
                .clicked()
            {
                self.save_as(ui.ctx(), root);
            }
            heading.id
        });
        match &self.saved {
            Some(Saved::To(path)) => {
                ui.weak(format!("Saved to {}", path.display()));
            }
            Some(Saved::Failed(error)) => {
                ui.colored_label(ERROR_COLOR, format!("Could not save: {error}"));
            }
            None => {}
        }
        ui.separator();
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.code)
                    .code_editor()
                    .desired_rows(CODE_ROWS)
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(heading.inner);
        });
    }

    /// Asks where to save, off the UI code (see `dialogs`);
    /// `poll_save_dialog` saves there.
    fn save_as(&mut self, ctx: &egui::Context, root: Option<&Path>) {
        let file_name = self.file_name.clone();
        let root = root.map(Path::to_path_buf);
        let dialog = dialogs::spawn(ctx, move || {
            let dialog = rfd::FileDialog::new()
                .set_title("Save the fuzz harness")
                .set_file_name(file_name);
            match root {
                Some(root) => dialog.set_directory(root),
                None => dialog,
            }
            .save_file()
        });
        match dialog {
            Ok(dialog) => self.save_dialog = Some(dialog),
            Err(error) => self.saved = Some(Saved::Failed(error)),
        }
    }

    fn poll_save_dialog(&mut self) {
        let Some(state) = self.save_dialog.as_ref().map(PendingDialog::poll) else {
            return;
        };
        if state != DialogState::Open {
            self.save_dialog = None;
        }
        match state {
            DialogState::Open | DialogState::Cancelled => {}
            DialogState::Picked(path) => self.save_to(&path),
            DialogState::Failed(error) => self.saved = Some(Saved::Failed(error)),
        }
    }

    /// Writes the harness to `path`, replacing what's there but never
    /// writing through a symlink.
    fn save_to(&mut self, path: &Path) {
        let parts = path.parent().zip(path.file_name().and_then(|n| n.to_str()));
        let result = match parts {
            Some((dir, name)) => {
                safe_fs::write_atomically(dir, name, &self.code).map_err(|e| e.to_string())
            }
            None => Err("that isn't a file path".into()),
        };
        self.saved = Some(match result {
            Ok(()) => Saved::To(path.to_path_buf()),
            Err(error) => Saved::Failed(error),
        });
    }
}

impl SrcWranglrApp {
    /// Opens a drafted harness for the function defined at `file:line`.
    pub(super) fn open_harness(&mut self, symbol: SymbolId, file: &str, line: u32) {
        let graph = &self.project.graph;
        let drafted = graph
            .functions
            .iter()
            .find(|f| f.symbol == symbol && f.file == file && f.line == line)
            .and_then(|f| draft(graph, f));
        let name = graph.symbols.get(&symbol).map(|s| s.qualified_name.clone());
        match (drafted, name) {
            (Some(harness), Some(name)) => {
                self.harness_view = Some(HarnessView::new(harness, &name));
            }
            _ => {
                self.status = Status::Error(
                    "Could not draft a harness: that function is no longer indexed.".into(),
                );
            }
        }
    }

    pub(super) fn show_harness_view(&mut self, ctx: &egui::Context) {
        let root = self.project.root.clone();
        let is_open = self
            .harness_view
            .as_mut()
            .is_some_and(|view| view.show(ctx, root.as_deref()));
        if !is_open {
            self.harness_view = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    use super::*;

    fn view() -> HarnessView {
        let harness = Harness {
            file_name: "fuzz_parse.cc".into(),
            code: "// TODO: one\nint x;  // TODO: two\n".into(),
        };
        HarnessView::new(harness, "parse")
    }

    #[test]
    fn shows_the_file_name_and_what_needs_a_look() {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, view: &mut HarnessView| {
                    let ctx = ui.ctx().clone();
                    view.show(&ctx, None);
                },
                view(),
            );
        harness.run();
        assert!(harness.query_by_label("fuzz_parse.cc").is_some());
        assert!(harness.query_by_label("2 TODOs to look at").is_some());
        assert!(harness.query_by_label("Copy").is_some());
        assert!(harness.query_by_label("Save As…").is_some());
    }

    #[test]
    fn saving_writes_the_edited_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fuzz_parse.cc");
        let mut view = view();
        view.code.push_str("// edited\n");
        view.save_to(&path);
        assert_eq!(view.saved, Some(Saved::To(path.clone())));
        let written = std::fs::read_to_string(&path).expect("read");
        assert!(written.ends_with("// edited\n"));
    }

    #[test]
    fn a_path_picked_in_the_save_dialog_gets_the_harness() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fuzz_parse.cc");
        let picked = path.clone();
        let mut view = view();
        view.save_dialog =
            Some(dialogs::spawn(&egui::Context::default(), move || Some(picked)).expect("spawn"));
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, view: &mut HarnessView| {
                    let ctx = ui.ctx().clone();
                    view.show(&ctx, None);
                },
                view,
            );
        for _ in 0..400 {
            harness.step();
            if harness.state().save_dialog.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(harness.state().saved, Some(Saved::To(path.clone())));
        let written = std::fs::read_to_string(&path).expect("read");
        assert!(written.contains("TODO: one"));
    }

    #[cfg(unix)]
    #[test]
    fn saving_never_writes_through_a_symlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, "keep").expect("write");
        let link = dir.path().join("fuzz_parse.cc");
        std::os::unix::fs::symlink(&victim, &link).expect("symlink");
        let mut view = view();
        view.save_to(&link);
        assert!(matches!(view.saved, Some(Saved::Failed(_))));
        assert_eq!(std::fs::read_to_string(&victim).expect("read"), "keep");
    }
}

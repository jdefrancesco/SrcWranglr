//! The window showing what SrcWranglr drafted to fuzz a function: a
//! harness for libFuzzer, one for AFL++, a dictionary of the constants it
//! compares, and a Makefile building them all. Edit each in place, copy
//! it, or save it (or all of them) where you choose. Nothing is written
//! unless you pick a path.

use std::path::{Path, PathBuf};

use super::dialogs::PendingDialog;
use super::survey_view::{plural, ERROR_COLOR};
use super::{SrcWranglrApp, Status};
use crate::analysis::fuzz::dictionary::{self, Dictionary};
use crate::analysis::fuzz::harness::{draft, todo_count, Engine, Harness};
use crate::analysis::fuzz::makefile::{self, Makefile};
use crate::model::SymbolId;

#[path = "harness_saving.rs"]
mod saving;

use saving::{Draft, Saved};

const DEFAULT_SIZE: [f32; 2] = [720.0, 560.0];
const CODE_ROWS: usize = 28;

/// Everything drafted for one function.
pub struct Drafts {
    pub libfuzzer: Harness,
    pub afl: Harness,
    /// `None` when the function has no constants to list.
    pub dictionary: Option<Dictionary>,
    pub makefile: Makefile,
}

/// What the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Harness(Engine),
    Dictionary,
    Makefile,
}

pub struct HarnessView {
    function: String,
    tab: Tab,
    /// Whether the engine picked last has been looked up yet.
    is_tab_restored: bool,
    libfuzzer: Draft,
    afl: Draft,
    dictionary: Option<Draft>,
    makefile: Draft,
    /// The project's folder, which a saved Makefile's ROOT points to.
    root: Option<PathBuf>,
    /// The Save As… dialog, while it is open, and the tab it saves.
    save_dialog: Option<(Tab, PendingDialog<PathBuf>)>,
    /// The Save All… folder dialog, while it is open.
    save_all_dialog: Option<PendingDialog<PathBuf>>,
    /// What the last Save All did.
    saved_all: Option<String>,
}

impl HarnessView {
    pub fn new(drafts: Drafts, function: &str) -> Self {
        let Drafts {
            libfuzzer,
            afl,
            dictionary,
            makefile,
        } = drafts;
        Self {
            function: function.to_string(),
            tab: Tab::Harness(Engine::LibFuzzer),
            is_tab_restored: false,
            libfuzzer: Draft::new(libfuzzer.file_name, libfuzzer.code),
            afl: Draft::new(afl.file_name, afl.code),
            dictionary: dictionary.map(|d| Draft::new(d.file_name, d.text)),
            makefile: Draft::new(makefile.file_name, makefile.text),
            root: None,
            save_dialog: None,
            save_all_dialog: None,
            saved_all: None,
        }
    }

    /// Draws the window; returns `false` once it is closed. `root` is the
    /// project's folder: where saving starts, and what a saved Makefile's
    /// ROOT points to.
    pub fn show(&mut self, ctx: &egui::Context, root: Option<&Path>) -> bool {
        self.root = root.map(Path::to_path_buf);
        if !std::mem::replace(&mut self.is_tab_restored, true) {
            if let Some(engine) = ctx.data(|d| d.get_temp::<Engine>(engine_id())) {
                self.tab = Tab::Harness(engine);
            }
        }
        let mut is_open = true;
        egui::Window::new(format!("Fuzz Harness: {}", self.function))
            .id(egui::Id::unique("fuzz_harness"))
            .open(&mut is_open)
            .default_size(DEFAULT_SIZE)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| self.contents(ui));
        is_open
    }

    fn draft_mut(&mut self, tab: Tab) -> Option<&mut Draft> {
        match tab {
            Tab::Harness(Engine::LibFuzzer) => Some(&mut self.libfuzzer),
            Tab::Harness(Engine::Afl) => Some(&mut self.afl),
            Tab::Dictionary => self.dictionary.as_mut(),
            Tab::Makefile => Some(&mut self.makefile),
        }
    }

    fn is_saving(&self) -> bool {
        self.save_dialog.is_some() || self.save_all_dialog.is_some()
    }

    fn contents(&mut self, ui: &mut egui::Ui) {
        self.poll_dialogs();
        self.tabs(ui);
        let tab = self.tab;
        let can_save = !self.is_saving();
        let Some(draft) = self.draft_mut(tab) else {
            return;
        };
        let mut wants_save = false;
        let heading = ui.horizontal(|ui| {
            let heading = ui.strong(&draft.file_name);
            ui.weak(summary(tab, &draft.text));
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(draft.text.clone());
            }
            wants_save = ui
                .add_enabled(can_save, egui::Button::new("Save As…"))
                .clicked();
            heading.id
        });
        match &draft.saved {
            Some(Saved::To(path)) => {
                ui.weak(format!("Saved to {}", path.display()));
            }
            Some(Saved::Note(path, note)) => {
                ui.weak(format!("Saved to {}.", path.display()));
                ui.colored_label(ERROR_COLOR, note);
            }
            Some(Saved::Failed(error)) => {
                ui.colored_label(ERROR_COLOR, format!("Could not save: {error}"));
            }
            None => {}
        }
        ui.separator();
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut draft.text)
                    .code_editor()
                    .desired_rows(CODE_ROWS)
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(heading.inner);
        });
        if wants_save {
            self.save_as(ui.ctx(), tab);
        }
    }

    /// The harnesses, the dictionary and the Makefile; and Save All.
    fn tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for engine in Engine::ALL {
                let label = format!("{} harness", engine.label());
                let tab = Tab::Harness(engine);
                if ui.selectable_label(self.tab == tab, label).clicked() {
                    self.tab = tab;
                    ui.ctx().data_mut(|d| d.insert_temp(engine_id(), engine));
                }
            }
            let has_dictionary = self.dictionary.is_some();
            let is_shown = self.tab == Tab::Dictionary;
            let dictionary = ui
                .add_enabled(
                    has_dictionary,
                    egui::Button::selectable(is_shown, "Dictionary"),
                )
                .on_hover_text("Tokens for afl-fuzz -x, or libFuzzer's -dict=")
                .on_disabled_hover_text(
                    "It and the functions it calls compare or write no strings or characters",
                );
            if dictionary.clicked() {
                self.tab = Tab::Dictionary;
            }
            let makefile = ui
                .selectable_label(self.tab == Tab::Makefile, "Makefile")
                .on_hover_text("Builds the fuzzers: make -f <file>");
            if makefile.clicked() {
                self.tab = Tab::Makefile;
            }
            ui.separator();
            let save_all = ui
                .add_enabled(!self.is_saving(), egui::Button::new("Save All…"))
                .on_hover_text("Save every file into a folder you pick");
            if save_all.clicked() {
                self.save_all(ui.ctx());
            }
        });
        if let Some(saved) = &self.saved_all {
            ui.weak(saved);
        }
        ui.separator();
    }
}

/// Where the engine picked last is remembered, so the next harness opens
/// on it.
fn engine_id() -> egui::Id {
    egui::Id::unique("fuzz_harness_engine")
}

/// What needs a look in a harness or the Makefile, or how many tokens a
/// dictionary has.
fn summary(tab: Tab, text: &str) -> String {
    match tab {
        Tab::Harness(_) | Tab::Makefile => {
            let todos = todo_count(text);
            format!("{todos} {} to look at", plural(todos, "TODO"))
        }
        Tab::Dictionary => {
            let tokens = text.lines().filter(|l| l.starts_with('"')).count();
            format!("{tokens} {}", plural(tokens, "token"))
        }
    }
}

impl SrcWranglrApp {
    /// Opens what is drafted to fuzz the function defined at `file:line`.
    pub(super) fn open_harness(&mut self, symbol: SymbolId, file: &str, line: u32) {
        let graph = &self.project.graph;
        let function = graph
            .functions
            .iter()
            .find(|f| f.symbol == symbol && f.file == file && f.line == line);
        let drafted = function.and_then(|f| {
            let dictionary = dictionary::draft(graph, f);
            Some(Drafts {
                libfuzzer: draft(graph, f, Engine::LibFuzzer)?,
                afl: draft(graph, f, Engine::Afl)?,
                makefile: makefile::draft(graph, f, dictionary.is_some())?,
                dictionary,
            })
        });
        let name = graph.symbols.get(&symbol).map(|s| s.qualified_name.clone());
        match (drafted, name) {
            (Some(drafts), Some(name)) => {
                self.harness_view = Some(HarnessView::new(drafts, &name));
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
#[path = "harness_view_tests.rs"]
mod tests;

//! The Semgrep Rules window: every rule semgrep runs use (SrcWranglr's
//! own, yours, the project's), to read, test, edit, copy and delete, and
//! new ones to write; and, in its second tab, rules suggested for the
//! project's wrappers of risky calls.

use std::path::{Path, PathBuf};

use egui::RichText;

use super::rule_editor::{EditorAction, RuleEditor, SaveButton, SaveTarget};
use super::rule_files::RuleHome;
use super::rule_library::{with_id, Library, RuleFile, RuleSource, TEMPLATE};
use super::rules_wrappers::{WrapperAction, WrapperList};
use super::survey_view::ERROR_COLOR;
use super::visible::{visible, visible_lines};
use crate::analysis::semgrep_rule::rule_id;
use crate::model::{CodeGraph, SymbolId};

const DEFAULT_SIZE: [f32; 2] = [920.0, 620.0];
/// Rule ids listed under a file name, at most.
const MAX_IDS_SHOWN: usize = 5;
const MAX_COPY_ID_CHARS: usize = 60;
const LIST_WIDTH: f32 = 260.0;
/// What a new rule's file is named after if its id can't name one.
const NEW_ID: &str = "my-rule";

/// What the user asked of the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RulesAction {
    Editor(EditorAction),
    /// Delete this rule file.
    Delete(RuleHome, PathBuf),
    /// Trust the project's own files.
    Trust,
    /// Read the rules folders again.
    Reload,
    /// Look for the project's wrappers of risky calls.
    FindWrappers,
    /// Show this place in the source.
    GoTo {
        symbol: Option<SymbolId>,
        file: String,
        line: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Selected {
    /// A rule not saved yet, named after this id if its own can't be.
    New(String),
    File(RuleSource, usize),
}

/// What the window shows of the file picked: all but its text, which
/// stays in the library (it may be large).
struct Shown {
    source: RuleSource,
    path: Option<PathBuf>,
    name: String,
    /// Why it wasn't read, if it wasn't.
    problem: Option<String>,
    is_nested: bool,
}

impl Shown {
    fn of(file: &RuleFile) -> Self {
        Self {
            source: file.source,
            path: file.path.clone(),
            name: file.name.clone(),
            problem: file.text.as_ref().err().cloned(),
            is_nested: file.is_nested,
        }
    }
}

/// What to show once the edits to the rule shown are discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Pick(Selected),
    /// A new rule: its text, and the id its file falls back to.
    New(String, String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Rules,
    Wrappers,
}

pub struct RulesView {
    library: Library,
    /// Whether the project is a folder on disk (not the demo).
    has_project: bool,
    is_trusted: bool,
    selected: Selected,
    /// The rule's text as it was picked, to tell whether it was edited.
    original: String,
    pub editor: RuleEditor,
    /// A pick waiting on the edits to be discarded.
    pending: Option<Pending>,
    is_confirming_delete: bool,
    tab: Tab,
    pub wrappers: WrapperList,
}

impl RulesView {
    pub fn open(library: Library, has_project: bool, is_trusted: bool) -> Self {
        Self {
            library,
            has_project,
            is_trusted,
            selected: Selected::New(NEW_ID.into()),
            original: TEMPLATE.into(),
            editor: RuleEditor::new(TEMPLATE.into()),
            pending: None,
            is_confirming_delete: false,
            tab: Tab::default(),
            wrappers: WrapperList::default(),
        }
    }

    /// Lists `library` instead, picking the file at `path` if given (a
    /// rule just saved there), else keeping what was picked.
    pub fn reload(&mut self, library: Library, is_trusted: bool, path: Option<&Path>) {
        let picked_path = path
            .map(Path::to_path_buf)
            .or_else(|| self.picked_file().and_then(|f| f.path.clone()));
        self.library = library;
        self.is_trusted = is_trusted;
        self.is_confirming_delete = false;
        // It may name a file by where it was in the old list.
        self.pending = None;
        let found = picked_path.and_then(|path| self.find(&path));
        match (found, &self.selected) {
            // Unsaved edits to the file picked are kept.
            (Some(selected), Selected::File(..)) if path.is_none() && self.is_edited() => {
                self.selected = selected;
            }
            (Some(selected), _) => self.pick(selected),
            (None, Selected::File(..)) => self.pick(Selected::New(NEW_ID.into())),
            (None, Selected::New(_)) => {}
        }
    }

    /// Starts a new rule from `yaml`, named after `id` if its own can't;
    /// once the edits to the rule shown are discarded, if it has any.
    pub fn edit_new(&mut self, yaml: String, id: &str) {
        self.tab = Tab::Rules;
        if self.is_edited() {
            self.pending = Some(Pending::New(yaml, id.to_string()));
            return;
        }
        self.start_new(yaml, id);
    }

    fn start_new(&mut self, yaml: String, id: &str) {
        self.selected = Selected::New(id.to_string());
        self.original = yaml.clone();
        self.editor = RuleEditor::new(yaml);
        self.pending = None;
        self.is_confirming_delete = false;
    }

    fn find(&self, path: &Path) -> Option<Selected> {
        [RuleSource::Yours, RuleSource::Project]
            .into_iter()
            .find_map(|source| {
                let i = self
                    .library
                    .files(source)
                    .iter()
                    .position(|f| f.path.as_deref() == Some(path))?;
                Some(Selected::File(source, i))
            })
    }

    fn picked_file(&self) -> Option<&RuleFile> {
        match &self.selected {
            Selected::File(source, i) => self.library.files(*source).get(*i),
            Selected::New(_) => None,
        }
    }

    /// Whether the rule shown was edited and not saved (or is being
    /// saved: semgrep is checking it).
    fn is_edited(&self) -> bool {
        self.editor.yaml != self.original || self.editor.is_checking()
    }

    /// Shows `selected` in the editor.
    fn pick(&mut self, selected: Selected) {
        let file = match &selected {
            Selected::File(source, i) => self.library.files(*source).get(*i).cloned(),
            Selected::New(_) => None,
        };
        self.selected = selected;
        self.pending = None;
        self.is_confirming_delete = false;
        let Some(file) = file else {
            self.original = TEMPLATE.into();
            self.editor = RuleEditor::new(TEMPLATE.into());
            return;
        };
        let text = file.text.unwrap_or_default();
        self.editor = match (file.source, self.is_trusted) {
            // A project's own text, maybe hostile: escaped, to read only.
            (RuleSource::Project, false) => RuleEditor::read_only(visible_lines(&text)),
            (RuleSource::BuiltIn, _) => RuleEditor::read_only(text),
            _ if file.is_nested => RuleEditor::read_only(text),
            (RuleSource::Yours, _) | (RuleSource::Project, true) => RuleEditor::new(text),
        };
        self.original = self.editor.yaml.clone();
    }

    /// Draws the window; `None` once it is closed, else what was asked.
    pub fn show(&mut self, ctx: &egui::Context, graph: &CodeGraph) -> Option<Option<RulesAction>> {
        let mut is_open = true;
        let mut action = None;
        egui::Window::new("Semgrep Rules")
            .id(egui::Id::unique("semgrep_rules"))
            .open(&mut is_open)
            .default_size(DEFAULT_SIZE)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Rules, "Rules");
                    ui.selectable_value(&mut self.tab, Tab::Wrappers, "Suggested for This Project")
                        .on_hover_text("Rules for the project's wrappers of risky calls");
                });
                ui.separator();
                action = match self.tab {
                    Tab::Rules => self.rules_tab(ui, graph),
                    Tab::Wrappers => self.wrappers_tab(ui),
                };
            });
        is_open.then_some(action)
    }

    fn wrappers_tab(&mut self, ui: &mut egui::Ui) -> Option<RulesAction> {
        match self.wrappers.show(ui)? {
            WrapperAction::Find => Some(RulesAction::FindWrappers),
            WrapperAction::GoTo { symbol, file, line } => Some(RulesAction::GoTo {
                symbol: Some(symbol),
                file,
                line,
            }),
            WrapperAction::Edit(yaml) => {
                self.edit_new(yaml, "wrappers");
                None
            }
        }
    }

    fn rules_tab(&mut self, ui: &mut egui::Ui, graph: &CodeGraph) -> Option<RulesAction> {
        let mut action = None;
        ui.horizontal_top(|ui| {
            let height = ui.available_height();
            let list = egui::vec2(LIST_WIDTH, height);
            let top_down = egui::Layout::top_down(egui::Align::Min);
            ui.allocate_ui_with_layout(list, top_down, |ui| {
                ui.set_width(LIST_WIDTH);
                egui::ScrollArea::vertical()
                    .id_salt("rule_files")
                    .auto_shrink([false, false])
                    .show(ui, |ui| action = self.rule_list(ui));
            });
            ui.separator();
            ui.vertical(|ui| {
                egui::ScrollArea::vertical()
                    .id_salt("rule_detail")
                    .auto_shrink([false, false])
                    .show(ui, |ui| action = self.detail(ui, graph).or(action.take()));
            });
        });
        action
    }

    fn rule_list(&mut self, ui: &mut egui::Ui) -> Option<RulesAction> {
        let mut action = None;
        let mut clicked = None;
        ui.horizontal(|ui| {
            if ui.button("New Rule").clicked() {
                clicked = Some(Selected::New(NEW_ID.into()));
            }
            if ui
                .button("Reload")
                .on_hover_text("Read the rules folders again")
                .clicked()
            {
                action = Some(RulesAction::Reload);
            }
        });
        ui.add_space(4.0);
        ui.strong("SrcWranglr's own");
        ui.weak("Every semgrep run uses them.");
        clicked = self.file_rows(ui, RuleSource::BuiltIn).or(clicked);
        ui.add_space(6.0);
        ui.strong("Yours, for every project");
        if let Some(dir) = &self.library.yours_dir {
            ui.weak(visible(&dir.display().to_string()));
        }
        if self.library.yours.is_empty() {
            ui.weak("None yet.");
        }
        clicked = self.file_rows(ui, RuleSource::Yours).or(clicked);
        ui.add_space(6.0);
        ui.strong("This project's");
        action = self.project_status(ui).or(action);
        clicked = self.file_rows(ui, RuleSource::Project).or(clicked);
        if self.library.is_cut_short {
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Not everything is listed: a rules folder holds more files (or deeper \
                     folders, or more text) than is read here. Semgrep runs still use them all.",
                )
                .color(ERROR_COLOR),
            );
        }
        if let Some(selected) = clicked.filter(|s| *s != self.selected) {
            match self.is_edited() {
                true => self.pending = Some(Pending::Pick(selected)),
                false => self.pick(selected),
            }
        }
        action
    }

    fn project_status(&self, ui: &mut egui::Ui) -> Option<RulesAction> {
        if !self.has_project {
            ui.weak("Open a project folder to keep rules in it.");
            return None;
        }
        if self.is_trusted {
            ui.weak("Used: you trust this project's files.");
            if self.library.project.is_empty() {
                ui.weak("None yet.");
            }
            return None;
        }
        if self.library.project.is_empty() {
            ui.weak("None yet.");
            return None;
        }
        ui.label(
            RichText::new("Not used until you trust this project's files.").color(ERROR_COLOR),
        );
        ui.button("Trust This Project's Files")
            .on_hover_text(
                "Use its own verdicts, dangerous-functions list and rules: only if it is yours",
            )
            .clicked()
            .then_some(RulesAction::Trust)
    }

    /// The rows of `source`'s files; the one clicked.
    fn file_rows(&self, ui: &mut egui::Ui, source: RuleSource) -> Option<Selected> {
        let mut clicked = None;
        for (i, file) in self.library.files(source).iter().enumerate() {
            let selected = Selected::File(source, i);
            let is_selected = self.selected == selected;
            let mut label = visible(&file.name).into_owned();
            if file.text.is_err() {
                label.push_str("  (not read)");
            }
            let row = ui.selectable_label(is_selected, label);
            let ids = file
                .ids
                .iter()
                .filter(|id| !file.name.starts_with(id.as_str()))
                .take(MAX_IDS_SHOWN)
                .map(|id| visible(id).into_owned())
                .collect::<Vec<_>>();
            if source != RuleSource::BuiltIn && !ids.is_empty() {
                let more = match file.ids.len() > MAX_IDS_SHOWN {
                    true => ", …",
                    false => "",
                };
                ui.weak(format!("   {}{more}", ids.join(", ")));
            }
            if row.clicked() {
                clicked = Some(selected);
            }
        }
        clicked
    }

    fn detail(&mut self, ui: &mut egui::Ui, graph: &CodeGraph) -> Option<RulesAction> {
        if let Some(pending) = self.pending.clone() {
            ui.label(RichText::new("This rule has changes you haven't saved.").color(ERROR_COLOR));
            ui.horizontal(|ui| {
                if ui.button("Discard Them").clicked() {
                    match pending {
                        Pending::Pick(selected) => self.pick(selected),
                        Pending::New(yaml, id) => self.start_new(yaml, &id),
                    }
                }
                if ui.button("Keep Editing").clicked() {
                    self.pending = None;
                }
            });
            ui.separator();
        }
        let file = self.picked_file().map(Shown::of);
        let before = self.selected.clone();
        let (saves, action) = match &file {
            None => (self.new_rule_header(ui), None),
            Some(file) => self.file_header(ui, file),
        };
        if action.is_some() || self.selected != before {
            return action;
        }
        if let Some(problem) = file.as_ref().and_then(|f| f.problem.as_deref()) {
            ui.label(RichText::new(visible(problem)).color(ERROR_COLOR));
            return None;
        }
        ui.add_space(4.0);
        // Once saved, the app reloads the list and picks the file saved,
        // which makes its text the unedited one.
        self.editor.show(ui, graph, &saves).map(RulesAction::Editor)
    }

    fn new_rule_header(&mut self, ui: &mut egui::Ui) -> Vec<SaveButton> {
        ui.strong("New rule");
        ui.weak(
            "Write a rule, Test it on the project, then save it. Or draft one from code: \
             right-click a line → Find More Like This….",
        );
        let id = match &self.selected {
            Selected::New(id) => id.clone(),
            Selected::File(..) => NEW_ID.into(),
        };
        SaveButton::new_file(&id).into()
    }

    /// The picked file's name, what can be done with it, and its save
    /// button (if it can be saved).
    fn file_header(
        &mut self,
        ui: &mut egui::Ui,
        file: &Shown,
    ) -> (Vec<SaveButton>, Option<RulesAction>) {
        ui.strong(visible(&file.name));
        if let Some(path) = &file.path {
            ui.weak(visible(&path.display().to_string()));
        }
        let stem = file
            .name
            .trim_start_matches("srcwranglr.")
            .trim_end_matches(".yaml")
            .trim_end_matches(".yml");
        let copy_id = rule_id(&format!("my-{stem}"), MAX_COPY_ID_CHARS);
        let mut action = None;
        let mut copy_to = None;
        match (file.source, self.is_trusted) {
            (RuleSource::BuiltIn, _) => {
                ui.weak("SrcWranglr's own: copy it to change it (this one still runs).");
                ui.horizontal(|ui| {
                    if ui.button("Copy to Yours").clicked() {
                        copy_to = Some(RuleHome::Yours);
                    }
                    let project =
                        ui.add_enabled(self.has_project, egui::Button::new("Copy to This Project"));
                    if project.clicked() {
                        copy_to = Some(RuleHome::Project);
                    }
                });
            }
            (RuleSource::Project, false) => {
                ui.weak(
                    "The project's own file, shown escaped. Trust this project's files to \
                     edit it, or to have semgrep runs use it.",
                );
            }
            _ if file.is_nested => {
                ui.weak("In a subfolder: semgrep runs use it; edit it in its folder.");
            }
            (source, _) => action = self.file_buttons(ui, file, source, &mut copy_to),
        }
        if copy_to.is_some() {
            let text = self
                .picked_file()
                .and_then(|f| f.text.as_deref().ok())
                .unwrap_or("")
                .to_string();
            self.edit_new(with_id(&text, &copy_id), &copy_id);
            return (Vec::new(), None);
        }
        let can_save = !file.is_nested && (file.source != RuleSource::Project || self.is_trusted);
        let saves = match (file.source.home(), &file.path) {
            (Some(home), Some(path)) if can_save => {
                vec![SaveButton {
                    target: SaveTarget::Existing(home, path.clone()),
                    label: "Save",
                    hover: "Save your changes to this file",
                }]
            }
            _ => Vec::new(),
        };
        (saves, action)
    }

    fn file_buttons(
        &mut self,
        ui: &mut egui::Ui,
        file: &Shown,
        source: RuleSource,
        copy_to: &mut Option<RuleHome>,
    ) -> Option<RulesAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            let (other, label) = match source {
                RuleSource::Project => (RuleHome::Yours, "Copy to Yours"),
                _ => (RuleHome::Project, "Copy to This Project"),
            };
            let can_copy = other == RuleHome::Yours || self.has_project;
            if ui.add_enabled(can_copy, egui::Button::new(label)).clicked() {
                *copy_to = Some(other);
            }
            if ui.button("Delete…").clicked() {
                self.is_confirming_delete = true;
            }
        });
        if self.is_confirming_delete {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Delete {}? This can't be undone.",
                    visible(&file.name)
                ));
                if ui.button("Delete").clicked() {
                    if let (Some(home), Some(path)) = (source.home(), &file.path) {
                        action = Some(RulesAction::Delete(home, path.clone()));
                    }
                    self.is_confirming_delete = false;
                }
                if ui.button("Cancel").clicked() {
                    self.is_confirming_delete = false;
                }
            });
        }
        action
    }
}

#[cfg(test)]
#[path = "rules_view_tests.rs"]
mod tests;

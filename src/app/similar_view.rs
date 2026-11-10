//! The "Find More Like This" window: a semgrep rule drafted from code you
//! picked (a line, several lines, or several places that share a
//! pattern), with exceptions for the fixed form and for checks, to edit,
//! test on the project, and save for the project or for all of them.

use egui::RichText;

use super::findings::line_text;
use super::rule_editor::{EditorAction, RuleEditor, SaveButton};
use super::survey_view::ERROR_COLOR;
use super::visible::{self, visible};
use crate::analysis::semgrep_rule::{Exception, ExceptionKind};
use crate::analysis::similar::{draft_from, exception_from, Draft, Example, Looseness};
use crate::model::CodeGraph;

const DEFAULT_SIZE: [f32; 2] = [660.0, 600.0];
/// Places listed in full; more are counted.
const MAX_PLACES_LISTED: usize = 8;
/// Places a draft is made from, at most.
const MAX_PLACES: usize = 50;
/// What a fresh draft's id falls back to.
const FALLBACK_ID: &str = "like-this";

pub struct SimilarView {
    places: Vec<Example>,
    /// Whether other code may come between the statements of several lines.
    allow_gaps: bool,
    looseness: Looseness,
    /// The draft (or why there is none).
    draft: Result<Draft, String>,
    exceptions: Vec<Exception>,
    /// What adding a place or an exception last said, if it went wrong.
    problem: Option<String>,
    /// The rule as last drafted, to tell whether it has been edited.
    drafted: String,
    pub editor: RuleEditor,
}

impl SimilarView {
    /// The window for `place`, with a rule drafted from it.
    pub fn open(graph: &CodeGraph, place: Example) -> Self {
        Self::open_on(graph, vec![place])
    }

    /// The window for `places`, with a rule drafted from what they share.
    pub fn open_on(graph: &CodeGraph, places: Vec<Example>) -> Self {
        let mut view = Self {
            places: Vec::new(),
            allow_gaps: true,
            looseness: Looseness::default(),
            draft: Err(String::new()),
            exceptions: Vec::new(),
            problem: None,
            drafted: String::new(),
            editor: RuleEditor::default(),
        };
        for place in places.into_iter().take(MAX_PLACES) {
            if !view.places.contains(&place) {
                view.places.push(place);
            }
        }
        view.redraft(graph);
        view
    }

    /// Drafts again from the places so far and `place`.
    pub fn add_place(&mut self, graph: &CodeGraph, place: Example) {
        self.problem = None;
        if self.places.contains(&place) {
            self.problem = Some(format!(
                "{} is already a place of this rule.",
                place.place()
            ));
            return;
        }
        if self.places.len() >= MAX_PLACES {
            self.problem = Some(format!(
                "A rule is drafted from at most {MAX_PLACES} places."
            ));
            return;
        }
        self.places.push(place);
        self.redraft(graph);
    }

    /// Adds an exception drafted from line `line` of `file`.
    pub fn add_exception_from(&mut self, graph: &CodeGraph, file: &str, line: u32) {
        self.problem = None;
        let Ok(draft) = &self.draft else {
            self.problem = Some("Draft a rule first, then add exceptions to it.".into());
            return;
        };
        let source = graph.file_contents.get(file).map_or("", String::as_str);
        match exception_from(source, file, line, &draft.names, draft.callee.as_deref()) {
            Ok(exception) => {
                self.exceptions.push(exception);
                self.regenerate();
            }
            Err(error) => self.problem = Some(format!("{file}:{line}: {error}")),
        }
    }

    fn redraft(&mut self, graph: &CodeGraph) {
        let sources: Vec<(Example, &str)> = self
            .places
            .iter()
            .map(|place| {
                let source = graph
                    .file_contents
                    .get(&place.file)
                    .map_or("", String::as_str);
                (place.clone(), source)
            })
            .collect();
        self.draft = draft_from(&sources, self.allow_gaps);
        if self
            .draft
            .as_ref()
            .is_ok_and(|draft| draft.any_values.is_none())
        {
            self.looseness = Looseness::SameShape;
        }
        self.regenerate();
    }

    /// The rule from the draft as set up now, in place of the one shown.
    fn regenerate(&mut self) {
        let Ok(draft) = &self.draft else {
            return;
        };
        self.drafted = draft.yaml(self.looseness, &self.exceptions);
        self.editor.set_yaml(self.drafted.clone());
    }

    /// The id a new rule file falls back to.
    fn fallback_id(&self) -> String {
        self.draft
            .as_ref()
            .map_or(FALLBACK_ID.to_string(), |d| d.id.clone())
    }

    /// Draws the window; `None` once it is closed, else what was asked.
    pub fn show(&mut self, ctx: &egui::Context, graph: &CodeGraph) -> Option<Option<EditorAction>> {
        let mut is_open = true;
        let mut action = None;
        egui::Window::new("Find More Like This")
            .id(egui::Id::unique("find_more_like_this"))
            .open(&mut is_open)
            .default_size(DEFAULT_SIZE)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("find_more_like_this_scroll")
                    .show(ui, |ui| action = self.contents(ui, graph));
            });
        is_open.then_some(action)
    }

    fn contents(&mut self, ui: &mut egui::Ui, graph: &CodeGraph) -> Option<EditorAction> {
        self.places_list(ui, graph);
        ui.weak(
            "Add another place: right-click a line, or lines picked by shift-clicking their \
             numbers, → Add Place to Rule. Leave out fixed code or code after a check: \
             right-click it → Use as Rule Exception.",
        );
        if let Some(problem) = &self.problem {
            ui.label(RichText::new(problem).color(ERROR_COLOR));
        }
        if let Err(error) = &self.draft {
            ui.label(RichText::new(error).color(ERROR_COLOR));
            return None;
        }
        let is_edited = self.editor.yaml != self.drafted;
        let (looseness_changed, gaps_changed) = self.options(ui);
        let exceptions_changed = self.exception_list(ui);
        if is_edited {
            ui.weak("(changing these replaces your edits to the rule)");
        }
        if gaps_changed {
            self.redraft(graph);
        } else if looseness_changed || exceptions_changed {
            self.regenerate();
        }
        ui.separator();
        let saves = SaveButton::new_file(&self.fallback_id());
        self.editor.show(ui, graph, &saves)
    }

    fn places_list(&mut self, ui: &mut egui::Ui, graph: &CodeGraph) {
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let color = ui.visuals().text_color();
        if self.places.len() > 1 {
            ui.label(format!("From {} places:", self.places.len()));
        }
        let mut removed = None;
        for (i, place) in self.places.iter().enumerate().take(MAX_PLACES_LISTED) {
            let code = line_text(graph, &place.file, place.first_line).trim();
            ui.horizontal_wrapped(|ui| {
                let label = match self.places.len() {
                    1 => format!("From {}", visible(&place.place())),
                    _ => visible(&place.place()).into_owned(),
                };
                ui.label(label);
                ui.label(visible::job(code, font.clone(), color));
                let can_remove = self.places.len() > 1;
                if can_remove
                    && ui
                        .small_button("✕")
                        .on_hover_text("Leave this place out")
                        .clicked()
                {
                    removed = Some(i);
                }
            });
        }
        if self.places.len() > MAX_PLACES_LISTED {
            ui.weak(format!(
                "and {} more",
                self.places.len() - MAX_PLACES_LISTED
            ));
        }
        if let Some(i) = removed {
            self.places.remove(i);
            self.redraft(graph);
        }
    }

    /// The draft's options. Whether the looseness and the gaps changed.
    fn options(&mut self, ui: &mut egui::Ui) -> (bool, bool) {
        let has_any_values = self
            .draft
            .as_ref()
            .is_ok_and(|draft| draft.any_values.is_some());
        let is_sequence = self.draft.as_ref().is_ok_and(|draft| {
            draft.same_shape.contains('\n')
                || self.places.iter().any(|p| p.first_line != p.last_line)
        });
        let before = (self.looseness, self.allow_gaps);
        if has_any_values {
            ui.horizontal(|ui| {
                ui.label("Match:");
                for looseness in Looseness::ALL {
                    ui.radio_value(&mut self.looseness, looseness, looseness.label());
                }
            });
        }
        if is_sequence {
            ui.checkbox(
                &mut self.allow_gaps,
                "Other code may come between the statements",
            )
            .on_hover_text("Puts ... between them");
        }
        (self.looseness != before.0, self.allow_gaps != before.1)
    }

    /// The exceptions, to edit. Whether one changed.
    fn exception_list(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let mut removed = None;
        if !self.exceptions.is_empty() {
            ui.label("Except:");
        }
        for (i, exception) in self.exceptions.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let name = ui.label(format!("{}.", i + 1));
                let before = exception.kind;
                egui::ComboBox::from_id_salt(("exception_kind", i))
                    .selected_text(exception.kind.label())
                    .show_ui(ui, |ui| {
                        for kind in ExceptionKind::ALL {
                            ui.selectable_value(&mut exception.kind, kind, kind.label())
                                .on_hover_text(kind.hint());
                        }
                    })
                    .response
                    .labelled_by(name.id);
                changed |= exception.kind != before;
                let width = ui.available_width() - 32.0;
                let edit = ui
                    .add(
                        egui::TextEdit::singleline(&mut exception.pattern)
                            .code_editor()
                            .desired_width(width.max(120.0)),
                    )
                    .labelled_by(name.id);
                changed |= edit.changed();
                if ui
                    .small_button("✕")
                    .on_hover_text("Remove this exception")
                    .clicked()
                {
                    removed = Some(i);
                }
            });
        }
        if let Some(i) = removed {
            self.exceptions.remove(i);
            changed = true;
        }
        let add = ui
            .button("Add Exception")
            .on_hover_text("Leave out matches like a pattern you write here");
        if add.clicked() {
            self.exceptions.push(Exception::default());
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
#[path = "similar_view_tests.rs"]
mod tests;

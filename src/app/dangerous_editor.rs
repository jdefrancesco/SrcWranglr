//! "Mark as Dangerous…": a modal to add a function to your global or
//! project dangerous-functions list (or change / silence its entry).

use egui::Id;

use super::dangerous_lists::ListScope;
use crate::analysis::dangerous::DangerousFunction;
use crate::analysis::Severity;

const WIDTH: f32 = 440.0;
const SEVERITIES: [(Option<Severity>, &str); 4] = [
    (Some(Severity::High), "High"),
    (Some(Severity::Medium), "Medium"),
    (Some(Severity::Low), "Low"),
    (None, "Off (never flag)"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DangerousEditorOutcome {
    Save(DangerousFunction, ListScope),
    Cancel,
}

pub struct DangerousEditor {
    name: String,
    severity: Option<Severity>,
    reason: String,
    fix: String,
    scope: ListScope,
    can_use_project: bool,
    /// Kept from the existing entry (the editor doesn't show it).
    cwe: Option<u32>,
    error: Option<String>,
}

impl DangerousEditor {
    /// Starts from `existing` (the entry already listed, if any), else a
    /// high-severity entry for `name`.
    pub fn new(name: &str, existing: Option<&DangerousFunction>, can_use_project: bool) -> Self {
        Self {
            name: name.to_string(),
            severity: existing.map_or(Some(Severity::High), |e| e.severity),
            reason: existing.map(|e| e.reason.clone()).unwrap_or_default(),
            fix: existing.and_then(|e| e.fix.clone()).unwrap_or_default(),
            scope: if can_use_project {
                ListScope::Project
            } else {
                ListScope::Global
            },
            can_use_project,
            cwe: existing.and_then(|e| e.cwe),
            error: None,
        }
    }

    /// Keeps the editor open, showing why saving failed.
    pub fn show_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<DangerousEditorOutcome> {
        let response = egui::Modal::new(Id::unique("dangerous_editor")).show(ctx, |ui| {
            ui.set_width(WIDTH);
            ui.heading("Mark as Dangerous");
            self.fields(ui);
            ui.separator();
            ui.horizontal(|ui| self.buttons(ui)).inner
        });
        let should_close = response.should_close();
        response
            .inner
            .or_else(|| should_close.then_some(DangerousEditorOutcome::Cancel))
    }

    fn fields(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("dangerous_fields")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                let function = ui.label("Function");
                ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(f32::INFINITY))
                    .labelled_by(function.id);
                ui.end_row();
                ui.label("Severity");
                ui.horizontal(|ui| {
                    for (severity, label) in SEVERITIES {
                        ui.radio_value(&mut self.severity, severity, label);
                    }
                });
                ui.end_row();
                ui.label("Why");
                let why = egui::TextEdit::multiline(&mut self.reason)
                    .hint_text("What goes wrong when it's called (required)")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY);
                ui.add(why);
                ui.end_row();
                ui.label("Fix");
                let fix = egui::TextEdit::singleline(&mut self.fix)
                    .hint_text("Optional")
                    .desired_width(f32::INFINITY);
                ui.add(fix);
                ui.end_row();
                ui.label("Save in");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.scope, ListScope::Global, "Global list");
                    ui.add_enabled_ui(self.can_use_project, |ui| {
                        ui.radio_value(&mut self.scope, ListScope::Project, "This project");
                    });
                });
                ui.end_row();
            });
        ui.weak("A plain name matches the function in any namespace or class.");
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::from_rgb(0xE0, 0x6C, 0x6C), error);
        }
    }

    fn buttons(&self, ui: &mut egui::Ui) -> Option<DangerousEditorOutcome> {
        let entry = self.entry();
        let save = ui.add_enabled(entry.is_some(), egui::Button::new("Save"));
        if save.clicked() {
            return entry.map(|e| DangerousEditorOutcome::Save(e, self.scope));
        }
        ui.button("Cancel")
            .clicked()
            .then_some(DangerousEditorOutcome::Cancel)
    }

    /// The entry as filled in, if it has a name and a reason.
    fn entry(&self) -> Option<DangerousFunction> {
        let (name, reason) = (self.name.trim(), self.reason.trim());
        if name.is_empty() || reason.is_empty() {
            return None;
        }
        let fix = self.fix.trim();
        Some(DangerousFunction {
            name: name.to_string(),
            severity: self.severity,
            reason: reason.to_string(),
            fix: (!fix.is_empty()).then(|| fix.to_string()),
            cwe: self.cwe,
            is_advisory: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;

    struct State {
        editor: DangerousEditor,
        outcome: Option<DangerousEditorOutcome>,
    }

    fn harness(editor: DangerousEditor) -> Harness<'static, State> {
        let state = State {
            editor,
            outcome: None,
        };
        let mut harness = Harness::new_ui_state(
            |ui, state: &mut State| {
                let ctx = ui.ctx().clone();
                if let Some(outcome) = state.editor.show(&ctx) {
                    state.outcome = Some(outcome);
                }
            },
            state,
        );
        harness.run();
        harness
    }

    #[test]
    fn needs_a_reason_before_it_can_save() {
        let mut harness = harness(DangerousEditor::new("net::parse", None, true));
        harness.get_by_label("Save").click();
        harness.run();
        assert_eq!(harness.state().outcome, None);

        let text = harness.get_by_role(egui::Role::MultilineTextInput);
        text.focus();
        harness.run();
        harness
            .get_by_role(egui::Role::MultilineTextInput)
            .type_text("trusts the length field");
        harness.run();
        harness.get_by_label("Save").click();
        harness.run();
        let Some(DangerousEditorOutcome::Save(entry, scope)) = &harness.state().outcome else {
            panic!("not saved: {:?}", harness.state().outcome);
        };
        assert_eq!(entry.name, "net::parse");
        assert_eq!(entry.reason, "trusts the length field");
        assert_eq!(entry.severity, Some(Severity::High));
        assert_eq!(*scope, ListScope::Project);
    }

    #[test]
    fn an_existing_entry_can_be_switched_off_in_the_global_list() {
        let existing = DangerousFunction {
            name: "memcpy".into(),
            severity: Some(Severity::Low),
            reason: "check sizes".into(),
            fix: Some("bound it".into()),
            cwe: Some(120),
            is_advisory: false,
        };
        let mut harness = harness(DangerousEditor::new("memcpy", Some(&existing), false));
        harness.get_by_label("Off (never flag)").click();
        harness.run();
        harness.get_by_label("Save").click();
        harness.run();
        let Some(DangerousEditorOutcome::Save(entry, scope)) = &harness.state().outcome else {
            panic!("not saved");
        };
        assert_eq!(
            (entry.severity, entry.fix.as_deref()),
            (None, Some("bound it"))
        );
        // The demo has no project folder: only the global list.
        assert_eq!(*scope, ListScope::Global);
    }
}

//! The TODO pane on the right: the project's TODO list. Type one and press
//! Enter to add it; right-click a line in the source, then Add TODO Here,
//! to add one about that line, which it then links to.

use egui::RichText;

use super::layout::Layout;
use crate::notes::todo::{Todo, Todos};

const DEFAULT_WIDTH: f32 = 300.0;
const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x6C, 0x6C);

/// What the user did in the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoPanelAction {
    /// The list as changed, to save.
    Changed(Todos),
    /// The list with the TODO typed added, to save; then `clear_draft`.
    Added(Todos),
    /// Show this line in the source.
    GoTo { file: String, line: u32 },
}

#[derive(Default)]
pub struct TodoPanel {
    is_open: bool,
    /// The TODO being typed.
    draft: String,
    /// The line the next TODO is about.
    link: Option<(String, u32)>,
    /// The TODO being edited: index and text.
    editing: Option<(usize, String)>,
    /// Focus the new-TODO field next frame.
    should_focus: bool,
    error: Option<String>,
}

impl TodoPanel {
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn set_open(&mut self, is_open: bool) {
        self.is_open = is_open;
    }

    pub fn toggle(&mut self) {
        self.is_open = !self.is_open;
    }

    /// Forgets what belonged to the project before: what was being typed
    /// or edited, and the line it was about. Stays open or closed.
    pub fn reset(&mut self) {
        *self = Self {
            is_open: self.is_open,
            ..Self::default()
        };
    }

    /// Clears the TODO typed, once it has been saved.
    pub fn clear_draft(&mut self) {
        self.draft.clear();
        self.link = None;
        self.error = None;
    }

    /// Opens the pane to write a TODO about `file:line`.
    pub fn add_here(&mut self, file: String, line: u32) {
        self.is_open = true;
        self.link = Some((file, line));
        self.should_focus = true;
    }

    /// Draws the pane (if open) at the right of `ui`. `storage` says where
    /// the list is kept (or why it can't be).
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        todos: &Todos,
        storage: Result<&str, &str>,
        layout: Layout,
    ) -> Option<TodoPanelAction> {
        if !self.is_open {
            return None;
        }
        egui::Panel::right(layout.pane_id("todo_panel"))
            .resizable(true)
            .default_size(DEFAULT_WIDTH)
            .show(ui, |ui| self.contents(ui, todos, storage))
            .inner
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        todos: &Todos,
        storage: Result<&str, &str>,
    ) -> Option<TodoPanelAction> {
        ui.heading(format!("TODO ({} open)", todos.open_count()));
        match storage {
            Ok(place) => ui.weak(place),
            Err(error) => ui.label(RichText::new(error).color(ERROR_COLOR)),
        };
        // A list that couldn't be read can't be saved over either.
        let is_editable = storage.is_ok();
        let mut action = ui
            .add_enabled_ui(is_editable, |ui| self.new_todo(ui, todos))
            .inner;
        if let Some(error) = &self.error {
            ui.label(RichText::new(error).color(ERROR_COLOR));
        }
        ui.separator();
        if todos.items().is_empty() {
            ui.weak("Nothing to do yet. Type one above, or right-click a line of code.");
            return action;
        }
        // Above the list: the list takes the rest of the pane.
        if todos.items().iter().any(|t| t.is_done) && ui.button("Clear Done").clicked() {
            action = Some(TodoPanelAction::Changed(todos.without_done()));
        }
        let listed = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_enabled_ui(is_editable, |ui| self.list(ui, todos))
                    .inner
            })
            .inner;
        action.or(listed)
    }

    /// The field a new TODO is typed in (and the line it will be about).
    fn new_todo(&mut self, ui: &mut egui::Ui, todos: &Todos) -> Option<TodoPanelAction> {
        let field = egui::TextEdit::singleline(&mut self.draft)
            .hint_text("New TODO… (Enter adds it)")
            .desired_width(f32::INFINITY);
        let response = ui.add(field);
        if std::mem::take(&mut self.should_focus) {
            response.request_focus();
        }
        let is_entered = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if let Some((file, line)) = self.link.clone() {
            ui.horizontal(|ui| {
                ui.weak(format!("About {file}:{line}"));
                if ui.small_button("Unlink").clicked() {
                    self.link = None;
                }
            });
        }
        if !is_entered || self.draft.trim().is_empty() {
            return None;
        }
        match todos.with_added(Todo::new(&self.draft, self.link.clone())) {
            // Cleared once saved (`clear_draft`), so a failed save keeps it.
            Ok(added) => Some(TodoPanelAction::Added(added)),
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }

    fn list(&mut self, ui: &mut egui::Ui, todos: &Todos) -> Option<TodoPanelAction> {
        let mut action = None;
        for (index, todo) in todos.items().iter().enumerate() {
            let row = ui.horizontal_wrapped(|ui| self.row(ui, todos, index, todo));
            action = action.or(row.inner);
        }
        action
    }

    fn row(
        &mut self,
        ui: &mut egui::Ui,
        todos: &Todos,
        index: usize,
        todo: &Todo,
    ) -> Option<TodoPanelAction> {
        if self.editing.as_ref().is_some_and(|(i, _)| *i == index) {
            return self.edit_row(ui, todos);
        }
        let mut is_done = todo.is_done;
        let mut action = None;
        let checkbox = ui.checkbox(&mut is_done, "");
        if checkbox.changed() {
            action = Some(TodoPanelAction::Changed(todos.with_done(index, is_done)));
        }
        let text = match todo.is_done {
            true => RichText::new(&todo.text).strikethrough().weak(),
            false => RichText::new(&todo.text),
        };
        let label = ui.label(text).on_hover_text("Double-click to edit");
        checkbox.labelled_by(label.id);
        if label.double_clicked() {
            self.editing = Some((index, todo.text.clone()));
        }
        if let Some((file, line)) = todo.place() {
            let link = ui
                .link(format!("{file}:{line}"))
                .on_hover_text("Show in the source");
            if link.clicked() {
                action = Some(TodoPanelAction::GoTo {
                    file: file.to_string(),
                    line,
                });
            }
        }
        if ui.small_button("Delete").clicked() {
            action = Some(TodoPanelAction::Changed(todos.without(index)));
        }
        action
    }
}

impl TodoPanel {
    /// The TODO being edited: Enter (or clicking away) keeps the text,
    /// Escape drops it.
    fn edit_row(&mut self, ui: &mut egui::Ui, todos: &Todos) -> Option<TodoPanelAction> {
        if self
            .editing
            .as_ref()
            .is_some_and(|(i, _)| *i >= todos.items().len())
        {
            self.editing = None;
            return None;
        }
        let (index, text) = self.editing.as_mut()?;
        let response = ui.text_edit_singleline(text);
        response.request_focus();
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.editing = None;
            return None;
        }
        if !response.lost_focus() {
            return None;
        }
        let edited = todos.with_text(*index, text);
        self.editing = None;
        match edited {
            Ok(edited) => Some(TodoPanelAction::Changed(edited)),
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;

    struct State {
        panel: TodoPanel,
        todos: Todos,
        action: Option<TodoPanelAction>,
    }

    fn sample() -> Todos {
        Todos::default()
            .with_added(Todo::new("check parse_header", Some(("net.c".into(), 42))))
            .and_then(|t| t.with_added(Todo::new("fuzz the reader", None)))
            .expect("valid")
    }

    fn harness(todos: Todos, panel: TodoPanel) -> Harness<'static, State> {
        let mut harness = Harness::new_ui_state(
            |ui, state: &mut State| {
                let todos = state.todos.clone();
                let storage = Ok("Saved in .srcwranglr/todo.json");
                if let Some(action) = state.panel.show(ui, &todos, storage, Layout::default()) {
                    match &action {
                        TodoPanelAction::Changed(changed) => state.todos = changed.clone(),
                        TodoPanelAction::Added(added) => {
                            state.todos = added.clone();
                            state.panel.clear_draft();
                        }
                        TodoPanelAction::GoTo { .. } => {}
                    }
                    state.action = Some(action);
                }
            },
            State {
                panel: TodoPanel {
                    is_open: true,
                    ..panel
                },
                todos,
                action: None,
            },
        );
        harness.run();
        harness
    }

    #[test]
    fn lists_todos_with_their_place() {
        let harness = harness(sample(), TodoPanel::default());
        harness.get_by_label("TODO (2 open)");
        harness.get_by_label("check parse_header");
        harness.get_by_label("net.c:42");
        harness.get_by_label("Saved in .srcwranglr/todo.json");
    }

    #[test]
    fn typing_one_and_pressing_enter_adds_it_linked_to_the_line() {
        let mut panel = TodoPanel::default();
        panel.add_here("util.c".into(), 7);
        let mut harness = harness(Todos::default(), panel);
        harness.get_by_label("About util.c:7");
        harness
            .get_by_role(egui::Role::TextInput)
            .type_text("bounds check");
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        let items = harness.state().todos.items().to_vec();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "bounds check");
        assert_eq!(items[0].place(), Some(("util.c", 7)));
    }

    #[test]
    fn switching_projects_forgets_what_was_being_typed() {
        let mut panel = TodoPanel::default();
        panel.add_here("a.c".into(), 3);
        panel.draft = "half typed".into();
        panel.editing = Some((0, "edit".into()));
        panel.reset();
        assert!(panel.is_open());
        assert!(panel.draft.is_empty() && panel.link.is_none() && panel.editing.is_none());
    }

    #[test]
    fn a_place_goes_to_its_line_and_done_ones_can_be_cleared() {
        let mut harness = harness(sample().with_done(1, true), TodoPanel::default());
        harness.get_by_label("net.c:42").click();
        harness.run();
        assert_eq!(
            harness.state().action,
            Some(TodoPanelAction::GoTo {
                file: "net.c".into(),
                line: 42
            })
        );
        harness.get_by_label("Clear Done").click();
        harness.run();
        assert_eq!(harness.state().todos.items().len(), 1);
    }
}

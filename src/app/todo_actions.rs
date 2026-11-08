//! What the app does with the project's TODO list: reading it when a
//! project opens, the TODO pane, and saving each change.

use super::todo_panel::TodoPanelAction;
use super::{SrcWranglrApp, Status};
use crate::notes::store::NOTES_DIR;
use crate::notes::todo::{self, Todos, TODO_FILE};

impl SrcWranglrApp {
    /// Reads the open project's TODOs; a bad file leaves them read-only.
    pub(super) fn load_todos(&mut self) {
        let loaded = self.project.root.as_deref().map(todo::load);
        (self.todos, self.todos_error) = match loaded {
            Some(Ok(todos)) => (todos, None),
            Some(Err(error)) => (Todos::default(), Some(error)),
            None => (Todos::default(), None),
        };
    }

    pub(super) fn show_todo_panel(&mut self, ui: &mut egui::Ui) {
        let storage = match (&self.todos_error, &self.project.root) {
            (Some(error), _) => Err(format!("Read-only: {error}")),
            (None, Some(_)) => Ok(format!("Saved in {NOTES_DIR}/{TODO_FILE}")),
            (None, None) => Ok("Demo project: TODOs aren't saved".to_string()),
        };
        let storage = storage.as_deref().map_err(String::as_str);
        match self.todo_panel.show(ui, &self.todos, storage, self.layout) {
            Some(TodoPanelAction::Changed(todos)) => {
                self.set_todos(todos);
            }
            Some(TodoPanelAction::Added(todos)) => {
                if self.set_todos(todos) {
                    self.todo_panel.clear_draft();
                }
            }
            Some(TodoPanelAction::GoTo { file, line }) => self.go_to_place(None, &file, line),
            Some(TodoPanelAction::ShowTag(tag)) => {
                self.go_to_span(tag.function, &tag.file, tag.span)
            }
            None => {}
        }
    }

    /// Keeps `todos` as the list, saved with the project (if it has a
    /// folder and its file could be read). Whether it was kept.
    fn set_todos(&mut self, todos: Todos) -> bool {
        if let Some(error) = &self.todos_error {
            self.status = Status::Error(format!("TODOs are read-only: {error}"));
            return false;
        }
        if let Some(root) = &self.project.root {
            if let Err(error) = todo::save(root, &todos) {
                self.status = Status::Error(error);
                return false;
            }
        }
        self.todos = todos;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::launch::LaunchOptions;
    use crate::app::project::Project;
    use crate::notes::todo::Todo;

    #[test]
    fn a_change_is_saved_with_the_project_and_read_back() {
        let dir = tempfile::tempdir().expect("dir");
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let graph = crate::model::CodeGraph::default();
        app.project = Project::new("t", Some(dir.path().to_path_buf()), graph);
        app.load_todos();
        let todos = Todos::default()
            .with_added(Todo::new("audit the parser", None))
            .expect("valid");
        assert!(app.set_todos(todos.clone()));
        assert_eq!(app.todos, todos);
        app.todos = Todos::default();
        app.load_todos();
        assert_eq!(app.todos, todos);
        app.todos_error = Some("bad file".into());
        assert!(!app.set_todos(Todos::default()), "read-only");
        assert_eq!(app.todos, todos);
    }
}

//! What the app does with the Strings pane: showing it, and going to the
//! place picked in it.

use super::strings_panel::StringsPanelAction;
use super::SrcWranglrApp;

impl SrcWranglrApp {
    pub(super) fn show_strings_panel(&mut self, ui: &mut egui::Ui) {
        let picked = self
            .strings_panel
            .show(ui, &self.project.graph, self.layout);
        match picked {
            Some(StringsPanelAction::GoTo { symbol, file, span }) => {
                self.go_to_span(symbol, &file, span);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::launch::LaunchOptions;
    use crate::app::project::Project;
    use crate::app::strings::entries;
    use crate::app::SrcWranglrApp;
    use crate::index::graph_of_files;

    #[test]
    fn going_to_a_string_selects_its_function_and_shows_the_source() {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let graph = graph_of_files(&[("a.c", "int greet(void) {\n  return puts(\"hello\");\n}\n")]);
        let all = entries(&graph);
        app.project = Project::new("p", None, graph);
        app.strings_panel.set_strings(all.clone());
        app.layout = app.layout.with_source_shown(false);
        let place = all[0].uses[0].clone();
        app.go_to_span(place.function, &place.file, place.span);
        let greet = app.project.find("greet");
        assert!(greet.is_some());
        assert_eq!(app.graph_view.selected(), greet);
        assert!(app.layout.is_source_shown);
    }
}

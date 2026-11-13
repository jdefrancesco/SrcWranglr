//! What the app does for cross references: opening the window on a name
//! (X over it, its line's menu) or the selected symbol, and going to the
//! one clicked.

use super::xrefs_view::{XrefAction, XrefsView};
use super::{SrcWranglrApp, Status};
use crate::code_view::Referenced;

impl SrcWranglrApp {
    pub(super) fn show_cross_references(&mut self, target: &Referenced) {
        self.xrefs_view = Some(XrefsView::open(&self.project.graph, target));
    }

    /// Shows the project file `name` (as written in `from`) names, or says
    /// it isn't one of the project's.
    pub(super) fn open_named_file(&mut self, from: &str, name: &str) {
        match super::includes::resolve(&self.project.graph, from, name) {
            Some(file) => self.go_to_place(None, &file, 1),
            None => {
                self.status = Status::Info(format!(
                    "{name} isn't one of the project's files (a system or generated header?)."
                ));
            }
        }
    }

    pub(super) fn cross_references_to_selected(&mut self) {
        match self.graph_view.selected() {
            Some(id) => self.show_cross_references(&Referenced::Symbol(id)),
            None => {
                self.status = Status::Info(
                    "Select a symbol first, or press X over a name in the source.".into(),
                );
            }
        }
    }

    /// X, if nothing took it (not the source, not a text field): the
    /// selected symbol's cross references.
    pub(super) fn handle_cross_reference_key(&mut self, ctx: &egui::Context) {
        let is_pressed = !ctx.egui_wants_keyboard_input()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::X));
        if is_pressed && self.graph_view.selected().is_some() {
            self.cross_references_to_selected();
        }
    }

    pub(super) fn show_xrefs_view(&mut self, ctx: &egui::Context) {
        let Some(view) = self.xrefs_view.as_mut() else {
            return;
        };
        match view.show(ctx, &self.project.graph) {
            None => self.xrefs_view = None,
            Some(Some(XrefAction::GoTo { symbol, file, span })) => {
                self.go_to_span(symbol, &file, span);
            }
            Some(None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::launch::LaunchOptions;
    use crate::app::project::Project;
    use crate::app::SrcWranglrApp;
    use crate::code_view::Referenced;
    use crate::index::graph_of_files;

    #[test]
    fn an_included_header_is_shown_and_a_system_one_explained() {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let files = [
            (
                "src/main.c",
                "#include \"util.h\"\nint main(void) { return 0; }\n",
            ),
            ("src/util.h", "int util(int x);\n"),
        ];
        app.project = Project::new("p", None, graph_of_files(&files));
        app.open_named_file("src/main.c", "util.h");
        let shown = app
            .graph_view
            .selected()
            .and_then(|id| app.project.graph.symbols.get(&id));
        assert_eq!(shown.map(|s| s.file.as_str()), Some("src/util.h"));
        app.open_named_file("src/main.c", "stdio.h");
        assert!(
            matches!(&app.status, crate::app::Status::Info(said) if said.contains("stdio.h isn't"))
        );
    }

    #[test]
    fn a_name_on_a_line_opens_its_cross_references() {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let source = "void a(char *d) { memcpy(d, d, 4); }\n";
        app.project = Project::new("p", None, graph_of_files(&[("x.c", source)]));
        app.show_cross_references(&Referenced::Library("memcpy".into()));
        assert!(app.xrefs_view.is_some());
        app.cross_references_to_selected();
        assert!(matches!(app.status, crate::app::Status::Info(_)));
    }
}

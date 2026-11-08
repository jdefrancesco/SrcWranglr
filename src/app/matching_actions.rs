//! What the app does with the Matching pane: showing it, and Find Code
//! Like This Function from a line or a symbol.

use super::matching_panel::MatchingAction;
use super::{SrcWranglrApp, Status};
use crate::index::functions_holding_lines;
use crate::model::SymbolId;

impl SrcWranglrApp {
    pub(super) fn show_matching_panel(&mut self, ui: &mut egui::Ui) {
        self.matching_panel.poll(ui.ctx());
        let graph = std::sync::Arc::clone(&self.project.graph);
        if let Some(MatchingAction::GoTo { symbol, file, line }) =
            self.matching_panel.show(ui, &graph, self.layout)
        {
            self.go_to_place(symbol, &file, line);
        }
    }

    /// Lists the functions most like `symbol` in the Matching pane.
    pub(super) fn find_code_like(&mut self, symbol: SymbolId, ctx: &egui::Context) {
        let graph = std::sync::Arc::clone(&self.project.graph);
        self.matching_panel.find_like(symbol, &graph, ctx);
    }

    /// Lists the functions most like the one holding `file:line`.
    pub(super) fn find_code_like_line(&mut self, file: &str, line: u32, ctx: &egui::Context) {
        let holding = functions_holding_lines(&self.project.graph, &[(file, line)]);
        match holding.first().copied().flatten() {
            Some(symbol) => self.find_code_like(symbol, ctx),
            None => {
                self.status = Status::Info(
                    "That line isn't inside a function, so nothing was compared.".into(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::launch::LaunchOptions;
    use crate::app::project::Project;
    use crate::index::graph_of_files;

    #[test]
    fn a_line_outside_functions_is_said_and_one_inside_opens_the_pane() {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let source = "int x = 1;\nint f(int a) {\n  return a + x;\n}\n";
        app.project = Project::new("t", None, graph_of_files(&[("a.c", source)]));
        app.find_code_like_line("a.c", 1, &ctx);
        assert!(!app.matching_panel.is_open());
        let Status::Info(text) = &app.status else {
            panic!("expected info");
        };
        assert!(text.contains("isn't inside a function"), "{text}");
        app.find_code_like_line("a.c", 3, &ctx);
        assert!(app.matching_panel.is_open());
    }
}

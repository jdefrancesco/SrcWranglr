//! What the app does with docs: showing a function's doc comment or man
//! page in the Docs window, and looking up the man pages of library calls
//! hovered in the source.

use super::docs::{ProjectDoc, Shown};
use super::docs_view::{self, DocsAction};
use super::{SrcWranglrApp, Status};
use crate::model::SymbolId;

impl SrcWranglrApp {
    /// Shows the docs for the function `name` called in the source: its
    /// doc comment if the project defines it, else this system's man page.
    pub(super) fn show_docs_for(&mut self, name: &str, ctx: &egui::Context) {
        let graph = &self.project.graph;
        let defined = graph
            .symbols
            .values()
            .filter(|s| s.qualified_name == name)
            .min_by_key(|s| s.id)
            .map(|s| s.id);
        if let Some(id) = defined {
            return self.show_symbol_docs(id);
        }
        match self.docs.request(name, ctx) {
            Some(looked_up) => self.docs.shown = Some(Shown::Library(looked_up)),
            None => {
                self.status = Status::Info(format!(
                    "No docs for `{name}`: the project doesn't define it, and it isn't a C \
                     library name to look up."
                ));
            }
        }
    }

    /// Shows the doc comment of the project symbol `id`.
    pub(super) fn show_symbol_docs(&mut self, id: SymbolId) {
        if let Some(doc) = ProjectDoc::of(&self.project.graph, id) {
            self.docs.shown = Some(Shown::Project(Box::new(doc)));
        }
    }

    /// Starts looking up the man page of the library function `name`,
    /// hovered in the source.
    pub(super) fn look_up_docs(&mut self, name: &str, ctx: &egui::Context) {
        // A page read before (for another project's source pane) is
        // handed over at once; a new one arrives in `poll_docs`.
        let Some(looked_up) = self.docs.request(name, ctx) else {
            return;
        };
        if let Some(summary) = self.docs.summary(&looked_up) {
            self.code_panel.set_call_summary(looked_up, summary);
        }
    }

    /// Takes in the man pages read, for the source's hover summaries.
    pub(super) fn poll_docs(&mut self, ctx: &egui::Context) {
        for name in self.docs.poll(ctx) {
            let summary = self.docs.summary(&name).flatten();
            self.code_panel.set_call_summary(name, summary);
        }
    }

    pub(super) fn show_docs_window(&mut self, ctx: &egui::Context) {
        // A page let go to make room is read again while it is shown.
        if let Some(Shown::Library(name)) = self.docs.shown.clone() {
            self.docs.request(&name, ctx);
        }
        if let Some(DocsAction::GoTo { symbol, file, line }) = docs_view::show(ctx, &mut self.docs)
        {
            self.go_to_place(Some(symbol), &file, line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::launch::LaunchOptions;
    use crate::app::project::Project;
    use crate::index::graph_of_files;

    const SOURCE: &str = "/** @brief Frees it. */\nvoid release(void *p) {\n}\n\
        void run(char *p) {\n    release(p);\n    no_such_fn_q(p);\n}\n";

    fn app() -> (SrcWranglrApp, egui::Context) {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        app.project = Project::new("t", None, graph_of_files(&[("r.c", SOURCE)]));
        app.is_project_open = true;
        (app, ctx)
    }

    #[test]
    fn a_project_function_shows_its_doc_comment() {
        let (mut app, ctx) = app();
        app.show_docs_for("release", &ctx);
        let Some(Shown::Project(doc)) = &app.docs.shown else {
            panic!("expected the project's doc");
        };
        assert_eq!(doc.name, "release");
        let (parts, ..) = doc.comment.as_ref().expect("a comment");
        assert_eq!(parts.brief, "Frees it.");
    }

    #[test]
    fn another_function_shows_its_man_page() {
        let (mut app, ctx) = app();
        app.show_docs_for("std::no_such_fn_q", &ctx);
        assert_eq!(app.docs.shown, Some(Shown::Library("no_such_fn_q".into())));
    }

    #[test]
    fn a_page_read_before_is_handed_to_a_new_source_pane_at_once() {
        let (mut app, ctx) = app();
        let page = crate::docs::man::ManPage {
            name: "memcpy".into(),
            section: "3".into(),
            text: "NAME\n     memcpy – copy memory area\n".into(),
        };
        app.docs = std::mem::take(&mut app.docs).with_page("memcpy", Some(page));
        app.code_panel = crate::app::code_panel::CodePanel::default();
        app.look_up_docs("memcpy", &ctx);
        assert_eq!(
            app.code_panel.call_summary("memcpy"),
            Some(&Some("memcpy – copy memory area".to_string()))
        );
    }

    #[test]
    fn a_name_that_is_not_c_is_not_looked_up() {
        let (mut app, ctx) = app();
        app.show_docs_for("obj.method", &ctx);
        assert_eq!(app.docs.shown, None);
        let Status::Info(text) = &app.status else {
            panic!("expected info");
        };
        assert!(text.contains("No docs for `obj.method`"), "{text}");
    }
}

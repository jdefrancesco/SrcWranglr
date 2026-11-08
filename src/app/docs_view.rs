//! The Docs window: a project function's doc comment laid out (brief,
//! description, parameters, what it returns, notes), or a library
//! function's man page from this system.

use egui::RichText;

use super::docs::{Docs, PageState, ProjectDoc, Shown};
use super::visible::{visible, visible_lines};
use crate::docs::doxygen::Doc;
use crate::model::SymbolId;

const WIDTH: f32 = 600.0;
const HEIGHT: f32 = 520.0;

/// What the user asked for in the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocsAction {
    /// Show where a project function is.
    GoTo {
        symbol: SymbolId,
        file: String,
        line: u32,
    },
}

/// Draws the window while something is shown. Closing it forgets it.
pub fn show(ctx: &egui::Context, docs: &mut Docs) -> Option<DocsAction> {
    let shown = docs.shown.clone()?;
    let mut is_open = true;
    let title = match &shown {
        Shown::Project(doc) => format!("Docs: {}", visible(&doc.name)),
        Shown::Library(name) => format!("Docs: {name}()"),
    };
    let action = egui::Window::new(title)
        .id(egui::Id::unique("docs_window"))
        .open(&mut is_open)
        .default_size([WIDTH, HEIGHT])
        .resizable(true)
        .show(ctx, |ui| match &shown {
            Shown::Project(doc) => project(ui, doc),
            Shown::Library(name) => {
                library(ui, name, docs.page(name));
                None
            }
        })
        .and_then(|response| response.inner)
        .flatten();
    if !is_open {
        docs.shown = None;
    }
    action
}

fn project(ui: &mut egui::Ui, doc: &ProjectDoc) -> Option<DocsAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.heading(visible(&doc.name));
        ui.weak(doc.kind);
    });
    ui.horizontal(|ui| {
        ui.weak(visible(&format!("{}:{}", doc.file, doc.line)));
        if ui.small_button("Show in Source").clicked() {
            action = Some(DocsAction::GoTo {
                symbol: doc.symbol,
                file: doc.file.clone(),
                line: doc.line,
            });
        }
    });
    if !doc.signature.is_empty() {
        ui.label(RichText::new(visible(&doc.signature)).monospace());
    }
    ui.separator();
    let Some((parts, file, line, is_doxygen)) = &doc.comment else {
        ui.weak(format!(
            "No comment documents `{}`, above its definition or a declaration.",
            visible(&doc.name)
        ));
        return action;
    };
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            comment(ui, parts);
            ui.add_space(8.0);
            let kind = match is_doxygen {
                true => "Doxygen comment",
                false => "comment",
            };
            ui.weak(visible(&format!("From the {kind} at {file}:{line}.")));
        });
    action
}

/// The parts of a doc comment, each under its own heading.
fn comment(ui: &mut egui::Ui, doc: &Doc) {
    if !doc.brief.is_empty() {
        ui.label(RichText::new(visible_lines(&doc.brief)).strong());
    }
    for paragraph in &doc.details {
        ui.add_space(4.0);
        ui.label(visible_lines(paragraph));
    }
    if !doc.params.is_empty() {
        section(ui, "Parameters");
        egui::Grid::new("docs_params")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                for param in &doc.params {
                    let name = match &param.direction {
                        Some(direction) => format!("{} [{direction}]", param.name),
                        None => param.name.clone(),
                    };
                    ui.label(RichText::new(visible(&name)).monospace().strong());
                    ui.label(visible_lines(&param.text));
                    ui.end_row();
                }
            });
    }
    if !doc.returns.is_empty() {
        section(ui, "Returns");
        for text in &doc.returns {
            ui.label(visible_lines(text));
        }
    }
    for (label, text) in &doc.notes {
        section(ui, label);
        ui.label(visible_lines(text));
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(title).strong());
}

fn library(ui: &mut egui::Ui, name: &str, page: Option<&PageState>) {
    match page {
        None | Some(PageState::Reading) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Reading the man page for {name}()…"));
            });
        }
        Some(PageState::Failed(error)) => {
            ui.label(format!("Couldn't read the man page for {name}(): {error}"));
        }
        Some(PageState::Read(None)) => {
            ui.label(format!("This system has no man page for {name}()."));
            ui.weak(
                "It may be a macro, a function the project gets from elsewhere, or one from a \
                 library whose manual isn't installed.",
            );
        }
        Some(PageState::Read(Some(page))) => {
            ui.horizontal(|ui| {
                ui.heading(page.title());
                ui.weak(format!(
                    "this system's manual (man {} {})",
                    page.section, page.name
                ));
            });
            if let Some(summary) = page.summary() {
                ui.label(RichText::new(summary).strong());
            }
            ui.separator();
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Laid out by `man` already: scroll sideways, don't wrap.
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.label(RichText::new(visible_lines(&page.text)).monospace());
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::docs::doxygen::Param;

    fn showing(shown: Shown) -> Docs {
        let mut docs = Docs::default();
        docs.shown = Some(shown);
        docs
    }

    fn sample() -> ProjectDoc {
        ProjectDoc {
            symbol: 7,
            name: "copy_field".into(),
            kind: "function",
            file: "f.c".into(),
            line: 12,
            signature: "int copy_field(char *dst, const char *src, int n)".into(),
            comment: Some((
                Doc {
                    brief: "Copies a field.".into(),
                    details: vec!["At most n bytes.".into()],
                    params: vec![Param {
                        name: "dst".into(),
                        direction: Some("out".into()),
                        text: "where to".into(),
                    }],
                    returns: vec!["the bytes copied".into()],
                    notes: vec![("Warning".into(), "Not thread-safe.".into())],
                },
                "f.h".into(),
                3,
                true,
            )),
        }
    }

    #[test]
    fn shows_a_project_function_s_doc_and_goes_to_it() {
        let mut docs = showing(Shown::Project(Box::new(sample())));
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
            |ui, docs: &mut Docs| {
                let ctx = ui.ctx().clone();
                if let Some(DocsAction::GoTo { line, .. }) = show(&ctx, docs) {
                    docs.shown = None;
                    assert_eq!(line, 12);
                }
            },
            std::mem::take(&mut docs),
        );
        harness.run();
        for text in [
            "Copies a field.",
            "dst [out]",
            "where to",
            "Returns",
            "Warning",
            "Not thread-safe.",
        ] {
            harness.get_by_label(text);
        }
        harness.get_by_label_contains("From the Doxygen comment at f.h:3");
        harness.get_by_label("Show in Source").click();
        harness.run();
        assert!(harness.state().shown.is_none());
    }

    #[test]
    fn says_when_no_comment_documents_it() {
        let docs = showing(Shown::Project(Box::new(ProjectDoc {
            comment: None,
            ..sample()
        })));
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
            |ui, docs: &mut Docs| {
                let ctx = ui.ctx().clone();
                show(&ctx, docs);
            },
            docs,
        );
        harness.run();
        harness.get_by_label_contains("No comment documents `copy_field`");
    }

    #[test]
    fn a_library_page_being_read_shows_a_spinner() {
        let docs = showing(Shown::Library("strcpy".into()));
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
            |ui, docs: &mut Docs| {
                let ctx = ui.ctx().clone();
                show(&ctx, docs);
            },
            docs,
        );
        harness.step();
        harness.get_by_label_contains("Reading the man page for strcpy()");
    }
}

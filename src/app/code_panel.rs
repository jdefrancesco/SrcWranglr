//! The bottom panel: a header listing where the selected symbol appears
//! (◀ 3 / 12 ▶ · call in `Foo::bar`), above the source at that location.

use egui::RichText;

use super::locations::LocationCursor;
use crate::code_view::{CodeView, Focus};
use crate::model::{CodeGraph, Location, OccurrenceRole, Symbol, SymbolId};

enum HeaderAction {
    Step(isize),
    Select(SymbolId),
}

#[derive(Default)]
pub struct CodePanel {
    view: CodeView,
    cursor: Option<LocationCursor>,
    /// The focus moved since the last frame, so scroll it into view.
    should_scroll: bool,
}

impl CodePanel {
    /// Shows the selected symbol's current location. Returns a symbol the
    /// user clicked (a name in the source, or the enclosing symbol link).
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        selected: Option<SymbolId>,
    ) -> Option<SymbolId> {
        self.sync(graph, selected);
        let Some(symbol) = selected.and_then(|id| graph.symbols.get(&id)) else {
            ui.centered_and_justified(|ui| ui.label("Select a symbol to view its source."));
            return None;
        };
        let header_action = self.show_header(ui, graph, symbol);
        ui.separator();
        if let Some(HeaderAction::Step(delta)) = header_action {
            self.cursor = self.cursor.take().map(|c| c.stepped(delta));
            self.should_scroll = true;
        }
        let location = self.cursor.as_ref().and_then(LocationCursor::current);
        let focus = focus_for(symbol, location, self.should_scroll);
        let clicked = self.view.show(ui, graph, &focus);
        let file = focus.file.to_string();
        self.should_scroll = false;
        if let Some(HeaderAction::Select(id)) = header_action {
            return Some(id);
        }
        let clicked = clicked?;
        if clicked.symbol != symbol.id {
            return Some(clicked.symbol);
        }
        // Another occurrence of the selected symbol: make it the current location.
        self.cursor = self
            .cursor
            .take()
            .map(|c| c.focused_on(&file, clicked.span));
        None
    }

    /// Starts a fresh location list whenever the selection changes.
    fn sync(&mut self, graph: &CodeGraph, selected: Option<SymbolId>) {
        if self.cursor.as_ref().map(LocationCursor::symbol) != selected {
            self.cursor = selected.map(|id| LocationCursor::new(graph, id));
            self.should_scroll = true;
        }
    }

    fn show_header(
        &self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        symbol: &Symbol,
    ) -> Option<HeaderAction> {
        let cursor = self.cursor.as_ref();
        let location = cursor.and_then(LocationCursor::current);
        let file = location.map_or(symbol.file.as_str(), |l| l.file.as_str());
        ui.horizontal(|ui| {
            ui.label(RichText::new(file).strong().monospace());
            let cursor = cursor.filter(|c| c.len() > 0)?;
            ui.separator();
            let mut action = None;
            let can_step = cursor.len() > 1;
            let previous = ui.add_enabled(can_step, egui::Button::new("◀"));
            if previous.on_hover_text("Previous location").clicked() {
                action = Some(HeaderAction::Step(-1));
            }
            ui.label(format!("{} / {}", cursor.position(), cursor.len()));
            let next = ui.add_enabled(can_step, egui::Button::new("▶"));
            if next.on_hover_text("Next location").clicked() {
                action = Some(HeaderAction::Step(1));
            }
            let context = location.and_then(|l| describe(ui, graph, l));
            action.or(context)
        })
        .inner
    }
}

/// "declaration", or "call in <link to the calling symbol>".
fn describe(ui: &mut egui::Ui, graph: &CodeGraph, location: &Location) -> Option<HeaderAction> {
    ui.separator();
    let OccurrenceRole::Reference(kind) = location.occurrence.role else {
        ui.weak("declaration");
        return None;
    };
    let context = location
        .occurrence
        .context
        .and_then(|id| graph.symbols.get(&id));
    let Some(context) = context else {
        ui.weak(kind.label());
        return None;
    };
    ui.weak(format!("{} in", kind.label()));
    let link = ui.link(RichText::new(&context.qualified_name).monospace());
    link.on_hover_text("Explore this symbol")
        .clicked()
        .then_some(HeaderAction::Select(context.id))
}

fn focus_for<'a>(
    symbol: &'a Symbol,
    location: Option<&'a Location>,
    should_scroll: bool,
) -> Focus<'a> {
    match location {
        Some(l) => Focus {
            file: &l.file,
            line: l.occurrence.span.line,
            span: Some(l.occurrence.span),
            selected: Some(symbol.id),
            should_scroll,
        },
        None => Focus {
            file: &symbol.file,
            line: symbol.line,
            span: None,
            selected: Some(symbol.id),
            should_scroll,
        },
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Modifiers, PointerButton, Pos2};
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;
    use crate::index::{build_graph, Extractor, SourceFile};

    const SOURCE: &str = "int helper() { return 1; }\n\
                          int first() { return helper(); }\n\
                          int second() { return helper() + helper(); }\n";

    const HARNESS_HEIGHT: f32 = 400.0;
    const SCROLL_ANIMATION_STEPS: usize = 30;

    struct State {
        panel: CodePanel,
        graph: CodeGraph,
        selected: Option<SymbolId>,
        clicked: Option<SymbolId>,
    }

    fn graph_of(source: &str) -> CodeGraph {
        let facts = Extractor::new()
            .expect("grammar")
            .extract(source)
            .expect("parse");
        build_graph(vec![SourceFile {
            path: "a.cpp".into(),
            contents: source.into(),
            facts,
        }])
    }

    fn id(graph: &CodeGraph, name: &str) -> SymbolId {
        graph
            .symbols
            .values()
            .find(|s| s.qualified_name == name)
            .map(|s| s.id)
            .unwrap_or_else(|| panic!("no symbol {name}"))
    }

    fn harness(selected: &str) -> Harness<'static, State> {
        harness_for(SOURCE, selected)
    }

    fn harness_for(source: &str, selected: &str) -> Harness<'static, State> {
        let graph = graph_of(source);
        let state = State {
            panel: CodePanel::default(),
            selected: Some(id(&graph, selected)),
            graph,
            clicked: None,
        };
        // ◀ ▶ come from the system font in the app; the harness only has
        // egui's bundled fonts.
        let mut harness = Harness::builder()
            .allow_missing_glyphs()
            .with_size(egui::vec2(800.0, HARNESS_HEIGHT))
            .build_ui_state(
                |ui, state: &mut State| {
                    if let Some(id) = state.panel.show(ui, &state.graph, state.selected) {
                        state.clicked = Some(id);
                    }
                },
                state,
            );
        harness.run();
        harness
    }

    fn click_at(harness: &mut Harness<State>, pos: Pos2) {
        harness.hover_at(pos);
        for pressed in [true, false] {
            harness.event(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        harness.run();
    }

    #[test]
    fn starts_at_the_definition_and_steps_through_call_sites() {
        let mut harness = harness("helper");
        harness.get_by_label("1 / 4");
        harness.get_by_label("declaration");

        harness.get_by_label("▶").click();
        harness.run();
        harness.get_by_label("2 / 4");
        harness.get_by_label("call in");

        harness.get_by_label("◀").click();
        harness.run();
        harness.get_by_label("◀").click();
        harness.run();
        harness.get_by_label("4 / 4");
    }

    #[test]
    fn the_calling_symbol_link_explores_it() {
        let mut harness = harness("helper");
        harness.get_by_label("▶").click();
        harness.run();
        harness.get_by_label("first").click();
        harness.run();
        let first = id(&harness.state().graph, "first");
        assert_eq!(harness.state().clicked, Some(first));
    }

    #[test]
    fn clicking_a_name_in_the_source_explores_it() {
        let mut harness = harness("second");
        let line = SOURCE.lines().nth(2).expect("third line");
        let rect = harness.get_by_label(line).rect();
        // Monospace: aim at the middle of the first `helper` on that line.
        let column = line.find("helper").expect("call") + 3;
        let char_width = rect.width() / line.chars().count() as f32;
        let pos = Pos2::new(
            rect.min.x + char_width * (column as f32 + 0.5),
            rect.center().y,
        );
        click_at(&mut harness, pos);
        let helper = id(&harness.state().graph, "helper");
        assert_eq!(harness.state().clicked, Some(helper));
    }

    #[test]
    fn clicking_plain_code_does_nothing() {
        let mut harness = harness("second");
        let line = SOURCE.lines().nth(2).expect("third line");
        let rect = harness.get_by_label(line).rect();
        click_at(&mut harness, Pos2::new(rect.min.x + 2.0, rect.center().y)); // on `int`
        assert_eq!(harness.state().clicked, None);
    }

    #[test]
    fn clicking_another_occurrence_of_the_selected_symbol_focuses_it() {
        let mut harness = harness("helper");
        let line = SOURCE.lines().nth(2).expect("third line");
        let rect = harness.get_by_label(line).rect();
        let column = line.rfind("helper").expect("second call") + 3;
        let char_width = rect.width() / line.chars().count() as f32;
        let pos = Pos2::new(
            rect.min.x + char_width * (column as f32 + 0.5),
            rect.center().y,
        );
        click_at(&mut harness, pos);
        assert_eq!(harness.state().clicked, None);
        harness.get_by_label("4 / 4");
    }

    #[test]
    fn stepping_to_a_distant_call_scrolls_it_into_view() {
        let filler = "// filler\n".repeat(150);
        let source =
            format!("int helper() {{ return 1; }}\n{filler}int far() {{ return helper(); }}\n");
        let mut harness = harness_for(&source, "helper");
        assert!(harness
            .query_by_label("int far() { return helper(); }")
            .is_none());

        harness.get_by_label("▶").click();
        harness.run_steps(SCROLL_ANIMATION_STEPS); // scrolling is animated
        let rect = harness
            .get_by_label("int far() { return helper(); }")
            .rect();
        assert!(rect.max.y <= HARNESS_HEIGHT, "focused line at {rect:?}");
    }

    #[test]
    fn stepping_to_a_visible_location_does_not_scroll() {
        let mut harness = harness("helper");
        let first_line = SOURCE.lines().next().expect("first line");
        let before = harness.get_by_label(first_line).rect();
        harness.get_by_label("▶").click();
        harness.run();
        assert_eq!(harness.get_by_label(first_line).rect(), before);
    }
}

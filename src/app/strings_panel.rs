//! The Strings pane: every string literal in the project, once each, with
//! how often it is used. Pick one to list where it is written; click a
//! place to show it in the source.

use egui::{FontId, RichText};

use super::layout::Layout;
use super::pane_bar::{self, Edge};
use super::strings::{listed, StringEntry, StringOrder, StringUse};
use super::visible::{self, visible};
use crate::index::{MAX_STRINGS_PER_FILE, MAX_STRINGS_TOTAL};
use crate::model::{CodeGraph, Span, SymbolId};

const DEFAULT_WIDTH: f32 = 380.0;
/// The places list's height until dragged, and its bounds.
const USES_HEIGHT: f32 = 200.0;
const MIN_USES_HEIGHT: f32 = 80.0;
/// The most of the pane's height the places list may take.
const MAX_USES_SHARE: f32 = 0.6;

/// What the user picked in the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StringsPanelAction {
    /// Show this place in the source.
    GoTo {
        symbol: Option<SymbolId>,
        file: String,
        span: Span,
    },
}

#[derive(Default)]
pub struct StringsPanel {
    is_open: bool,
    /// The project's strings, each once.
    entries: Vec<StringEntry>,
    query: String,
    order: StringOrder,
    /// The strings listed (indices into `entries`), for `rows_for`.
    rows: Vec<usize>,
    /// The filter and order `rows` were computed for; `None` means stale.
    rows_for: Option<(String, StringOrder)>,
    /// The string whose places are listed, by index into `entries`.
    selected: Option<usize>,
}

impl StringsPanel {
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn set_open(&mut self, is_open: bool) {
        self.is_open = is_open;
    }

    /// Lists `entries` (a new project's), forgetting the string picked.
    /// Stays open or closed, with the same filter and order.
    pub fn set_strings(&mut self, entries: Vec<StringEntry>) {
        self.entries = entries;
        self.rows_for = None;
        self.selected = None;
    }

    /// Draws the pane (while open, sliding in and out) at the right of
    /// `ui`.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        layout: Layout,
    ) -> Option<StringsPanelAction> {
        let mut is_open = self.is_open;
        let shown = pane_bar::right_pane(layout.pane_id("strings_panel"), DEFAULT_WIDTH)
            .show_collapsible(ui, &mut is_open, |ui| self.contents(ui, graph, layout));
        // Shut by dragging, or by its hide arrow.
        self.is_open = is_open && self.is_open;
        shown.and_then(|shown| shown.inner)
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        layout: Layout,
    ) -> Option<StringsPanelAction> {
        let title = format!("Strings ({})", self.entries.len());
        if pane_bar::heading(ui, title, Edge::Right) {
            self.is_open = false;
        }
        self.controls(ui);
        cut_short_note(ui, &graph.strings_cut_short);
        ui.separator();
        self.refresh();
        if self.rows.is_empty() {
            ui.weak(if self.entries.is_empty() {
                "No string literals in this project."
            } else {
                "No matching strings."
            });
            return None;
        }
        // The places first, at the bottom; the strings take the rest.
        let picked = self.selected.and_then(|index| self.entries.get(index));
        let action = picked.and_then(|entry| show_uses(ui, entry, graph, layout));
        self.string_rows(ui);
        action
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let filter = egui::TextEdit::singleline(&mut self.query)
            .hint_text("Filter strings…")
            .desired_width(f32::INFINITY);
        ui.add(filter);
        ui.horizontal(|ui| {
            for order in StringOrder::ALL {
                ui.selectable_value(&mut self.order, order, order.label());
            }
        });
    }

    /// Recomputes the rows if the filter or order changed, letting go of a
    /// picked string the filter now hides.
    fn refresh(&mut self) {
        let is_current = self
            .rows_for
            .as_ref()
            .is_some_and(|(query, order)| *query == self.query && *order == self.order);
        if is_current {
            return;
        }
        self.rows = listed(&self.entries, &self.query, self.order);
        self.rows_for = Some((self.query.clone(), self.order));
        if self.selected.is_some_and(|s| !self.rows.contains(&s)) {
            self.selected = None;
        }
    }

    /// The strings, one per row: how many places use it, then the string.
    fn string_rows(&mut self, ui: &mut egui::Ui) {
        let row_height = ui.spacing().interact_size.y;
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let color = ui.visuals().text_color();
        let (rows, entries, selected) = (&self.rows, &self.entries, self.selected);
        let clicked = egui::ScrollArea::vertical()
            .id_salt("strings_rows")
            .auto_shrink([false, false])
            .show_rows(ui, row_height, rows.len(), |ui, range| {
                // Rows must stay one line high for `show_rows` to place them.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                let mut clicked = None;
                for &index in &rows[range] {
                    let Some(entry) = entries.get(index) else {
                        continue;
                    };
                    let label = row_label(entry, font.clone(), color);
                    if ui
                        .selectable_label(selected == Some(index), label)
                        .clicked()
                    {
                        clicked = Some(index);
                    }
                }
                clicked
            })
            .inner;
        if clicked.is_some() {
            self.selected = clicked;
        }
    }
}

/// `   3×  "the string"`, its hidden characters marked.
fn row_label(entry: &StringEntry, font: FontId, color: egui::Color32) -> egui::text::LayoutJob {
    let mut job = visible::job(&format!("{:>4}×  ", entry.uses.len()), font.clone(), color);
    visible::append(&mut job, &entry.text, font, color);
    job
}

/// Where `entry` is written, in a list at the bottom of `ui`. Returns the
/// place clicked.
fn show_uses(
    ui: &mut egui::Ui,
    entry: &StringEntry,
    graph: &CodeGraph,
    layout: Layout,
) -> Option<StringsPanelAction> {
    let most = (ui.available_height() * MAX_USES_SHARE).max(MIN_USES_HEIGHT);
    egui::Panel::bottom(layout.pane_id("strings_uses"))
        .resizable(true)
        .default_size(USES_HEIGHT)
        .size_range(MIN_USES_HEIGHT..=most)
        .show(ui, |ui| {
            ui.add_space(ui.spacing().item_spacing.y);
            let font = egui::TextStyle::Monospace.resolve(ui.style());
            ui.label(visible::job(&entry.text, font, ui.visuals().text_color()));
            ui.weak(match entry.uses.len() {
                1 => "Written in 1 place:".to_string(),
                n => format!("Written in {n} places:"),
            });
            egui::ScrollArea::vertical()
                .id_salt("strings_uses_rows")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    let mut picked = None;
                    for place in &entry.uses {
                        if ui
                            .selectable_label(false, place_label(place, graph))
                            .clicked()
                        {
                            picked = Some(StringsPanelAction::GoTo {
                                symbol: place.function,
                                file: place.file.clone(),
                                span: place.span,
                            });
                        }
                    }
                    picked
                })
                .inner
        })
        .inner
}

/// `file:line  function`, or `file:line  (file scope)`; names from the
/// project with their hidden characters marked.
fn place_label(place: &StringUse, graph: &CodeGraph) -> String {
    let function = place
        .function
        .and_then(|id| graph.symbols.get(&id))
        .map_or("(file scope)", |s| s.qualified_name.as_str());
    format!(
        "{}:{}  {}",
        visible(&place.file),
        place.span.line,
        visible(function)
    )
}

/// Says which files' strings weren't all kept, if any.
fn cut_short_note(ui: &mut egui::Ui, files: &[String]) {
    if files.is_empty() {
        return;
    }
    let count = match files.len() {
        1 => "1 file's strings weren't".to_string(),
        n => format!("{n} files' strings weren't"),
    };
    let note = format!(
        "{count} all kept: at most {MAX_STRINGS_PER_FILE} per file and \
         {MAX_STRINGS_TOTAL} in all."
    );
    let names: Vec<String> = files.iter().map(|f| visible(f).into_owned()).collect();
    ui.label(RichText::new(note).weak())
        .on_hover_text(names.join("\n"));
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;
    use crate::app::strings::entries;
    use crate::index::graph_of_files;

    struct State {
        panel: StringsPanel,
        graph: CodeGraph,
        action: Option<StringsPanelAction>,
    }

    fn harness_of(graph: CodeGraph) -> Harness<'static, State> {
        let mut panel = StringsPanel::default();
        panel.set_open(true);
        panel.set_strings(entries(&graph));
        let state = State {
            panel,
            graph,
            action: None,
        };
        let mut harness = crate::app::fonts::test_harness()
            .with_size(egui::vec2(500.0, 600.0))
            .build_ui_state(
                |ui, state: &mut State| {
                    let action = state.panel.show(ui, &state.graph, Layout::default());
                    if action.is_some() {
                        state.action = action;
                    }
                },
                state,
            );
        harness.run();
        harness
    }

    fn harness() -> Harness<'static, State> {
        harness_of(graph_of_files(&[(
            "net.c",
            "const char *banner = \"hello\";\n\
             int greet(void) {\n  return puts(\"hello\");\n}\n\
             int fail(void) {\n  return puts(\"bad packet\");\n}\n",
        )]))
    }

    #[test]
    fn lists_each_string_once_with_how_often_it_is_used() {
        let harness = harness();
        harness.get_by_label("Strings (2)");
        harness.get_by_label_contains("2×  \"hello\"");
        harness.get_by_label_contains("1×  \"bad packet\"");
    }

    #[test]
    fn the_filter_narrows_the_list() {
        let mut harness = harness();
        harness.get_by_role(egui::Role::TextInput).focus();
        harness.run();
        harness
            .get_by_role(egui::Role::TextInput)
            .type_text("PACKET");
        harness.run();
        harness.get_by_label_contains("\"bad packet\"");
        assert!(harness.query_by_label_contains("\"hello\"").is_none());
    }

    #[test]
    fn picking_a_string_lists_its_places_and_a_place_goes_there() {
        let mut harness = harness();
        harness.get_by_label_contains("2×  \"hello\"").click();
        harness.run();
        harness.get_by_label("Written in 2 places:");
        harness.get_by_label("net.c:1  (file scope)");
        harness.get_by_label("net.c:3  greet").click();
        harness.run();
        let Some(StringsPanelAction::GoTo { symbol, file, span }) = harness.state().action.clone()
        else {
            panic!("expected a place");
        };
        let greet = harness
            .state()
            .graph
            .symbols
            .get(&symbol.expect("in greet"));
        assert_eq!(greet.map(|s| s.qualified_name.as_str()), Some("greet"));
        assert_eq!((file.as_str(), span.line, span.start), ("net.c", 3, 14));
    }

    #[test]
    fn a_project_without_strings_says_so() {
        let harness = harness_of(graph_of_files(&[("a.c", "int f(void) { return 0; }\n")]));
        harness.get_by_label("Strings (0)");
        harness.get_by_label("No string literals in this project.");
    }

    #[test]
    fn its_arrow_hides_it() {
        let mut harness = harness();
        harness.get_by_label("▸").click();
        harness.run();
        assert!(!harness.state().panel.is_open());
    }

    #[test]
    fn files_cut_short_are_mentioned() {
        let graph = CodeGraph {
            strings_cut_short: vec!["big_table.c".into()],
            ..graph_of_files(&[("a.c", "const char *s = \"x\";\n")])
        };
        let harness = harness_of(graph);
        harness.get_by_label_contains("1 file's strings weren't all kept");
    }

    fn filter_by(harness: &mut Harness<'static, State>, text: &str) {
        harness.get_by_role(egui::Role::TextInput).focus();
        harness.run();
        harness.get_by_role(egui::Role::TextInput).type_text(text);
        harness.run();
    }

    #[test]
    fn a_picked_string_the_filter_hides_is_let_go() {
        let mut harness = harness();
        harness.get_by_label_contains("2×  \"hello\"").click();
        harness.run();
        harness.get_by_label("Written in 2 places:");
        filter_by(&mut harness, "packet");
        assert!(harness.query_by_label("Written in 2 places:").is_none());
        assert_eq!(harness.state().panel.selected, None);
    }

    #[test]
    fn hidden_characters_in_strings_and_names_are_marked() {
        let mut harness = harness_of(graph_of_files(&[(
            "invoice\u{202E}fdp.c",
            "int f(void) {\n  return puts(R\"(two\nlines)\");\n}\n",
        )]));
        harness.get_by_label_contains("‹LF›").click();
        harness.run();
        harness.get_by_label("invoice‹U+202E›fdp.c:2  f");
        assert!(harness.query_by_label_contains("\u{202E}").is_none());
    }
}

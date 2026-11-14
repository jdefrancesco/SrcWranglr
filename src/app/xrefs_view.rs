//! The Cross References window (X over a name, as in IDA): every place a
//! symbol is referenced or a library function called, filterable; click
//! one to go there.

use std::collections::{BTreeMap, HashMap};

use super::visible::{self, visible};
use super::xrefs::{name_of, xrefs, Xref};
use crate::code_view::Referenced;
use crate::model::{CodeGraph, Span, SymbolId};

const DEFAULT_SIZE: [f32; 2] = [760.0, 460.0];

/// What the user asked of the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XrefAction {
    GoTo {
        symbol: Option<SymbolId>,
        file: String,
        span: Span,
    },
}

struct Row {
    xref: Xref,
    /// `file:line`.
    place: String,
    /// The function it is in, as named.
    function: String,
    /// Its place and function, lowercase, to filter by.
    key: String,
}

pub struct XrefsView {
    name: String,
    rows: Vec<Row>,
    /// "12 calls, 1 declaration".
    summary: String,
    is_cut_short: bool,
    filter: String,
    /// The rows the filter keeps, and the filter they are for.
    shown: Vec<usize>,
    shown_for: Option<String>,
    /// Each file's line starts, and each row's code, once drawn.
    line_starts: HashMap<String, Vec<usize>>,
    snippets: HashMap<usize, String>,
    /// Whether a field had the keyboard as the last frame ended: egui lets
    /// go of it as Escape comes in, before the window sees the key.
    was_typing: bool,
}

/// A row's code is cut to this many characters.
const MAX_SNIPPET_CHARS: usize = 200;

impl XrefsView {
    /// What the references are to: `helper`, `memcpy()`.
    #[cfg(test)]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn open(graph: &CodeGraph, target: &Referenced) -> Self {
        let (found, is_cut_short) = xrefs(graph, target);
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        for xref in &found {
            *kinds.entry(xref.kind).or_default() += 1;
        }
        let summary = kinds
            .iter()
            .map(|(kind, n)| format!("{n} {kind}{}", if *n == 1 { "" } else { "s" }))
            .collect::<Vec<_>>()
            .join(", ");
        let rows = found
            .into_iter()
            .map(|xref| {
                let place = format!("{}:{}", xref.file, xref.span.line);
                let function = xref
                    .function
                    .and_then(|id| graph.symbols.get(&id))
                    .map_or(String::new(), |s| s.qualified_name.clone());
                let key = format!("{place} {function}").to_lowercase();
                Row {
                    xref,
                    place,
                    function,
                    key,
                }
            })
            .collect();
        Self {
            name: name_of(graph, target),
            rows,
            summary,
            is_cut_short,
            filter: String::new(),
            shown: Vec::new(),
            shown_for: None,
            line_starts: HashMap::new(),
            snippets: HashMap::new(),
            was_typing: false,
        }
    }

    /// Draws the window; `None` once it is closed, else what was asked.
    pub fn show(&mut self, ctx: &egui::Context, graph: &CodeGraph) -> Option<Option<XrefAction>> {
        let mut is_open = true;
        let mut action = None;
        let shown = egui::Window::new(format!("Cross References to {}", visible(&self.name)))
            .id(egui::Id::unique("cross_references"))
            .open(&mut is_open)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .default_size(DEFAULT_SIZE)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| action = self.contents(ui, graph));
        let is_pointed_at = shown.is_some_and(|s| s.response.contains_pointer());
        // A field with the keyboard (this filter, another window's) gets
        // Escape first.
        let is_dismissed = is_pointed_at
            && !self.was_typing
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        self.was_typing = ctx.memory(|m| m.focused().is_some());
        (is_open && !is_dismissed).then_some(action)
    }

    fn contents(&mut self, ui: &mut egui::Ui, graph: &CodeGraph) -> Option<XrefAction> {
        if self.rows.is_empty() {
            ui.label("No references in the project.");
            return None;
        }
        let count = match self.rows.len() {
            1 => "1 reference".to_string(),
            n => format!("{n} references"),
        };
        ui.label(format!("{count}: {}", self.summary));
        if self.is_cut_short {
            ui.weak("Only the first ones are listed.");
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.filter).hint_text("Filter by file or function…"),
        );
        self.refilter();
        let row_height = ui.spacing().interact_size.y;
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let color = ui.visuals().text_color();
        let weak = ui.visuals().weak_text_color();
        let shown = std::mem::take(&mut self.shown);
        let picked = egui::ScrollArea::vertical()
            .id_salt("cross_reference_rows")
            .auto_shrink([false, false])
            .show_rows(ui, row_height, shown.len(), |ui, range| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                let mut picked = None;
                for &index in &shown[range] {
                    let code = self.snippet(graph, index);
                    let row = &self.rows[index];
                    let kind = format!("{:<12}", row.xref.kind);
                    let mut label = visible::job(&kind, font.clone(), weak);
                    visible::append(&mut label, &format!("{}  ", row.place), font.clone(), color);
                    if !row.function.is_empty() {
                        let function = format!("{}  ", row.function);
                        visible::append(&mut label, &function, font.clone(), weak);
                    }
                    visible::append(&mut label, &code, font.clone(), color);
                    if ui.selectable_label(false, label).clicked() {
                        picked = Some(XrefAction::GoTo {
                            symbol: row.xref.function,
                            file: row.xref.file.clone(),
                            span: row.xref.span,
                        });
                    }
                }
                picked
            })
            .inner;
        self.shown = shown;
        picked
    }

    /// The rows the filter keeps, worked out again only when it changes.
    fn refilter(&mut self) {
        let needle = self.filter.to_lowercase();
        if self.shown_for.as_deref() == Some(needle.as_str()) {
            return;
        }
        self.shown = (0..self.rows.len())
            .filter(|&i| needle.is_empty() || self.rows[i].key.contains(&needle))
            .collect();
        self.shown_for = Some(needle);
    }

    /// Row `index`'s line of code, trimmed and cut short, found once.
    fn snippet(&mut self, graph: &CodeGraph, index: usize) -> String {
        if let Some(code) = self.snippets.get(&index) {
            return code.clone();
        }
        let xref = &self.rows[index].xref;
        let text = graph
            .file_contents
            .get(&xref.file)
            .map_or("", String::as_str);
        let starts = self
            .line_starts
            .entry(xref.file.clone())
            .or_insert_with(|| line_starts(text));
        let row = xref.span.line.saturating_sub(1) as usize;
        let line = starts.get(row).and_then(|&start| {
            let end = starts.get(row + 1).map_or(text.len(), |&next| next);
            text.get(start..end)
        });
        let code: String = line
            .unwrap_or("")
            .trim()
            .chars()
            .take(MAX_SNIPPET_CHARS)
            .collect();
        self.snippets.insert(index, code.clone());
        code
    }
}

/// Where each line of `text` starts, in bytes.
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .filter(|&start| start < text.len())
        .collect()
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;
    use crate::index::graph_of_files;

    struct State {
        view: XrefsView,
        graph: CodeGraph,
        actions: Vec<XrefAction>,
        is_open: bool,
    }

    fn harness(target: Referenced) -> Harness<'static, State> {
        let source = "void a(char *d) { memcpy(d, d, 4); }\nvoid b(char *e) { memcpy(e, e, 8); }\n";
        let graph = graph_of_files(&[("x.c", source)]);
        let view = XrefsView::open(&graph, &target);
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
            |ui, state: &mut State| {
                let ctx = ui.ctx().clone();
                if !state.is_open {
                    return;
                }
                match state.view.show(&ctx, &state.graph) {
                    Some(Some(action)) => state.actions.push(action),
                    Some(None) => {}
                    None => state.is_open = false,
                }
            },
            State {
                view,
                graph,
                actions: Vec::new(),
                is_open: true,
            },
        );
        harness.run();
        harness
    }

    #[test]
    fn lists_each_call_and_goes_to_the_one_clicked() {
        let mut harness = harness(Referenced::Library("memcpy".into()));
        harness.get_by_label("2 references: 2 calls");
        harness
            .get_by_label_contains("x.c:2  b  void b(char *e) { memcpy(e, e, 8); }")
            .click();
        harness.run();
        assert!(matches!(
            harness.state().actions.as_slice(),
            [XrefAction::GoTo { file, span, .. }] if file == "x.c" && span.line == 2
        ));
    }

    #[test]
    fn the_filter_keeps_matching_places() {
        let mut harness = harness(Referenced::Library("memcpy".into()));
        harness.state_mut().view.filter = "x.c:1".into();
        harness.run();
        assert!(harness
            .query_by_label_contains("memcpy(e, e, 8);")
            .is_none());
        harness.get_by_label_contains("memcpy(d, d, 4);");
    }

    /// The harness, and whether the window is still open after Escape.
    fn escape_closes(harness: &mut Harness<'static, State>) -> bool {
        harness.key_press(egui::Key::Escape);
        harness.run();
        harness.query_by_label_contains("references:").is_some()
    }

    #[test]
    fn escape_leaves_the_filter_first_and_closes_only_the_window_pointed_at() {
        let mut harness = harness(Referenced::Library("memcpy".into()));
        harness.get_by_role(egui::Role::TextInput).click();
        harness.run();
        assert!(escape_closes(&mut harness), "typing: the field gets Escape");
        harness.get_by_label_contains("memcpy(d, d, 4);").hover();
        harness.run();
        assert!(
            !escape_closes(&mut harness),
            "pointed at, not typing: it closes"
        );
    }

    #[test]
    fn nothing_to_list_says_so() {
        let harness = harness(Referenced::Library("strcpy".into()));
        harness.get_by_label("No references in the project.");
    }
}

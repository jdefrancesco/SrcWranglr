//! The bottom panel: a header listing where the selected symbol appears
//! (◀ 3 / 12 ▶ · call in `Foo::bar`), above the source at that location.

use std::collections::BTreeMap;

use egui::RichText;

use super::findings::Findings;
use super::locations::LocationCursor;
use super::notebook::Notebook;
use crate::code_view::{CodeEvent, CodeView, Focus, LineAction, LineMark, LineWarning};
use crate::model::{CodeGraph, Location, OccurrenceRole, Symbol, SymbolId};
use crate::notes::NoteTarget;
use crate::style::severity_color;

/// Tallest the selected symbol's note gets above the code (it scrolls).
const NOTE_MAX_HEIGHT: f32 = 72.0;

enum HeaderAction {
    Step(isize),
    Select(SymbolId),
    OpenInEditor,
    ToggleBookmark,
    EditNote,
}

/// What the user asked for in the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelAction {
    /// Explore a symbol (a name in the source, or the enclosing symbol).
    Explore(SymbolId),
    /// Open the current location in an external editor.
    OpenInEditor,
    /// Star or unstar the symbol or a line.
    ToggleBookmark(NoteTarget),
    /// Open the note editor for the symbol or a line.
    EditNote(NoteTarget),
}

/// A line to show instead of the selected symbol's current location
/// (e.g. a line note's line), until the user steps or selects elsewhere.
struct Reveal {
    symbol: SymbolId,
    file: String,
    line: u32,
}

/// A place in a project file; `line` and `column` are 1-based, the column
/// counted in characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePosition {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

#[derive(Default)]
pub struct CodePanel {
    view: CodeView,
    cursor: Option<LocationCursor>,
    /// The focus moved since the last frame, so scroll it into view.
    should_scroll: bool,
    reveal: Option<Reveal>,
}

impl CodePanel {
    /// Shows the selected symbol's current location, with its note and
    /// the notes of the lines shown. Returns what the user asked for.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        selected: Option<SymbolId>,
        notebook: &Notebook,
        findings: &Findings,
    ) -> Option<PanelAction> {
        self.sync(graph, selected);
        let Some(symbol) = selected.and_then(|id| graph.symbols.get(&id)) else {
            ui.centered_and_justified(|ui| ui.label("Select a symbol to view its source."));
            return None;
        };
        let symbol_target = NoteTarget::Symbol(symbol.qualified_name.clone());
        let note = notebook.notes().get(&symbol_target);
        let is_starred = note.is_some_and(|n| n.is_starred);
        let header_action = self.show_header(ui, graph, symbol, is_starred);
        if let Some(text) = note.map(|n| n.text.as_str()).filter(|t| !t.is_empty()) {
            show_note(ui, text);
        }
        ui.separator();
        if let Some(HeaderAction::Step(delta)) = header_action {
            self.cursor = self.cursor.take().map(|c| c.stepped(delta));
            self.reveal = None;
            self.should_scroll = true;
        }
        let event = self.show_code(ui, graph, symbol, notebook, findings);
        match header_action {
            Some(HeaderAction::Select(id)) => return Some(PanelAction::Explore(id)),
            Some(HeaderAction::OpenInEditor) => return Some(PanelAction::OpenInEditor),
            Some(HeaderAction::ToggleBookmark) => {
                return Some(PanelAction::ToggleBookmark(symbol_target))
            }
            Some(HeaderAction::EditNote) => return Some(PanelAction::EditNote(symbol_target)),
            Some(HeaderAction::Step(_)) | None => {}
        }
        let (file, event) = event?;
        match event {
            CodeEvent::Line(line, action) => {
                let target = NoteTarget::Line { file, line };
                Some(match action {
                    LineAction::EditNote => PanelAction::EditNote(target),
                    LineAction::ToggleBookmark => PanelAction::ToggleBookmark(target),
                })
            }
            CodeEvent::Clicked(clicked) if clicked.symbol != symbol.id => {
                Some(PanelAction::Explore(clicked.symbol))
            }
            CodeEvent::Clicked(clicked) => {
                // Another occurrence of the selected symbol: make it the
                // current location.
                self.cursor = self
                    .cursor
                    .take()
                    .map(|c| c.focused_on(&file, clicked.span));
                self.reveal = None;
                None
            }
        }
    }

    /// Shows `file:line` for `symbol` (which the caller selects) instead of
    /// one of its locations.
    pub fn reveal(&mut self, symbol: SymbolId, file: String, line: u32) {
        self.reveal = Some(Reveal { symbol, file, line });
        self.should_scroll = true;
    }

    /// Draws the source at the focus. Returns the file shown and what the
    /// user did in it.
    fn show_code(
        &mut self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        symbol: &Symbol,
        notebook: &Notebook,
        findings: &Findings,
    ) -> Option<(String, CodeEvent)> {
        let location = self.cursor.as_ref().and_then(LocationCursor::current);
        let reveal = self.reveal.as_ref().filter(|r| r.symbol == symbol.id);
        let file = match (reveal, location) {
            (Some(r), _) => r.file.as_str(),
            (None, Some(l)) => l.file.as_str(),
            (None, None) => symbol.file.as_str(),
        };
        let line_marks = line_marks_for(file, notebook, findings);
        let focus = match reveal {
            Some(r) => Focus {
                file: &r.file,
                line: r.line,
                span: None,
                selected: Some(symbol.id),
                should_scroll: self.should_scroll,
                line_marks: &line_marks,
            },
            None => focus_for(symbol, location, self.should_scroll, &line_marks),
        };
        let event = self.view.show(ui, graph, &focus);
        let file = focus.file.to_string();
        self.should_scroll = false;
        event.map(|e| (file, e))
    }

    /// Where the panel is: the current location of `selected`, else its
    /// definition line. `None` if nothing is selected.
    pub fn position(&self, graph: &CodeGraph, selected: Option<SymbolId>) -> Option<FilePosition> {
        let symbol = graph.symbols.get(&selected?)?;
        if let Some(reveal) = self.reveal.as_ref().filter(|r| r.symbol == symbol.id) {
            return Some(FilePosition {
                file: reveal.file.clone(),
                line: reveal.line,
                column: 1,
            });
        }
        let location = self
            .cursor
            .as_ref()
            .filter(|c| c.symbol() == symbol.id)
            .and_then(LocationCursor::current);
        let Some(location) = location else {
            return Some(FilePosition {
                file: symbol.file.clone(),
                line: symbol.line,
                column: 1,
            });
        };
        let span = location.occurrence.span;
        Some(FilePosition {
            file: location.file.clone(),
            line: span.line,
            column: text_column(graph, &location.file, span.line, span.start),
        })
    }

    /// Starts a fresh location list whenever the selection changes.
    fn sync(&mut self, graph: &CodeGraph, selected: Option<SymbolId>) {
        if self.cursor.as_ref().map(LocationCursor::symbol) != selected {
            self.cursor = selected.map(|id| LocationCursor::new(graph, id));
            self.should_scroll = true;
        }
        if self.reveal.as_ref().map(|r| r.symbol) != selected {
            self.reveal = None;
        }
    }

    fn show_header(
        &self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        symbol: &Symbol,
        is_starred: bool,
    ) -> Option<HeaderAction> {
        let cursor = self.cursor.as_ref();
        let location = cursor.and_then(LocationCursor::current);
        let file = location.map_or(symbol.file.as_str(), |l| l.file.as_str());
        ui.horizontal(|ui| {
            ui.label(RichText::new(file).strong().monospace());
            let open = ui.small_button("Open in Editor");
            if open
                .on_hover_text("Open this location in your editor")
                .clicked()
            {
                return Some(HeaderAction::OpenInEditor);
            }
            let star = ui.small_button(if is_starred { "★" } else { "☆" });
            if star.on_hover_text("Bookmark this symbol").clicked() {
                return Some(HeaderAction::ToggleBookmark);
            }
            let note = ui.small_button("Note…");
            if note.on_hover_text("Write a note on this symbol").clicked() {
                return Some(HeaderAction::EditNote);
            }
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

/// 1-based character column of byte column `byte` on `line` of `file`.
fn text_column(graph: &CodeGraph, file: &str, line: u32, byte: u32) -> u32 {
    let text = graph
        .file_contents
        .get(file)
        .and_then(|contents| contents.lines().nth(line.saturating_sub(1) as usize));
    let chars = text.map_or(byte as usize, |text| {
        let end = (byte as usize).min(text.len());
        // Snap to a character boundary, so a bad span can't panic.
        let end = (0..=end)
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        text[..end].chars().count()
    });
    u32::try_from(chars).unwrap_or(u32::MAX).saturating_add(1)
}

/// The gutter marks of `file`: its line notes and bookmarks, and the most
/// severe finding on each line.
fn line_marks_for<'a>(
    file: &str,
    notebook: &'a Notebook,
    findings: &'a Findings,
) -> BTreeMap<u32, LineMark<'a>> {
    let mut marks: BTreeMap<u32, LineMark> = notebook
        .line_marks(file)
        .into_iter()
        .map(|(line, (mark, note))| {
            let mark = LineMark {
                is_starred: mark.is_starred,
                note,
                warning: None,
            };
            (line, mark)
        })
        .collect();
    for (line, finding) in findings.worst_by_line(file) {
        marks.entry(line).or_default().warning = Some(LineWarning {
            title: &finding.rule,
            text: &finding.message,
            color: severity_color(finding.severity),
        });
    }
    marks
}

/// The selected symbol's note, above its code.
fn show_note(ui: &mut egui::Ui, text: &str) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("symbol_note")
            .max_height(NOTE_MAX_HEIGHT)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(text);
            });
    });
}

fn focus_for<'a>(
    symbol: &'a Symbol,
    location: Option<&'a Location>,
    should_scroll: bool,
    line_marks: &'a BTreeMap<u32, LineMark<'a>>,
) -> Focus<'a> {
    match location {
        Some(l) => Focus {
            file: &l.file,
            line: l.occurrence.span.line,
            span: Some(l.occurrence.span),
            selected: Some(symbol.id),
            should_scroll,
            line_marks,
        },
        None => Focus {
            file: &symbol.file,
            line: symbol.line,
            span: None,
            selected: Some(symbol.id),
            should_scroll,
            line_marks,
        },
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Modifiers, PointerButton, Pos2};
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;
    use crate::app::notebook::Notebook;
    use crate::app::project::Project;
    use crate::index::{build_graph, Extractor, SourceFile};
    use crate::notes::NoteTarget;

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
        is_open_requested: bool,
        notebook: Notebook,
        findings: Findings,
        last_action: Option<PanelAction>,
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
        let notebook = Notebook::open(&Project::new("t", None, graph.clone()));
        let findings = Findings::for_graph(&graph);
        let state = State {
            panel: CodePanel::default(),
            selected: Some(id(&graph, selected)),
            graph,
            clicked: None,
            is_open_requested: false,
            findings,
            notebook,
            last_action: None,
        };
        // ◀ ▶ come from the system font in the app; the harness only has
        // egui's bundled fonts.
        let mut harness = Harness::builder()
            .allow_missing_glyphs()
            .with_size(egui::vec2(800.0, HARNESS_HEIGHT))
            .build_ui_state(
                |ui, state: &mut State| {
                    let action = state.panel.show(
                        ui,
                        &state.graph,
                        state.selected,
                        &state.notebook,
                        &state.findings,
                    );
                    match &action {
                        Some(PanelAction::Explore(id)) => state.clicked = Some(*id),
                        Some(PanelAction::OpenInEditor) => state.is_open_requested = true,
                        _ => {}
                    }
                    if action.is_some() {
                        state.last_action = action;
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
    fn open_in_editor_button_asks_to_open() {
        let mut harness = harness("helper");
        harness.get_by_label("Open in Editor").click();
        harness.run();
        assert!(harness.state().is_open_requested);
        assert_eq!(harness.state().clicked, None);
    }

    #[test]
    fn position_follows_the_current_location_in_text_columns() {
        let mut harness = harness_for(
            "int hélper() { return 1; }\nint f() { /* é */ return hélper(); }\n",
            "hélper",
        );
        let position = |h: &Harness<State>| {
            let state = h.state();
            state.panel.position(&state.graph, state.selected)
        };
        let expected = |line, column| {
            Some(FilePosition {
                file: "a.cpp".into(),
                line,
                column,
            })
        };
        // `hélper` starts at byte 4 on line 1: text column 5.
        assert_eq!(position(&harness), expected(1, 5));
        harness.get_by_label("▶").click();
        harness.run();
        // Line 2: the call starts at byte 26, but `é` is two bytes: column 26.
        assert_eq!(position(&harness), expected(2, 26));
    }

    fn line(number: u32) -> NoteTarget {
        NoteTarget::Line {
            file: "a.cpp".into(),
            line: number,
        }
    }

    #[test]
    fn header_buttons_bookmark_and_annotate_the_symbol() {
        let mut harness = harness("helper");
        harness.get_by_label("☆").click();
        harness.run();
        let helper = NoteTarget::Symbol("helper".into());
        assert_eq!(
            harness.state().last_action,
            Some(PanelAction::ToggleBookmark(helper.clone()))
        );
        harness.get_by_label("Note…").click();
        harness.run();
        assert_eq!(
            harness.state().last_action,
            Some(PanelAction::EditNote(helper))
        );
    }

    #[test]
    fn a_starred_symbol_shows_a_filled_star_and_its_note() {
        let mut harness = harness("helper");
        let project = Project::new("t", None, harness.state().graph.clone());
        let target = NoteTarget::Symbol("helper".into());
        harness
            .state_mut()
            .notebook
            .set(&project, target, "always returns 1", true)
            .expect("set");
        harness.run();
        harness.get_by_label("★");
        harness.get_by_label("always returns 1");
    }

    #[test]
    fn right_clicking_a_line_offers_line_notes_and_bookmarks() {
        let mut harness = harness("helper");
        harness
            .get_by_label("int first() { return helper(); }")
            .click_secondary();
        harness.run();
        harness.get_by_label("Bookmark Line").click();
        harness.run();
        assert_eq!(
            harness.state().last_action,
            Some(PanelAction::ToggleBookmark(line(2)))
        );

        harness
            .get_by_label("int second() { return helper() + helper(); }")
            .click_secondary();
        harness.run();
        harness.get_by_label("Add Note…").click();
        harness.run();
        assert_eq!(
            harness.state().last_action,
            Some(PanelAction::EditNote(line(3)))
        );
    }

    #[test]
    fn line_menu_offers_editing_and_unbookmarking_marked_lines() {
        let mut harness = harness("helper");
        let project = Project::new("t", None, harness.state().graph.clone());
        harness
            .state_mut()
            .notebook
            .set(&project, line(2), "caller", true)
            .expect("set");
        harness.run();
        harness
            .get_by_label("int first() { return helper(); }")
            .click_secondary();
        harness.run();
        harness.get_by_label("Edit Note…");
        harness.get_by_label("Remove Bookmark").click();
        harness.run();
        assert_eq!(
            harness.state().last_action,
            Some(PanelAction::ToggleBookmark(line(2)))
        );
    }

    #[test]
    fn line_marks_combine_notes_and_the_worst_finding() {
        let source = "void f(char *d, const char *s) {\n  memcpy(d, s, 1); strcpy(d, s);\n}\n";
        let graph = graph_of(source);
        let project = Project::new("t", None, graph.clone());
        let mut notebook = Notebook::open(&project);
        notebook
            .set(&project, line(2), "checked?", true)
            .expect("set");
        notebook.set(&project, line(3), "end", false).expect("set");
        let findings = Findings::for_graph(&graph);
        let marks = line_marks_for("a.cpp", &notebook, &findings);
        let two = marks[&2];
        assert!(two.is_starred && two.note == "checked?");
        assert_eq!(two.warning.map(|w| w.title), Some("strcpy"));
        assert_eq!(marks[&3].warning, None);
        assert_eq!(marks.len(), 2);
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

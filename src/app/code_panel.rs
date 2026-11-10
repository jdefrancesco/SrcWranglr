//! The bottom panel: a header listing where the selected symbol appears
//! (◀ 3 / 12 ▶ · call in `Foo::bar`), above the source at that location.

use std::collections::{BTreeMap, HashMap};

use egui::RichText;

use super::findings::Findings;
use super::locations::LocationCursor;
use super::notebook::Notebook;
use super::pane_bar::{self, Edge};
use super::session::TraceSource;
use crate::code_view::scheme::{Scheme, SourceColors};
use crate::code_view::{CodeEvent, CodeView, Focus, LineAction, LineMark, LineWarning, RiskSpan};
use crate::model::{CodeGraph, Location, OccurrenceRole, Span, Symbol, SymbolId};
use crate::notes::NoteTarget;
use crate::style::severity_color;

/// Tallest the selected symbol's note gets above the code (it scrolls).
const NOTE_MAX_HEIGHT: f32 = 72.0;

enum HeaderAction {
    Hide,
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
    /// Explain the finding at this index of the project's findings.
    ShowFinding(usize),
    /// Add this function to the dangerous-functions list.
    MarkDangerous(String),
    /// Write a TODO about `file:line`.
    AddTodo { file: String, line: u32 },
    /// Draft a rule finding code like lines `first..=last` of `file`.
    FindSimilar { file: String, first: u32, last: u32 },
    /// Add lines `first..=last` of `file` to the rule being drafted.
    AddRulePlace { file: String, first: u32, last: u32 },
    /// Leave code like `file:line` out of the rule being drafted.
    UseAsRuleException { file: String, line: u32 },
    /// Trace where the variable at `file:line`, byte `column`, gets its value.
    Trace {
        file: String,
        line: u32,
        column: u32,
    },
    /// Trace where the value of the variable at `file:line`, byte
    /// `column`, goes.
    TraceForward {
        file: String,
        line: u32,
        column: u32,
    },
    /// Make the variable `name` at `file:line`, byte `column`, the source
    /// of a trace to a sink.
    SetTraceSource(TraceSource),
    /// Trace from the source picked to the call to `call` at `file:line`.
    TraceToSink {
        file: String,
        line: u32,
        call: String,
    },
    /// Show the docs for the function `name`.
    ShowDocs(String),
    /// List the functions most like the one holding `file:line`.
    FindLookAlikes { file: String, line: u32 },
    /// Look up the man page of the library function `name`, hovered.
    LookUpDocs(String),
    /// Its hide arrow was clicked.
    Hide,
}

/// A line to show instead of the selected symbol's current location
/// (e.g. a line note's line), until the user steps or selects elsewhere.
struct Reveal {
    symbol: SymbolId,
    file: String,
    line: u32,
    /// What to highlight on the line, if anything.
    span: Option<Span>,
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
    /// The colors the source is drawn in.
    scheme: Scheme,
    /// Where the steps of the trace path shown are, by file.
    trail: Vec<(String, Span)>,
    /// The variable picked as the source of a trace to a sink, if any.
    trace_source: Option<String>,
    /// Library functions' man page summaries, for hovering their calls.
    call_summaries: HashMap<String, Option<String>>,
    /// Lines picked by clicking their numbers: the file, the line clicked
    /// first, and the line Shift-clicked last.
    picked_lines: Option<(String, u32, u32)>,
    /// Whether a rule is being drafted, so lines' menus can add to it.
    pub is_drafting_rule: bool,
}

impl CodePanel {
    /// Draws the source in `scheme`'s colors from now on.
    pub fn set_scheme(&mut self, scheme: Scheme) {
        self.scheme = scheme;
    }

    /// Shows the selected symbol's current location, with its note and
    /// the notes of the lines shown. Returns what the user asked for.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        selected: Option<SymbolId>,
        notebook: &Notebook,
        findings: &Findings,
        edge: Edge,
    ) -> Option<PanelAction> {
        self.sync(graph, selected);
        let Some(symbol) = selected.and_then(|id| graph.symbols.get(&id)) else {
            let should_hide = ui
                .with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    pane_bar::hide_button(ui, edge)
                })
                .inner;
            ui.centered_and_justified(|ui| ui.label("Select a symbol to view its source."));
            return should_hide.then_some(PanelAction::Hide);
        };
        let symbol_target = NoteTarget::Symbol(symbol.qualified_name.clone());
        let note = notebook.notes().get(&symbol_target);
        let is_starred = note.is_some_and(|n| n.is_starred);
        let header_action = self.show_header(ui, graph, symbol, is_starred, edge);
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
            Some(HeaderAction::Hide) => return Some(PanelAction::Hide),
            Some(HeaderAction::Select(id)) => return Some(PanelAction::Explore(id)),
            Some(HeaderAction::OpenInEditor) => return Some(PanelAction::OpenInEditor),
            Some(HeaderAction::ToggleBookmark) => {
                return Some(PanelAction::ToggleBookmark(symbol_target))
            }
            Some(HeaderAction::EditNote) => return Some(PanelAction::EditNote(symbol_target)),
            Some(HeaderAction::Step(_)) | None => {}
        }
        let (file, event) = event?;
        self.event_action(file, event, symbol.id)
    }

    /// What a click or menu pick in the code asks for; clicking another
    /// occurrence of `selected` moves to it instead.
    fn event_action(
        &mut self,
        file: String,
        event: CodeEvent,
        selected: SymbolId,
    ) -> Option<PanelAction> {
        match event {
            CodeEvent::Line(line, LineAction::Trace(column)) => {
                Some(PanelAction::Trace { file, line, column })
            }
            CodeEvent::Line(line, LineAction::TraceForward(column)) => {
                Some(PanelAction::TraceForward { file, line, column })
            }
            CodeEvent::Line(line, LineAction::SetTraceSource { column, name }) => {
                Some(PanelAction::SetTraceSource(TraceSource {
                    file,
                    line,
                    column,
                    name,
                }))
            }
            CodeEvent::Line(line, LineAction::TraceToHere(call)) => {
                Some(PanelAction::TraceToSink { file, line, call })
            }
            CodeEvent::Line(_, LineAction::ShowDocs(name)) => Some(PanelAction::ShowDocs(name)),
            CodeEvent::Line(line, LineAction::FindLookAlikes) => {
                Some(PanelAction::FindLookAlikes { file, line })
            }
            CodeEvent::HoveredCall(name) => Some(PanelAction::LookUpDocs(name)),
            CodeEvent::Line(line, LineAction::AddTodo) => Some(PanelAction::AddTodo { file, line }),
            CodeEvent::Line(line, LineAction::FindSimilar) => {
                let (first, last) = self.lines_at(&file, line);
                Some(PanelAction::FindSimilar { file, first, last })
            }
            CodeEvent::Line(line, LineAction::AddRulePlace) => {
                let (first, last) = self.lines_at(&file, line);
                Some(PanelAction::AddRulePlace { file, first, last })
            }
            CodeEvent::Line(line, LineAction::UseAsRuleException) => {
                Some(PanelAction::UseAsRuleException { file, line })
            }
            CodeEvent::LineNumber { line, extend } => {
                self.pick_line(file, line, extend);
                None
            }
            CodeEvent::Line(line, action) => {
                let target = NoteTarget::Line { file, line };
                Some(match action {
                    LineAction::EditNote => PanelAction::EditNote(target),
                    LineAction::ToggleBookmark => PanelAction::ToggleBookmark(target),
                    LineAction::MarkDangerous(name) => PanelAction::MarkDangerous(name),
                    LineAction::Trace(_)
                    | LineAction::TraceForward(_)
                    | LineAction::SetTraceSource { .. }
                    | LineAction::TraceToHere(_)
                    | LineAction::ShowDocs(_)
                    | LineAction::FindLookAlikes
                    | LineAction::AddTodo
                    | LineAction::FindSimilar
                    | LineAction::AddRulePlace
                    | LineAction::UseAsRuleException => return None,
                })
            }
            CodeEvent::Risk(id) => Some(PanelAction::ShowFinding(id)),
            CodeEvent::Clicked(clicked) if clicked.symbol != selected => {
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
        self.reveal = Some(Reveal {
            symbol,
            file,
            line,
            span: None,
        });
        self.should_scroll = true;
    }

    /// Marks the steps of a trace path (where each is, by file); empty
    /// clears them.
    pub fn set_trail(&mut self, trail: Vec<(String, Span)>) {
        self.trail = trail;
    }

    /// Keeps `summary` as what hovering a call to the library function
    /// `name` shows (`None`: this system has no page for it).
    pub fn set_call_summary(&mut self, name: String, summary: Option<String>) {
        self.call_summaries.insert(name, summary);
    }

    /// What hovering a call to `name` shows, if it was handed over.
    #[cfg(test)]
    pub fn call_summary(&self, name: &str) -> Option<&Option<String>> {
        self.call_summaries.get(name)
    }

    /// Offers tracing from the variable `name` to the calls on a line,
    /// while it is the source picked (`None`: no source is).
    pub fn set_trace_source(&mut self, name: Option<String>) {
        self.trace_source = name;
    }

    /// Where the trace path's steps are in `file`.
    pub fn trail_in(&self, file: &str) -> Vec<Span> {
        self.trail
            .iter()
            .filter(|(f, _)| f == file)
            .map(|(_, span)| *span)
            .collect()
    }

    /// Highlights `span` on the revealed line (if it is on that line).
    pub fn highlight(&mut self, span: Option<Span>) {
        if let Some(reveal) = self.reveal.as_mut() {
            reveal.span = span.filter(|s| s.line == reveal.line);
        }
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
        let risks = risks_for(file, findings);
        let trail = self.trail_in(file);
        let focus = match reveal {
            Some(r) => Focus {
                file: &r.file,
                line: r.line,
                span: r.span,
                selected: Some(symbol.id),
                should_scroll: self.should_scroll,
                line_marks: &line_marks,
                risks: &risks,
                trail: &[],
                trace_source: None,
                call_summaries: None,
                selected_lines: None,
                is_drafting_rule: false,
                colors: SourceColors::default(),
            },
            None => focus_for(symbol, location, self.should_scroll, &line_marks, &risks),
        };
        let focus = Focus {
            colors: self.scheme.colors(),
            trail: &trail,
            trace_source: self.trace_source.as_deref(),
            call_summaries: Some(&self.call_summaries),
            selected_lines: self.picked_in(file),
            is_drafting_rule: self.is_drafting_rule,
            ..focus
        };
        let event = self.view.show(ui, graph, &focus);
        let file = focus.file.to_string();
        self.should_scroll = false;
        event.map(|e| (file, e))
    }

    /// Picks `line` of `file` (clicked on its number); with `extend`, the
    /// lines from the one picked first to it. Clicking the only line
    /// picked unpicks it.
    fn pick_line(&mut self, file: String, line: u32, extend: bool) {
        self.picked_lines = match self.picked_lines.take() {
            Some((picked, anchor, _)) if extend && picked == file => Some((picked, anchor, line)),
            Some((picked, anchor, other)) if picked == file && anchor == line && other == line => {
                None
            }
            _ => Some((file, line, line)),
        };
    }

    /// The lines picked in `file`, first and last.
    fn picked_in(&self, file: &str) -> Option<(u32, u32)> {
        let (picked, a, b) = self.picked_lines.as_ref()?;
        (picked == file).then(|| (*a.min(b), *a.max(b)))
    }

    /// The lines an action on `line` of `file` is for: the lines picked if
    /// it is one of them, else just it.
    fn lines_at(&self, file: &str, line: u32) -> (u32, u32) {
        self.picked_in(file)
            .filter(|(first, last)| (*first..=*last).contains(&line))
            .unwrap_or((line, line))
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
        edge: Edge,
    ) -> Option<HeaderAction> {
        // The arrow first, at the right; the items wrap in the room left,
        // so a narrow pane never gets a header wider than itself.
        let (action, should_hide) = egui::Sides::new().shrink_left().show(
            ui,
            |ui| {
                ui.horizontal_wrapped(|ui| self.header_items(ui, graph, symbol, is_starred))
                    .inner
            },
            |ui| pane_bar::hide_button(ui, edge),
        );
        if should_hide {
            Some(HeaderAction::Hide)
        } else {
            action
        }
    }

    /// The file, its buttons, and where this location is among the
    /// symbol's others.
    fn header_items(
        &self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        symbol: &Symbol,
        is_starred: bool,
    ) -> Option<HeaderAction> {
        let cursor = self.cursor.as_ref();
        let location = cursor.and_then(LocationCursor::current);
        let file = location.map_or(symbol.file.as_str(), |l| l.file.as_str());
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

/// The snippets of `file`'s findings, by line, to highlight.
fn risks_for<'a>(file: &str, findings: &'a Findings) -> BTreeMap<u32, Vec<RiskSpan<'a>>> {
    let mut risks: BTreeMap<u32, Vec<RiskSpan>> = BTreeMap::new();
    for (id, finding) in findings.snippets_in(file) {
        let Some((start, end)) = finding.columns else {
            continue;
        };
        risks.entry(finding.line).or_default().push(RiskSpan {
            start,
            end,
            color: severity_color(finding.severity),
            title: &finding.rule,
            why: &finding.message,
            id,
        });
    }
    risks
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
    risks: &'a BTreeMap<u32, Vec<RiskSpan<'a>>>,
) -> Focus<'a> {
    match location {
        Some(l) => Focus {
            file: &l.file,
            line: l.occurrence.span.line,
            span: Some(l.occurrence.span),
            selected: Some(symbol.id),
            should_scroll,
            line_marks,
            risks,
            trail: &[],
            trace_source: None,
            call_summaries: None,
            selected_lines: None,
            is_drafting_rule: false,
            colors: SourceColors::default(),
        },
        None => Focus {
            file: &symbol.file,
            line: symbol.line,
            span: None,
            selected: Some(symbol.id),
            should_scroll,
            line_marks,
            risks,
            trail: &[],
            trace_source: None,
            call_summaries: None,
            selected_lines: None,
            is_drafting_rule: false,
            colors: SourceColors::default(),
        },
    }
}

#[cfg(test)]
#[path = "code_panel_tests.rs"]
mod tests;

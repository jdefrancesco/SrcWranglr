//! Code view panel: the source of the file in focus, with the focused line
//! highlighted (Sourcetrail's "active" line). Every name the indexer
//! resolved is clickable, and occurrences of the selected symbol are tinted.
//!
//! Syntax colouring uses a small hand-written tokenizer (`tokenize`);
//! symbol-aware decoration comes from the index's occurrences.

mod line_menu;
pub mod scheme;
mod segments;
mod tokenize;
mod variables;

use crate::app::visible::{visible, visible_lines};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use std::sync::Arc;

use egui::text::{CCursor, Galley, LayoutJob, TextFormat};
use egui::{Color32, CursorIcon, Response, RichText, Sense, Stroke};

use crate::model::{CodeGraph, EdgeKind, Occurrence, OccurrenceRole, Span, SymbolId};
use crate::style::{BOOKMARK_COLOR, MARK_OUTLINE, NOTE_COLOR, TRAIL_COLOR};
use line_menu::{line_menu, MenuContext};
use scheme::SourceColors;
use segments::{merge, overlay_risks, Mark, Segment};
use tokenize::colored_ranges;

/// Names a line's menu lists for cross references, at most.
const MAX_MENU_NAMES: usize = 20;
/// Places a highlighted word is marked on one line, at most.
const MAX_WORD_MARKS: usize = 100;
const LINK_UNDERLINE_ALPHA: f32 = 0.35;
/// Risky snippets: a tint of their severity color, underlined.
const RISK_BACKGROUND_ALPHA: f32 = 0.22;
const RISK_UNDERLINE_WIDTH: f32 = 1.5;
const GUTTER_WIDTH: f32 = 52.0;
/// Gutter marker for a line with a note or bookmark.
const MARKER_RADIUS: f32 = 3.5;
const MARKER_INSET: f32 = 9.0;
/// Width of the bar at the gutter's left edge marking a warning.
const WARNING_BAR_WIDTH: f32 = 3.0;
/// Longer lines (minified/generated code) are clipped for display; shaping a
/// multi-megabyte line every frame would freeze the UI.
const MAX_DISPLAY_CHARS: usize = 2_000;
/// A trace path's steps are underlined in `TRAIL_COLOR` this thick, over
/// a tint of it this strong.
const TRAIL_UNDERLINE_WIDTH: f32 = 2.0;
const TRAIL_BACKGROUND_ALPHA: f32 = 0.22;
/// After jumping to the focused line, this fraction of the viewport sits above it.
const FOCUS_LINE_VIEW_FRACTION: f32 = 0.3;
/// A focused line this close to the top or bottom edge counts as off screen.
const SCROLL_MARGIN_ROWS: usize = 2;

/// What the code view should show.
pub struct Focus<'a> {
    pub file: &'a str,
    /// 1-based line to highlight and keep in view.
    pub line: u32,
    /// The exact name to highlight on that line, if known.
    pub span: Option<Span>,
    /// Symbol whose occurrences get a softer highlight.
    pub selected: Option<SymbolId>,
    /// Scroll `line` into view (set on the frame the focus changes).
    pub should_scroll: bool,
    /// Lines of `file` with a note or bookmark.
    pub line_marks: &'a BTreeMap<u32, LineMark<'a>>,
    /// Risky snippets (findings) of `file`, by line.
    pub risks: &'a BTreeMap<u32, Vec<RiskSpan<'a>>>,
    /// Where the steps of the trace path shown are in `file`.
    pub trail: &'a [Span],
    /// The variable picked as the source of a trace to a sink, if any.
    pub trace_source: Option<&'a str>,
    /// Man page summaries of library functions, by name: `None` where this
    /// system has no page. Names missing aren't looked up yet.
    pub call_summaries: Option<&'a HashMap<String, Option<String>>>,
    /// The lines picked by clicking their numbers (first, last), if any.
    pub selected_lines: Option<(u32, u32)>,
    /// The word clicked, highlighted wherever it is.
    pub highlighted_word: Option<&'a str>,
    /// Whether a rule is being drafted, so a line's menu can add to it.
    pub is_drafting_rule: bool,
    /// The color scheme to draw with.
    pub colors: SourceColors,
}

/// A finding's snippet on one line: byte columns `start..end`, drawn in
/// `color`; hovering explains it, clicking reports `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RiskSpan<'a> {
    pub start: u32,
    pub end: u32,
    pub color: Color32,
    pub title: &'a str,
    pub why: &'a str,
    pub id: usize,
}

/// What the gutter shows for a line: its note (possibly empty), whether
/// it is bookmarked, and a warning (a finding) on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LineMark<'a> {
    pub is_starred: bool,
    pub note: &'a str,
    pub warning: Option<LineWarning<'a>>,
}

/// A finding on a line: `title: text`, drawn as a bar in `color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineWarning<'a> {
    pub title: &'a str,
    pub text: &'a str,
    pub color: Color32,
}

/// What the user asked for on one line, from its right-click menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineAction {
    EditNote,
    ToggleBookmark,
    /// Add this called function to the dangerous-functions list.
    MarkDangerous(String),
    /// Trace where the variable at this byte column gets its value.
    Trace(u32),
    /// Trace where the value of the variable at this byte column goes.
    TraceForward(u32),
    /// Add a TODO about this line.
    AddTodo,
    /// Draft a semgrep rule finding code like this line (or the lines
    /// picked, if it is one of them).
    FindSimilar,
    /// Add this line (or the lines picked) to the rule being drafted.
    AddRulePlace,
    /// Leave code like this line out of the rule being drafted.
    UseAsRuleException,
    /// List the functions most like the one holding this line.
    FindLookAlikes,
    /// Make the variable `name` at this byte column the source of a trace
    /// to a sink.
    SetTraceSource {
        column: u32,
        name: String,
    },
    /// Trace from the source picked to the call to this function here.
    TraceToHere(String),
    /// Show the docs for this function called here.
    ShowDocs(String),
    /// List the cross references to this name used here.
    CrossReferences(String),
}

/// What a cross reference list is for: a project symbol, or a library
/// function by name (`memcpy`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Referenced {
    Symbol(SymbolId),
    Library(String),
}

/// Something the user did in the code view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeEvent {
    /// Clicked a resolved name.
    Clicked(Occurrence),
    /// Chose a line action for this 1-based line.
    Line(u32, LineAction),
    /// Clicked this line's number; with Shift held, `extend`s the lines
    /// picked to it.
    LineNumber { line: u32, extend: bool },
    /// Clicked a risky snippet: the `RiskSpan::id`.
    Risk(usize),
    /// Clicked this word (an identifier), or none (space, punctuation).
    Word(Option<String>),
    /// Pressed X over a name: list its cross references.
    CrossReferences(Referenced),
    /// Hovering a call to a library function whose man page isn't looked
    /// up yet: its name as looked up.
    HoveredCall(String),
}

/// Source viewer. Keeps the line index of the file it last showed, so
/// repainting doesn't re-split the whole file every frame.
#[derive(Default)]
pub struct CodeView {
    cached_file: Option<String>,
    lines: Vec<Range<usize>>,
    /// Rows drawn last frame (0-based), for the cached file.
    visible_rows: Range<usize>,
    /// The doc brief of the symbol last hovered, in the graph at that
    /// address: finding it scans the project, too slow for every frame.
    hovered_brief: RefCell<Option<(SymbolId, usize, Option<String>)>>,
}

/// Per-frame inputs shared by every row.
struct Rows<'a> {
    graph: &'a CodeGraph,
    focus: &'a Focus<'a>,
    contents: &'a str,
    font: egui::FontId,
    row_height: f32,
    hovered_brief: &'a RefCell<Option<(SymbolId, usize, Option<String>)>>,
}

impl CodeView {
    /// Renders `focus.file`. Only visible rows are laid out, so large files
    /// stay cheap. Returns what the user did, if anything.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        focus: &Focus,
    ) -> Option<CodeEvent> {
        let Some(contents) = graph.file_contents.get(focus.file) else {
            ui.label(format!("Source for {} is not available.", focus.file));
            return None;
        };
        if self.cached_file.as_deref() != Some(focus.file) {
            self.lines = line_ranges(contents);
            self.cached_file = Some(focus.file.to_string());
            self.visible_rows = 0..0;
        }
        let rows = Rows {
            graph,
            focus,
            contents,
            font: egui::TextStyle::Monospace.resolve(ui.style()),
            row_height: ui.text_style_height(&egui::TextStyle::Monospace),
            hovered_brief: &self.hovered_brief,
        };
        let mut area = egui::ScrollArea::both()
            .id_salt(focus.file)
            .auto_shrink([false, false]);
        let focus_row = usize::try_from(focus.line.saturating_sub(1)).unwrap_or(usize::MAX);
        if focus.should_scroll && !is_comfortably_visible(focus_row, &self.visible_rows) {
            let row_pitch = rows.row_height + ui.spacing().item_spacing.y;
            let target = focus.line.saturating_sub(1) as f32 * row_pitch
                - ui.available_height() * FOCUS_LINE_VIEW_FRACTION;
            area = area.vertical_scroll_offset(target.max(0.0));
        }

        let lines = &self.lines;
        let (clicked, visible_rows) = egui::Frame::NONE
            .fill(focus.colors.background)
            .show(ui, |ui| {
                area.show_rows(ui, rows.row_height, lines.len(), |ui, visible| {
                    // Rows must be exactly `row_height` tall, or the
                    // virtualization and scroll-to-line math drift.
                    ui.spacing_mut().interact_size.y = rows.row_height;
                    let clicked = visible
                        .clone()
                        .filter_map(|idx| rows.show_line(ui, idx, lines.get(idx)?.clone()))
                        .last();
                    (clicked, visible)
                })
                .inner
            })
            .inner;
        self.visible_rows = visible_rows;
        clicked
    }
}

/// Whether `row` is on screen and not hugging an edge of `visible`.
fn is_comfortably_visible(row: usize, visible: &Range<usize>) -> bool {
    let first = visible.start.saturating_add(SCROLL_MARGIN_ROWS);
    let past = visible.end.saturating_sub(SCROLL_MARGIN_ROWS);
    (first..past).contains(&row) || (visible.start == 0 && row < past)
}

impl Rows<'_> {
    fn show_line(&self, ui: &mut egui::Ui, idx: usize, range: Range<usize>) -> Option<CodeEvent> {
        let text = self.contents.get(range).unwrap_or("");
        let line_no = u32::try_from(idx + 1).unwrap_or(u32::MAX);
        let (shown, visible_len) = display_text(text);
        let occurrences = self.graph.occurrences_on_line(self.focus.file, line_no);
        let marks = self.marks_on(line_no, occurrences, &shown, visible_len);
        let risks = self
            .focus
            .risks
            .get(&line_no)
            .map_or(&[][..], Vec::as_slice);
        let risk_ranges: Vec<(Range<usize>, Color32)> = risks
            .iter()
            .map(|r| (risk_range(r, line_no, visible_len), r.color))
            .collect();
        let segments = overlay_risks(
            &shown,
            merge(&shown, &colored_ranges(&shown, &self.focus.colors), &marks),
            &risk_ranges,
        );
        let galley =
            ui.painter()
                .layout_job(layout(&shown, &segments, &self.font, &self.focus.colors));

        let is_picked = self
            .focus
            .selected_lines
            .is_some_and(|(first, last)| (first..=last).contains(&line_no));
        ui.horizontal(|ui| {
            if is_picked {
                let size = egui::vec2(ui.available_width().max(GUTTER_WIDTH), self.row_height);
                let row = egui::Rect::from_min_size(ui.cursor().min, size);
                ui.painter()
                    .rect_filled(row, 0.0, self.focus.colors.selected_name);
            }
            if line_no == self.focus.line {
                // Painted before the labels, so it sits underneath them.
                let size = egui::vec2(ui.available_width().max(GUTTER_WIDTH), self.row_height);
                let row = egui::Rect::from_min_size(ui.cursor().min, size);
                ui.painter()
                    .rect_filled(row, 0.0, self.focus.colors.active_line);
            }
            let mark = self.focus.line_marks.get(&line_no);
            let gutter = ui.add_sized(
                [GUTTER_WIDTH, self.row_height],
                egui::Label::new(
                    RichText::new(format!("{line_no:>5} "))
                        .font(self.font.clone())
                        .color(self.focus.colors.line_number),
                )
                .sense(Sense::click()),
            );
            let gutter = match mark {
                Some(mark) => paint_marker(ui, gutter, mark),
                None => gutter,
            };
            let is_focus_line = line_no == self.focus.line;
            let line = DrawnLine {
                rect: egui::Rect::NOTHING,
                galley: &galley,
                shown: &shown,
                occurrences,
                risks,
                line_no,
                visible_len,
                is_focus_line,
            };
            let (event, code) = self.show_code(ui, &line);
            let calls = self.calls_on_line(line_no);
            let names = self.names_on_line(line_no);
            let picked_lines = self
                .focus
                .selected_lines
                .filter(|(first, last)| is_picked && first != last);
            let menu = MenuContext {
                shown: &shown,
                mark,
                calls: &calls,
                names: &names,
                trace_source: self.focus.trace_source,
                picked_lines,
                is_drafting_rule: self.focus.is_drafting_rule,
            };
            let number_clicked = gutter.clicked().then(|| CodeEvent::LineNumber {
                line: line_no,
                extend: ui.input(|i| i.modifiers.shift),
            });
            let action = line_menu(&gutter, &menu).or_else(|| line_menu(&code, &menu));
            action
                .map(|a| CodeEvent::Line(line_no, a))
                .or(number_clicked)
                .or(event)
        })
        .inner
    }

    /// Paints one line of code and handles hovering/clicking its names and
    /// risky snippets. Returns what was clicked and the line's response.
    fn show_code(&self, ui: &mut egui::Ui, line: &DrawnLine) -> (Option<CodeEvent>, Response) {
        let size = egui::vec2(line.galley.size().x, self.row_height);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, line.shown));
        ui.painter()
            .galley(rect.min, Arc::clone(line.galley), self.focus.colors.text);
        let line = DrawnLine { rect, ..*line };
        (self.hit(ui, &response, &line), response)
    }

    /// Scrolls the focused name into view, and explains / reports the risky
    /// snippet or name under the pointer.
    fn hit(&self, ui: &egui::Ui, response: &Response, line: &DrawnLine) -> Option<CodeEvent> {
        let x_of = |byte: usize| {
            let chars = line.shown.get(..byte).map_or(0, |s| s.chars().count());
            line.rect.min.x + line.galley.pos_from_cursor(CCursor::new(chars)).min.x
        };
        let focus_span = self
            .focus
            .span
            .filter(|_| line.is_focus_line && self.focus.should_scroll);
        if let Some(span) = focus_span {
            // Bring the focused name into view sideways as well.
            let range = byte_range(span, line.visible_len);
            let x = x_of(range.start)..=x_of(range.end);
            ui.scroll_to_rect(egui::Rect::from_x_y_ranges(x, line.rect.y_range()), None);
        }

        let pointer_x = response.hover_pos()?.x;
        let name = occurrence_at_x(line.occurrences, line.visible_len, pointer_x, x_of);
        let risk = line.risks.iter().find(|r| {
            let range = risk_range(r, line.line_no, line.visible_len);
            !range.is_empty() && (x_of(range.start)..x_of(range.end)).contains(&pointer_x)
        });
        // A name inside a risky call (an argument, a nested call) explores
        // it; the risk itself is explained from the rest of the call.
        let risk = risk.filter(|r| name.is_none_or(|o| o.span.start == r.start));
        if let Some(risk) = risk {
            let response = response
                .clone()
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_ui_at_pointer(|ui| {
                    // Messages may quote the code, and notes come from it.
                    ui.strong(visible(risk.title));
                    ui.label(visible_lines(risk.why));
                    ui.weak("click for details");
                });
            return response.clicked().then_some(CodeEvent::Risk(risk.id));
        }
        let Some(hit) = name.copied() else {
            if response.clicked() {
                let word = byte_at_x(line, pointer_x, x_of)
                    .and_then(|at| word_at(line.shown, at))
                    .map(str::to_string);
                return Some(CodeEvent::Word(word));
            }
            return self.library_call_hover(response, line, pointer_x, x_of);
        };
        let symbol = self.graph.symbols.get(&hit.symbol)?;
        if pressed_x(ui.ctx()) {
            return Some(CodeEvent::CrossReferences(Referenced::Symbol(symbol.id)));
        }
        let brief = self.brief_of(symbol.id);
        let response = response
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_ui_at_pointer(|ui| {
                ui.strong(visible(&symbol.qualified_name));
                ui.label(visible(&format!(
                    "{} · {}:{}",
                    symbol.kind.label(),
                    symbol.file,
                    symbol.line
                )));
                if let Some(brief) = &brief {
                    ui.label(visible_lines(brief));
                }
                ui.weak("click to explore · x for cross references · right-click › Show Docs");
            });
        response.clicked().then_some(CodeEvent::Clicked(hit))
    }

    /// The first sentence of `id`'s doc comment, remembered for the symbol
    /// last asked about.
    fn brief_of(&self, id: SymbolId) -> Option<String> {
        let graph_at = std::ptr::from_ref(self.graph) as usize;
        let mut cached = self.hovered_brief.borrow_mut();
        if let Some((symbol, at, brief)) = cached.as_ref() {
            if *symbol == id && *at == graph_at {
                return brief.clone();
            }
        }
        let brief = crate::docs::symbol_brief(self.graph, id);
        *cached = Some((id, graph_at, brief.clone()));
        brief
    }

    /// Explains the library call under the pointer with its man page's
    /// summary; asks for the page if it isn't looked up yet.
    fn library_call_hover(
        &self,
        response: &Response,
        line: &DrawnLine,
        pointer_x: f32,
        x_of: impl Fn(usize) -> f32,
    ) -> Option<CodeEvent> {
        let calls = self
            .graph
            .external_calls_on_line(self.focus.file, line.line_no);
        let call = calls.iter().find(|c| {
            let range = byte_range(c.span, line.visible_len);
            !range.is_empty() && (x_of(range.start)..x_of(range.end)).contains(&pointer_x)
        })?;
        if pressed_x(&response.ctx) {
            let bare = crate::analysis::risky::bare_library_name(&call.name).unwrap_or(&call.name);
            return Some(CodeEvent::CrossReferences(Referenced::Library(
                bare.to_string(),
            )));
        }
        let name = crate::docs::man::lookup_name(&call.name)?;
        let summary = self.focus.call_summaries.and_then(|s| s.get(name));
        response.clone().on_hover_ui_at_pointer(|ui| {
            ui.strong(format!("{name}()"));
            match summary {
                Some(Some(summary)) => {
                    ui.label(summary);
                }
                Some(None) => {
                    ui.weak("This system has no man page for it.");
                }
                None => {
                    ui.weak("Looking up its man page…");
                }
            }
            ui.weak("x for cross references · right-click › Show Docs for the whole page");
        });
        summary
            .is_none()
            .then(|| CodeEvent::HoveredCall(name.to_string()))
    }

    /// Names of the functions called on `line`: the project's and others.
    fn calls_on_line(&self, line: u32) -> Vec<&str> {
        let graph = self.graph;
        let project = graph
            .occurrences_on_line(self.focus.file, line)
            .iter()
            .filter(|o| o.role == OccurrenceRole::Reference(EdgeKind::Calls))
            .filter_map(|o| graph.symbols.get(&o.symbol))
            .map(|s| s.qualified_name.as_str());
        let external = graph
            .external_calls_on_line(self.focus.file, line)
            .iter()
            .map(|c| c.name.as_str());
        let mut names: Vec<&str> = project.chain(external).collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// The names on `line` with cross references: the symbols named there
    /// and the functions called.
    fn names_on_line(&self, line: u32) -> Vec<&str> {
        let graph = self.graph;
        let project = graph
            .occurrences_on_line(self.focus.file, line)
            .iter()
            .filter_map(|o| graph.symbols.get(&o.symbol))
            .map(|s| s.qualified_name.as_str());
        let external = graph
            .external_calls_on_line(self.focus.file, line)
            .iter()
            .map(|c| c.name.as_str());
        let mut names: Vec<&str> = project.chain(external).collect();
        names.sort_unstable();
        names.dedup();
        names.truncate(MAX_MENU_NAMES);
        names
    }

    /// How the names on line `line_no` are marked: its occurrences, the
    /// trace path's steps, and the focused name.
    fn marks_on(
        &self,
        line_no: u32,
        occurrences: &[Occurrence],
        shown: &str,
        visible_len: usize,
    ) -> Vec<(Range<usize>, Mark)> {
        let on_line = |s: &&Span| s.line == line_no;
        // The focused span may be a name the index doesn't know (a local
        // variable a trace stepped to), so it is marked on its own too.
        let focused = self
            .focus
            .span
            .iter()
            .filter(on_line)
            .map(|s| (byte_range(*s, visible_len), Mark::Active));
        let traced = self
            .focus
            .trail
            .iter()
            .filter(on_line)
            .map(|s| (byte_range(*s, visible_len), Mark::Trail));
        let words = self
            .focus
            .highlighted_word
            .map(|word| word_ranges(shown, visible_len, word))
            .unwrap_or_default()
            .into_iter()
            .map(|range| (range, Mark::Word));
        occurrences
            .iter()
            .map(|o| (byte_range(o.span, visible_len), self.mark_for(o)))
            .chain(words)
            .chain(traced)
            .chain(focused)
            .collect()
    }

    fn mark_for(&self, occurrence: &Occurrence) -> Mark {
        if Some(occurrence.span) == self.focus.span {
            Mark::Active
        } else if Some(occurrence.symbol) == self.focus.selected {
            Mark::Selected
        } else {
            Mark::Link
        }
    }
}

/// One painted line of code, for hit-testing its names and risks.
#[derive(Clone, Copy)]
struct DrawnLine<'a> {
    rect: egui::Rect,
    galley: &'a Arc<Galley>,
    shown: &'a str,
    occurrences: &'a [Occurrence],
    risks: &'a [RiskSpan<'a>],
    line_no: u32,
    visible_len: usize,
    is_focus_line: bool,
}

/// Byte range of `risk` within the shown part of line `line_no`.
fn risk_range(risk: &RiskSpan, line_no: u32, visible_len: usize) -> Range<usize> {
    let span = Span {
        line: line_no,
        start: risk.start,
        end: risk.end,
    };
    byte_range(span, visible_len)
}

/// Draws a line's warning bar and note/bookmark dot in the gutter;
/// hovering the gutter explains them.
fn paint_marker(ui: &egui::Ui, gutter: Response, mark: &LineMark) -> Response {
    let mut explanations: Vec<String> = Vec::new();
    if let Some(warning) = mark.warning {
        let bar = egui::Rect::from_min_size(
            gutter.rect.left_top(),
            egui::vec2(WARNING_BAR_WIDTH, gutter.rect.height()),
        );
        ui.painter().rect_filled(bar, 0.0, warning.color);
        explanations.push(format!(
            "{}: {}",
            visible(warning.title),
            visible_lines(warning.text)
        ));
    }
    if mark.is_starred || !mark.note.is_empty() {
        let color = if mark.is_starred {
            BOOKMARK_COLOR
        } else {
            NOTE_COLOR
        };
        let center = egui::pos2(gutter.rect.left() + MARKER_INSET, gutter.rect.center().y);
        let outline = Stroke::new(1.0, MARK_OUTLINE);
        ui.painter().circle(center, MARKER_RADIUS, color, outline);
        explanations.push(match mark.note {
            "" => "Bookmarked line".to_string(),
            // A note's lines stay lines; what hides inside them is marked.
            note => visible_lines(note),
        });
    }
    gutter.on_hover_text(explanations.join("\n\n"))
}

/// Whether X was just pressed (and not for typing): taken, so nothing else
/// acts on it too.
fn pressed_x(ctx: &egui::Context) -> bool {
    !ctx.egui_wants_keyboard_input()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::X))
}

/// The byte of `line`'s text under `pointer_x`, if any.
fn byte_at_x(line: &DrawnLine, pointer_x: f32, x_of: impl Fn(usize) -> f32) -> Option<usize> {
    let text = line.shown.get(..line.visible_len)?;
    text.char_indices()
        .find(|(at, c)| (x_of(*at)..x_of(at + c.len_utf8())).contains(&pointer_x))
        .map(|(at, _)| at)
}

/// Whether `c` can be part of a name.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The name around byte `at` of `line`, if one is there (a number isn't).
fn word_at(line: &str, at: usize) -> Option<&str> {
    let after = line.get(at..)?;
    if !after.chars().next().is_some_and(is_word_char) {
        return None;
    }
    let start = line[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word_char(*c))
        .last()
        .map_or(at, |(i, _)| i);
    let end = after
        .char_indices()
        .find(|(_, c)| !is_word_char(*c))
        .map_or(line.len(), |(i, _)| at + i);
    let word = &line[start..end];
    (!word.starts_with(|c: char| c.is_ascii_digit())).then_some(word)
}

/// Where `word` is in the first `visible_len` bytes of `line`, as a whole
/// name (not part of a longer one).
fn word_ranges(line: &str, visible_len: usize, word: &str) -> Vec<Range<usize>> {
    let text = line.get(..visible_len).unwrap_or(line);
    if word.is_empty() {
        return Vec::new();
    }
    text.match_indices(word)
        .filter(|(at, _)| {
            let before = text[..*at].chars().next_back();
            let after = text[at + word.len()..].chars().next();
            !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
        })
        .map(|(at, _)| at..at + word.len())
        .take(MAX_WORD_MARKS)
        .collect()
}

fn byte_range(span: Span, visible_len: usize) -> Range<usize> {
    let clamp = |column: u32| usize::try_from(column).map_or(visible_len, |c| c.min(visible_len));
    clamp(span.start)..clamp(span.end)
}

/// The occurrence drawn under horizontal position `x`, given each byte
/// offset's x position. Names past the clipped part of the line don't count.
fn occurrence_at_x(
    occurrences: &[Occurrence],
    visible_len: usize,
    x: f32,
    x_of: impl Fn(usize) -> f32,
) -> Option<&Occurrence> {
    occurrences.iter().find(|o| {
        let range = byte_range(o.span, visible_len);
        !range.is_empty() && (x_of(range.start)..x_of(range.end)).contains(&x)
    })
}

fn layout(
    line: &str,
    segments: &[Segment],
    font: &egui::FontId,
    colors: &SourceColors,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    for segment in segments {
        let (background, underline) = match segment.mark {
            None => (Color32::TRANSPARENT, Stroke::NONE),
            Some(Mark::Link) => (
                Color32::TRANSPARENT,
                Stroke::new(1.0, segment.color.gamma_multiply(LINK_UNDERLINE_ALPHA)),
            ),
            Some(Mark::Selected) => (colors.selected_name, Stroke::NONE),
            Some(Mark::Word) => (colors.highlighted_word, Stroke::NONE),
            Some(Mark::Trail) => (
                TRAIL_COLOR.gamma_multiply(TRAIL_BACKGROUND_ALPHA),
                Stroke::new(TRAIL_UNDERLINE_WIDTH, TRAIL_COLOR),
            ),
            Some(Mark::Active) => (colors.active_name, Stroke::NONE),
        };
        // A risky snippet is underlined in its color, and tinted unless the
        // selection already colors it. A trace path's step keeps its own
        // marks there: it is what is being followed.
        let risk = segment.risk.filter(|_| segment.mark != Some(Mark::Trail));
        let (background, underline) = match risk {
            Some(risk) => (
                match background {
                    Color32::TRANSPARENT => risk.gamma_multiply(RISK_BACKGROUND_ALPHA),
                    selected => selected,
                },
                Stroke::new(RISK_UNDERLINE_WIDTH, risk),
            ),
            None => (background, underline),
        };
        let format = TextFormat {
            font_id: font.clone(),
            color: segment.color,
            background,
            underline,
            ..TextFormat::default()
        };
        job.append(line.get(segment.range.clone()).unwrap_or(""), 0.0, format);
    }
    job
}

/// `line`, clipped to `MAX_DISPLAY_CHARS` chars plus an ellipsis, and how
/// many of its bytes are the original text.
fn display_text(line: &str) -> (Cow<'_, str>, usize) {
    match line.char_indices().nth(MAX_DISPLAY_CHARS) {
        Some((cut, _)) => (Cow::Owned(format!("{}…", &line[..cut])), cut),
        None => (Cow::Borrowed(line), line.len()),
    }
}

/// Byte ranges of each line's text (without `\n` / `\r\n`).
fn line_ranges(contents: &str) -> Vec<Range<usize>> {
    let mut start = 0;
    contents
        .split_inclusive('\n')
        .map(|chunk| {
            let body = chunk.trim_end_matches(['\n', '\r']);
            let range = start..start + body.len();
            start += chunk.len();
            range
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EdgeKind, OccurrenceRole};

    fn occurrence(start: u32, end: u32, symbol: SymbolId) -> Occurrence {
        Occurrence {
            span: Span {
                line: 1,
                start,
                end,
            },
            symbol,
            role: OccurrenceRole::Reference(EdgeKind::Calls),
            context: None,
        }
    }

    /// Monospace: every byte is 10 px wide.
    fn mono(byte: usize) -> f32 {
        byte as f32 * 10.0
    }

    #[test]
    fn a_click_finds_the_whole_name_under_it() {
        let line = "  len = strlen(src_len) + 10;";
        assert_eq!(word_at(line, 2), Some("len"));
        assert_eq!(word_at(line, 4), Some("len"));
        assert_eq!(word_at(line, 17), Some("src_len"));
        assert_eq!(word_at(line, 5), None);
        assert_eq!(word_at(line, 27), None);
        assert_eq!(word_at("été x", 1), None);
        assert_eq!(word_at("été x", 0), Some("été"));
        assert_eq!(word_at(line, 99), None);
    }

    #[test]
    fn a_word_is_marked_only_where_it_is_a_whole_name() {
        let line = "len = strlen(src_len) + len;";
        assert_eq!(word_ranges(line, line.len(), "len"), [0..3, 24..27]);
        let cut = word_ranges(line, 10, "len");
        assert_eq!((cut.len(), cut.first()), (1, Some(&(0..3))));
        assert!(word_ranges(line, line.len(), "").is_empty());
    }

    #[test]
    fn line_ranges_match_str_lines() {
        let text = "a\r\nbb\n\nccc";
        let ranges = line_ranges(text);
        let lines: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(lines, text.lines().collect::<Vec<_>>());
        assert!(line_ranges("").is_empty());
    }

    #[test]
    fn clips_very_long_lines_on_a_char_boundary() {
        let long = "é".repeat(MAX_DISPLAY_CHARS + 50);
        let (shown, visible_len) = display_text(&long);
        assert_eq!(shown.chars().count(), MAX_DISPLAY_CHARS + 1);
        assert!(shown.ends_with('…'));
        assert_eq!(visible_len, MAX_DISPLAY_CHARS * 'é'.len_utf8());
        assert_eq!(display_text("short"), (Cow::Borrowed("short"), 5));
    }

    #[test]
    fn finds_the_occurrence_under_the_pointer() {
        let occurrences = [occurrence(0, 3, 1), occurrence(4, 7, 2)];
        let at = |x| occurrence_at_x(&occurrences, 100, x, mono).map(|o| o.symbol);
        assert_eq!(at(5.0), Some(1));
        assert_eq!(at(45.0), Some(2));
        assert_eq!(at(35.0), None); // the gap between them
        assert_eq!(at(70.0), None); // end is exclusive
    }

    #[test]
    fn ignores_occurrences_in_the_clipped_part_of_a_line() {
        let occurrences = [occurrence(50, 60, 1)];
        assert!(occurrence_at_x(&occurrences, 40, 550.0, mono).is_none());
    }

    #[test]
    fn layout_keeps_the_text_and_decorates_links() {
        let font = egui::FontId::monospace(12.0);
        let colors = SourceColors::default();
        let segments = merge(
            "f(x)",
            &colored_ranges("f(x)", &colors),
            &[(0..1, Mark::Link)],
        );
        let job = layout("f(x)", &segments, &font, &colors);
        assert_eq!(job.text, "f(x)");
        assert_ne!(job.sections[0].format.underline, Stroke::NONE);
        assert_eq!(job.sections[1].format.underline, Stroke::NONE);
    }
}

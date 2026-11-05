//! Code view panel: the source of the file in focus, with the focused line
//! highlighted (Sourcetrail's "active" line). Every name the indexer
//! resolved is clickable, and occurrences of the selected symbol are tinted.
//!
//! Syntax colouring uses a small hand-written tokenizer (`tokenize`);
//! symbol-aware decoration comes from the index's occurrences.

mod segments;
mod tokenize;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Range;

use egui::text::{CCursor, Galley, LayoutJob, TextFormat};
use egui::{Color32, CursorIcon, Response, RichText, Sense, Stroke};

use crate::model::{CodeGraph, Occurrence, Span, SymbolId};
use crate::style::{BOOKMARK_COLOR, MARK_OUTLINE, NOTE_COLOR};
use segments::{merge, Mark, Segment};
use tokenize::colored_ranges;

const CODE_BACKGROUND: Color32 = Color32::from_gray(18);
const ACTIVE_LINE_BACKGROUND: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x14);
const ACTIVE_NAME_BACKGROUND: Color32 = Color32::from_rgb(0x8A, 0x6D, 0x1C);
const SELECTED_NAME_BACKGROUND: Color32 = Color32::from_rgb(0x2E, 0x45, 0x63);
const LINK_UNDERLINE_ALPHA: f32 = 0.35;
const LINE_NUMBER_COLOR: Color32 = Color32::from_gray(90);
const GUTTER_WIDTH: f32 = 52.0;
/// Gutter marker for a line with a note or bookmark.
const MARKER_RADIUS: f32 = 3.5;
const MARKER_INSET: f32 = 9.0;
/// Width of the bar at the gutter's left edge marking a warning.
const WARNING_BAR_WIDTH: f32 = 3.0;
/// Longer lines (minified/generated code) are clipped for display; shaping a
/// multi-megabyte line every frame would freeze the UI.
const MAX_DISPLAY_CHARS: usize = 2_000;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineAction {
    EditNote,
    ToggleBookmark,
}

/// Something the user did in the code view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeEvent {
    /// Clicked a resolved name.
    Clicked(Occurrence),
    /// Chose a line action for this 1-based line.
    Line(u32, LineAction),
}

/// Source viewer. Keeps the line index of the file it last showed, so
/// repainting doesn't re-split the whole file every frame.
#[derive(Default)]
pub struct CodeView {
    cached_file: Option<String>,
    lines: Vec<Range<usize>>,
    /// Rows drawn last frame (0-based), for the cached file.
    visible_rows: Range<usize>,
}

/// Per-frame inputs shared by every row.
struct Rows<'a> {
    graph: &'a CodeGraph,
    focus: &'a Focus<'a>,
    contents: &'a str,
    font: egui::FontId,
    row_height: f32,
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
            .fill(CODE_BACKGROUND)
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
        let marks: Vec<(Range<usize>, Mark)> = occurrences
            .iter()
            .map(|o| (byte_range(o.span, visible_len), self.mark_for(o)))
            .collect();
        let segments = merge(&shown, &colored_ranges(&shown), &marks);
        let galley = ui
            .painter()
            .layout_job(layout(&shown, &segments, &self.font));

        ui.horizontal(|ui| {
            if line_no == self.focus.line {
                // Painted before the labels, so it sits underneath them.
                let size = egui::vec2(ui.available_width().max(GUTTER_WIDTH), self.row_height);
                let row = egui::Rect::from_min_size(ui.cursor().min, size);
                ui.painter().rect_filled(row, 0.0, ACTIVE_LINE_BACKGROUND);
            }
            let mark = self.focus.line_marks.get(&line_no);
            let gutter = ui.add_sized(
                [GUTTER_WIDTH, self.row_height],
                egui::Label::new(
                    RichText::new(format!("{line_no:>5} "))
                        .font(self.font.clone())
                        .color(LINE_NUMBER_COLOR),
                )
                .sense(Sense::click()),
            );
            let gutter = match mark {
                Some(mark) => paint_marker(ui, gutter, mark),
                None => gutter,
            };
            let is_focus_line = line_no == self.focus.line;
            let (clicked, code) =
                self.show_code(ui, galley, &shown, occurrences, visible_len, is_focus_line);
            let action = line_menu(&gutter, &shown, mark).or(line_menu(&code, &shown, mark));
            action
                .map(|a| CodeEvent::Line(line_no, a))
                .or(clicked.map(CodeEvent::Clicked))
        })
        .inner
    }

    /// Paints one line of code and handles hovering/clicking its names.
    /// Returns the clicked occurrence and the line's response.
    fn show_code(
        &self,
        ui: &mut egui::Ui,
        galley: std::sync::Arc<Galley>,
        shown: &str,
        occurrences: &[Occurrence],
        visible_len: usize,
        is_focus_line: bool,
    ) -> (Option<Occurrence>, Response) {
        let size = egui::vec2(galley.size().x, self.row_height);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, shown));
        ui.painter()
            .galley(rect.min, galley.clone(), Color32::from_gray(210));
        let line = DrawnLine {
            rect,
            galley: &galley,
            shown,
            occurrences,
            visible_len,
            is_focus_line,
        };
        (self.clicked_name(ui, &response, &line), response)
    }

    /// Scrolls the focused name into view, and shows/returns the name under
    /// the pointer when hovered/clicked.
    fn clicked_name(
        &self,
        ui: &egui::Ui,
        response: &Response,
        line: &DrawnLine,
    ) -> Option<Occurrence> {
        let DrawnLine {
            rect,
            galley,
            shown,
            occurrences,
            visible_len,
            is_focus_line,
        } = *line;

        let x_of = |byte: usize| {
            let chars = shown.get(..byte).map_or(0, |s| s.chars().count());
            rect.min.x + galley.pos_from_cursor(CCursor::new(chars)).min.x
        };
        let focus_span = self
            .focus
            .span
            .filter(|_| is_focus_line && self.focus.should_scroll);
        if let Some(span) = focus_span {
            // Bring the focused name into view sideways as well.
            let range = byte_range(span, visible_len);
            let x = x_of(range.start)..=x_of(range.end);
            ui.scroll_to_rect(egui::Rect::from_x_y_ranges(x, rect.y_range()), None);
        }

        let pointer_x = response.hover_pos()?.x;
        let hit = *occurrence_at_x(occurrences, visible_len, pointer_x, x_of)?;
        let symbol = self.graph.symbols.get(&hit.symbol)?;
        let response = response
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_ui_at_pointer(|ui| {
                ui.strong(&symbol.qualified_name);
                ui.label(format!(
                    "{} · {}:{}",
                    symbol.kind.label(),
                    symbol.file,
                    symbol.line
                ));
                ui.weak("click to explore");
            });
        response.clicked().then_some(hit)
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

/// One painted line of code, for hit-testing its names.
#[derive(Clone, Copy)]
struct DrawnLine<'a> {
    rect: egui::Rect,
    galley: &'a Galley,
    shown: &'a str,
    occurrences: &'a [Occurrence],
    visible_len: usize,
    is_focus_line: bool,
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
        explanations.push(format!("{}: {}", warning.title, warning.text));
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
            note => note.to_string(),
        });
    }
    gutter.on_hover_text(explanations.join("\n\n"))
}

/// The right-click menu of a line (on its number or its code).
fn line_menu(response: &Response, shown: &str, mark: Option<&LineMark>) -> Option<LineAction> {
    let has_note = mark.is_some_and(|m| !m.note.is_empty());
    let is_starred = mark.is_some_and(|m| m.is_starred);
    let mut action = None;
    response.context_menu(|ui| {
        if ui.button("Copy Line").clicked() {
            ui.ctx().copy_text(shown.to_string());
            ui.close();
        }
        ui.separator();
        let note = if has_note {
            "Edit Note…"
        } else {
            "Add Note…"
        };
        let star = if is_starred {
            "Remove Bookmark"
        } else {
            "Bookmark Line"
        };
        if ui.button(note).clicked() {
            action = Some(LineAction::EditNote);
        }
        if ui.button(star).clicked() {
            action = Some(LineAction::ToggleBookmark);
        }
        if action.is_some() {
            ui.close();
        }
    });
    action
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

fn layout(line: &str, segments: &[Segment], font: &egui::FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    for segment in segments {
        let (background, underline) = match segment.mark {
            None => (Color32::TRANSPARENT, Stroke::NONE),
            Some(Mark::Link) => (
                Color32::TRANSPARENT,
                Stroke::new(1.0, segment.color.gamma_multiply(LINK_UNDERLINE_ALPHA)),
            ),
            Some(Mark::Selected) => (SELECTED_NAME_BACKGROUND, Stroke::NONE),
            Some(Mark::Active) => (ACTIVE_NAME_BACKGROUND, Stroke::NONE),
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
        let segments = merge("f(x)", &colored_ranges("f(x)"), &[(0..1, Mark::Link)]);
        let job = layout("f(x)", &segments, &font);
        assert_eq!(job.text, "f(x)");
        assert_ne!(job.sections[0].format.underline, Stroke::NONE);
        assert_eq!(job.sections[1].format.underline, Stroke::NONE);
    }
}

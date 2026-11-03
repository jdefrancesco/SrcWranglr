//! Code view panel: shows the source file containing the selected symbol,
//! with the relevant line highlighted (Sourcetrail calls this "active" line
//! highlighting).
//!
//! Uses a small hand-written tokenizer rather than pulling in `syntect`, to
//! keep the dependency surface minimal for now. Swap in `syntect` later for
//! real language grammars; this is deliberately just keywords/strings/
//! comments/numbers.

use std::borrow::Cow;
use std::ops::Range;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, RichText};

const CPP_KEYWORDS: &[&str] = &[
    "class",
    "struct",
    "public",
    "private",
    "protected",
    "virtual",
    "override",
    "const",
    "static",
    "return",
    "explicit",
    "double",
    "int",
    "void",
    "auto",
    "namespace",
    "include",
    "default",
    "this",
];

enum Token<'a> {
    Keyword(&'a str),
    StringLit(&'a str),
    Comment(&'a str),
    Number(&'a str),
    Preproc(&'a str),
    Plain(&'a str),
}

fn tokenize_line(line: &str) -> Vec<Token<'_>> {
    if line.trim_start().starts_with('#') {
        return vec![Token::Preproc(line)];
    }
    if let Some(idx) = line.find("//") {
        let mut tokens = tokenize_line(&line[..idx]);
        tokens.push(Token::Comment(&line[idx..]));
        return tokens;
    }

    // Scans by `char`, so every slice boundary is a valid UTF-8 boundary.
    let mut tokens = Vec::new();
    let mut i = 0;
    while let Some(c) = line[i..].chars().next() {
        let end = if c.is_whitespace() {
            scan_while(line, i, char::is_whitespace)
        } else if c == '"' {
            string_end(line, i)
        } else if c.is_ascii_digit() {
            scan_while(line, i, |c| c.is_ascii_digit() || c == '.')
        } else if c.is_alphabetic() || c == '_' {
            scan_while(line, i, |c| c.is_alphanumeric() || c == '_')
        } else {
            i + c.len_utf8()
        };
        tokens.push(classify(c, &line[i..end]));
        i = end;
    }
    tokens
}

/// Byte index of the first char at/after `start` that fails `pred`.
fn scan_while(line: &str, start: usize, pred: impl Fn(char) -> bool) -> usize {
    line[start..]
        .char_indices()
        .find(|(_, c)| !pred(*c))
        .map_or(line.len(), |(offset, _)| start + offset)
}

/// End of a string literal opening at `start` (just past the closing quote,
/// or end of line if unterminated).
fn string_end(line: &str, start: usize) -> usize {
    let body = start + '"'.len_utf8();
    line[body..]
        .find('"')
        .map_or(line.len(), |offset| body + offset + 1)
}

fn classify(first: char, word: &str) -> Token<'_> {
    if first == '"' {
        Token::StringLit(word)
    } else if first.is_ascii_digit() {
        Token::Number(word)
    } else if CPP_KEYWORDS.contains(&word) {
        Token::Keyword(word)
    } else {
        Token::Plain(word)
    }
}

fn token_color(token: &Token) -> Color32 {
    match token {
        Token::Keyword(_) => Color32::from_rgb(0xCC, 0x7A, 0xD9),
        Token::StringLit(_) => Color32::from_rgb(0x9E, 0xC9, 0x6B),
        Token::Comment(_) => Color32::from_rgb(0x6A, 0x6A, 0x6A),
        Token::Number(_) => Color32::from_rgb(0xD9, 0xA6, 0x4A),
        Token::Preproc(_) => Color32::from_rgb(0x6A, 0x9E, 0xD9),
        Token::Plain(_) => Color32::from_gray(210),
    }
}

fn token_text<'a>(token: &Token<'a>) -> &'a str {
    match token {
        Token::Keyword(s)
        | Token::StringLit(s)
        | Token::Comment(s)
        | Token::Number(s)
        | Token::Preproc(s)
        | Token::Plain(s) => s,
    }
}

const CODE_BACKGROUND: Color32 = Color32::from_gray(18);
const ACTIVE_LINE_BACKGROUND: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x14);
const LINE_NUMBER_COLOR: Color32 = Color32::from_gray(90);
const GUTTER_WIDTH: f32 = 52.0;
/// Longer lines (minified/generated code) are clipped for display; shaping a
/// multi-megabyte line every frame would freeze the UI.
const MAX_DISPLAY_CHARS: usize = 2_000;
/// After jumping to the active line, this fraction of the viewport sits above it.
const ACTIVE_LINE_VIEW_FRACTION: f32 = 0.3;

/// `line`, clipped to `MAX_DISPLAY_CHARS` chars plus an ellipsis.
fn display_text(line: &str) -> Cow<'_, str> {
    match line.char_indices().nth(MAX_DISPLAY_CHARS) {
        Some((cut, _)) => Cow::Owned(format!("{}…", &line[..cut])),
        None => Cow::Borrowed(line),
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

/// Source viewer. Keeps the line index of the file it last showed, so
/// repainting doesn't re-split the whole file every frame.
#[derive(Default)]
pub struct CodeView {
    cached_file: Option<String>,
    lines: Vec<Range<usize>>,
}

impl CodeView {
    /// Renders `contents`, highlighting `active_line` (1-indexed) the way
    /// Sourcetrail highlights the line containing the selected symbol.
    /// Only visible rows are laid out, so large files stay cheap. Set
    /// `scroll_to_active` on the frame the selection changes.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        file_name: &str,
        contents: &str,
        active_line: Option<u32>,
        scroll_to_active: bool,
    ) {
        ui.label(RichText::new(file_name).strong().monospace());
        ui.separator();

        if self.cached_file.as_deref() != Some(file_name) {
            self.lines = line_ranges(contents);
            self.cached_file = Some(file_name.to_string());
        }
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
        let mut area = egui::ScrollArea::both()
            .id_salt(file_name)
            .auto_shrink([false, false]);
        if let Some(line) = active_line.filter(|_| scroll_to_active) {
            let row_pitch = row_height + ui.spacing().item_spacing.y;
            let target = line.saturating_sub(1) as f32 * row_pitch
                - ui.available_height() * ACTIVE_LINE_VIEW_FRACTION;
            area = area.vertical_scroll_offset(target.max(0.0));
        }

        let lines = &self.lines;
        egui::Frame::NONE.fill(CODE_BACKGROUND).show(ui, |ui| {
            area.show_rows(ui, row_height, lines.len(), |ui, rows| {
                for idx in rows {
                    let text = lines
                        .get(idx)
                        .and_then(|r| contents.get(r.clone()))
                        .unwrap_or("");
                    let line_no = u32::try_from(idx + 1).unwrap_or(u32::MAX);
                    let is_active = active_line == Some(line_no);
                    show_line(
                        ui,
                        line_no,
                        &display_text(text),
                        is_active,
                        row_height,
                        &font,
                    );
                }
            });
        });
    }
}

fn show_line(
    ui: &mut egui::Ui,
    line_no: u32,
    line: &str,
    is_active: bool,
    row_height: f32,
    font: &egui::FontId,
) {
    ui.horizontal(|ui| {
        if is_active {
            // Painted before the labels, so it sits underneath them.
            let size = egui::vec2(ui.available_width().max(GUTTER_WIDTH), row_height);
            let row = egui::Rect::from_min_size(ui.cursor().min, size);
            ui.painter().rect_filled(row, 0.0, ACTIVE_LINE_BACKGROUND);
        }
        ui.add_sized(
            [GUTTER_WIDTH, row_height],
            egui::Label::new(
                RichText::new(format!("{line_no:>5} "))
                    .font(font.clone())
                    .color(LINE_NUMBER_COLOR),
            ),
        );
        ui.add(egui::Label::new(highlighted(line, font)).extend());
    });
}

/// One line as a single multi-colored text job (one widget per line rather
/// than one per token).
fn highlighted(line: &str, font: &egui::FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    for token in tokenize_line(line) {
        let format = TextFormat {
            font_id: font.clone(),
            color: token_color(&token),
            ..TextFormat::default()
        };
        job.append(token_text(&token), 0.0, format);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(line: &str) -> String {
        tokenize_line(line).iter().map(token_text).collect()
    }

    #[test]
    fn tokens_reassemble_the_original_line() {
        for line in [
            "int x = 42; // done",
            "  return \"hi\" + y_1;",
            "#include <a.h>",
            "",
        ] {
            assert_eq!(roundtrip(line), line);
        }
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
        let shown = display_text(&long);
        assert_eq!(shown.chars().count(), MAX_DISPLAY_CHARS + 1);
        assert!(shown.ends_with('…'));
        assert_eq!(display_text("short"), "short");
    }

    #[test]
    fn handles_non_ascii_without_panicking() {
        for line in ["int naïve = é + 1;", "x = \"ünï\"; ü", "π", "→ // ok"] {
            assert_eq!(roundtrip(line), line);
        }
    }

    #[test]
    fn classifies_keywords_strings_numbers_and_comments() {
        let tokens = tokenize_line("return \"s\" 1.5 // c");
        assert!(matches!(tokens[0], Token::Keyword("return")));
        assert!(tokens
            .iter()
            .any(|t| matches!(t, Token::StringLit("\"s\""))));
        assert!(tokens.iter().any(|t| matches!(t, Token::Number("1.5"))));
        assert!(matches!(tokens.last(), Some(Token::Comment("// c"))));
    }
}

//! Showing text from the project (strings, file and function names) so it
//! can't read differently from what it holds: characters that don't show as
//! themselves are written as marks like `‹U+202E›`, colored apart from the
//! text so they can't be mistaken for it.

use std::borrow::Cow;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

/// How a hidden character is shown, so it isn't mistaken for text.
pub const MARK_COLOR: Color32 = Color32::from_rgb(0xE8, 0x8B, 0x3C);

/// A run of `text`: as written, or a mark standing for one hidden
/// character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece<'t> {
    Text(&'t str),
    Mark(String),
}

/// `text` split into what shows as itself and marks for what doesn't.
pub fn pieces(text: &str) -> Vec<Piece<'_>> {
    let mut found = Vec::new();
    let mut run_start = 0;
    for (at, c) in text.char_indices() {
        if !is_hidden(c) {
            continue;
        }
        if run_start < at {
            found.push(Piece::Text(&text[run_start..at]));
        }
        found.push(Piece::Mark(mark(c)));
        run_start = at + c.len_utf8();
    }
    if run_start < text.len() {
        found.push(Piece::Text(&text[run_start..]));
    }
    found
}

/// `text` with its hidden characters written as marks.
pub fn visible(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_hidden) {
        return Cow::Borrowed(text);
    }
    let shown: String = pieces(text)
        .into_iter()
        .map(|piece| match piece {
            Piece::Text(run) => Cow::Borrowed(run),
            Piece::Mark(mark) => Cow::Owned(mark),
        })
        .collect();
    Cow::Owned(shown)
}

/// `text` in `font` and `color`, its marks in `MARK_COLOR`.
pub fn job(text: &str, font: FontId, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    append(&mut job, text, font, color);
    job
}

/// Adds `text` to `job` as `job` does.
pub fn append(job: &mut LayoutJob, text: &str, font: FontId, color: Color32) {
    for piece in pieces(text) {
        let (run, color) = match &piece {
            Piece::Text(run) => (*run, color),
            Piece::Mark(mark) => (mark.as_str(), MARK_COLOR),
        };
        job.append(run, 0.0, TextFormat::simple(font.clone(), color));
    }
}

/// `‹LF›`, `‹TAB›`, or `‹U+202E›`.
fn mark(c: char) -> String {
    match c {
        '\n' => "‹LF›".to_string(),
        '\r' => "‹CR›".to_string(),
        '\t' => "‹TAB›".to_string(),
        c => format!("‹U+{:04X}›", u32::from(c)),
    }
}

/// Characters that don't show as themselves: control and format
/// characters, line and paragraph separators, spaces other than the plain
/// one, fillers that draw as blank, variation selectors, and tags.
pub fn is_hidden(c: char) -> bool {
    c.is_control()
        || (c.is_whitespace() && c != ' ')
        || matches!(
            c,
            '\u{00AD}'
                | '\u{034F}'
                | '\u{061C}'
                | '\u{115F}'..='\u{1160}'
                | '\u{17B4}'..='\u{17B5}'
                | '\u{180B}'..='\u{180F}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{3164}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{FFF0}'..='\u{FFFB}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0000}'..='\u{E007F}'
                | '\u{E0100}'..='\u{E01EF}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_shown_as_it_is() {
        assert_eq!(visible("\"plain\" text"), "\"plain\" text");
        assert!(matches!(visible("\"plain\""), Cow::Borrowed(_)));
        assert_eq!(pieces("abc"), [Piece::Text("abc")]);
    }

    #[test]
    fn hidden_characters_are_shown_as_marks() {
        assert_eq!(visible("a\nb\tc\r"), "a‹LF›b‹TAB›c‹CR›");
        // A right-to-left override could make the text read differently.
        assert_eq!(visible("ab\u{202E}cd"), "ab‹U+202E›cd");
        for c in [
            '\u{200B}',
            '\u{2028}',
            '\u{00A0}',
            '\u{3000}',
            '\u{00AD}',
            '\u{3164}',
            '\u{FE0F}',
            '\u{E0041}',
            '\u{2066}',
            '\u{FEFF}',
        ] {
            assert!(is_hidden(c), "U+{:04X}", u32::from(c));
            assert!(
                visible(&format!("x{c}y")).contains("‹U+"),
                "U+{:04X}",
                u32::from(c)
            );
        }
    }

    #[test]
    fn a_mark_is_told_apart_from_text_that_looks_like_one() {
        // The source text `\u202E` (an escape, as written) stays text.
        assert_eq!(pieces("\\u202E"), [Piece::Text("\\u202E")]);
        assert_eq!(
            pieces("a\u{202E}b"),
            [
                Piece::Text("a"),
                Piece::Mark("‹U+202E›".into()),
                Piece::Text("b")
            ]
        );
        let shown = job("a\u{202E}b", FontId::monospace(12.0), Color32::WHITE);
        let colors: Vec<Color32> = shown.sections.iter().map(|s| s.format.color).collect();
        assert_eq!(colors, [Color32::WHITE, MARK_COLOR, Color32::WHITE]);
        assert_eq!(shown.text, "a‹U+202E›b");
    }

    #[test]
    fn ordinary_letters_and_symbols_are_not_hidden() {
        for c in ['a', ' ', 'é', '中', '★', '‹', '…'] {
            assert!(!is_hidden(c), "{c}");
        }
    }
}

//! A small hand-written C/C++ tokenizer for syntax colouring: keywords,
//! strings, comments, numbers, and preprocessor lines. Deliberately simple;
//! symbol-aware highlighting comes from the index's occurrences instead.

use std::ops::Range;

use egui::Color32;

use super::scheme::SourceColors;

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

pub(super) enum Token<'a> {
    Keyword(&'a str),
    StringLit(&'a str),
    Comment(&'a str),
    Number(&'a str),
    Preproc(&'a str),
    Plain(&'a str),
}

pub(super) fn tokenize_line(line: &str) -> Vec<Token<'_>> {
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

/// Each token's byte range in `line` and its colour.
pub(super) fn colored_ranges(line: &str, colors: &SourceColors) -> Vec<(Range<usize>, Color32)> {
    let mut start = 0;
    tokenize_line(line)
        .iter()
        .map(|token| {
            let end = start + token_text(token).len();
            let range = start..end;
            start = end;
            (range, token_color(token, colors))
        })
        .collect()
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

fn token_color(token: &Token, colors: &SourceColors) -> Color32 {
    match token {
        Token::Keyword(_) => colors.keyword,
        Token::StringLit(_) => colors.string,
        Token::Comment(_) => colors.comment,
        Token::Number(_) => colors.number,
        Token::Preproc(_) => colors.preproc,
        Token::Plain(_) => colors.text,
    }
}

pub(super) fn token_text<'a>(token: &Token<'a>) -> &'a str {
    match token {
        Token::Keyword(s)
        | Token::StringLit(s)
        | Token::Comment(s)
        | Token::Number(s)
        | Token::Preproc(s)
        | Token::Plain(s) => s,
    }
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

    #[test]
    fn colored_ranges_tile_the_line() {
        let line = "int naïve = f(1); // é";
        let ranges = colored_ranges(line, &SourceColors::default());
        assert_eq!(ranges.first().map(|(r, _)| r.start), Some(0));
        assert_eq!(ranges.last().map(|(r, _)| r.end), Some(line.len()));
        assert!(ranges.windows(2).all(|w| w[0].0.end == w[1].0.start));
    }
}

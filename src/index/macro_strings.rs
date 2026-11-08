//! Strings in a macro's body (`#define LOG(x) printf("hi %s", x)`), which
//! tree-sitter keeps as plain text: found by a small scanner that knows
//! comments, character literals, escapes and raw strings.

use std::ops::Range;

/// The longest raw-string delimiter C++ allows.
const MAX_RAW_DELIMITER: usize = 16;
/// Prefixes a string literal may have: wide, UTF-8/16/32, raw.
const PREFIXES: [&str; 9] = ["L", "u8", "u", "U", "R", "LR", "u8R", "uR", "UR"];

/// The byte ranges of the string literals in `body`, adjacent ones (with
/// only blanks or line continuations between) grouped together.
pub fn literals(body: &str) -> Vec<Vec<Range<usize>>> {
    let bytes = body.as_bytes();
    let mut groups: Vec<Vec<Range<usize>>> = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        at = match (bytes[at], bytes.get(at + 1)) {
            (b'/', Some(b'/')) => line_end(bytes, at),
            (b'/', Some(b'*')) => comment_end(bytes, at),
            (b'\'', _) => quoted_end(bytes, at, b'\''),
            (b'"', _) => {
                let start = prefix_start(bytes, at);
                let is_raw = bytes[start..at].ends_with(b"R");
                let end = if is_raw {
                    raw_end(bytes, at)
                } else {
                    quoted_end(bytes, at, b'"')
                };
                add_literal(&mut groups, body, start..end);
                end
            }
            _ => at + 1,
        };
    }
    groups
}

/// Adds `literal` to the last group if only blanks separate them, else
/// starts a new group.
fn add_literal(groups: &mut Vec<Vec<Range<usize>>>, body: &str, literal: Range<usize>) {
    let previous_end = groups.last().and_then(|g| g.last()).map(|r| r.end);
    let is_adjacent = previous_end.is_some_and(|end| {
        body.get(end..literal.start)
            .is_some_and(|gap| gap.chars().all(|c| c.is_whitespace() || c == '\\'))
    });
    match groups.last_mut() {
        Some(group) if is_adjacent => group.push(literal),
        _ => groups.push(vec![literal]),
    }
}

/// Where a literal whose quote is at `quote` starts: at its prefix, if it
/// has one (`L"…"`, `u8"…"`, `R"(…)"`).
fn prefix_start(bytes: &[u8], quote: usize) -> usize {
    let is_name = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let start = bytes[..quote]
        .iter()
        .rposition(|&b| !is_name(b))
        .map_or(0, |i| i + 1);
    let word = std::str::from_utf8(&bytes[start..quote]).unwrap_or("");
    if PREFIXES.contains(&word) {
        start
    } else {
        quote
    }
}

/// Past the closing `close` of the literal opened at `open`, skipping
/// escapes; or the end of the line if it isn't closed there.
fn quoted_end(bytes: &[u8], open: usize, close: u8) -> usize {
    let mut at = open + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b if b == close => return at + 1,
            b'\n' => return at,
            _ => at += 1,
        }
    }
    bytes.len()
}

/// Past the `)delimiter"` closing the raw string opened at `quote`.
fn raw_end(bytes: &[u8], quote: usize) -> usize {
    let opened = bytes[quote + 1..]
        .iter()
        .take(MAX_RAW_DELIMITER + 1)
        .position(|&b| b == b'(');
    let Some(length) = opened else {
        return quoted_end(bytes, quote, b'"');
    };
    let delimiter = &bytes[quote + 1..quote + 1 + length];
    let closing: Vec<u8> = [b")", delimiter, b"\""].concat();
    let body_start = quote + 2 + length;
    bytes[body_start..]
        .windows(closing.len())
        .position(|w| w == closing.as_slice())
        .map_or(bytes.len(), |i| body_start + i + closing.len())
}

fn line_end(bytes: &[u8], at: usize) -> usize {
    bytes[at..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(bytes.len(), |i| at + i)
}

fn comment_end(bytes: &[u8], at: usize) -> usize {
    bytes[at + 2..]
        .windows(2)
        .position(|w| w == b"*/")
        .map_or(bytes.len(), |i| at + 2 + i + 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each group's literals, as written.
    fn found(body: &str) -> Vec<Vec<&str>> {
        literals(body)
            .into_iter()
            .map(|group| group.into_iter().map(|r| &body[r]).collect())
            .collect()
    }

    #[test]
    fn skips_comments_and_character_literals() {
        assert_eq!(
            found(r#"'"' "a" /* "b" */ "c" // "d""#),
            [vec![r#""a""#], vec![r#""c""#]]
        );
    }

    #[test]
    fn keeps_escaped_quotes_and_prefixes() {
        assert_eq!(found(r#"L"a\"b" u8"c""#), [vec![r#"L"a\"b""#, r#"u8"c""#]]);
        // `xL"…"` is a name next to a string, not a prefix.
        assert_eq!(found(r#"xL"a""#), [vec![r#""a""#]]);
    }

    #[test]
    fn reads_raw_strings_to_their_delimiter() {
        assert_eq!(
            found(r#"R"x(a")b)x" "c""#),
            [vec![r#"R"x(a")b)x""#, r#""c""#]]
        );
    }

    #[test]
    fn groups_adjacent_literals_across_line_continuations() {
        assert_eq!(
            found("\"a\" \\\n  \"b\" SEP \"c\""),
            [vec!["\"a\"", "\"b\""], vec!["\"c\""]]
        );
    }

    #[test]
    fn an_unclosed_string_or_comment_stops_at_the_end() {
        assert_eq!(found("\"open"), [vec!["\"open"]]);
        assert!(found("/* open \"x\"").is_empty());
        assert_eq!(found("R\"x(never"), [vec!["R\"x(never"]]);
    }
}

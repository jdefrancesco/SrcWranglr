//! The comment that documents a declaration: the block or run of line
//! comments just above it (past the lines of its own signature), the way
//! Doxygen and readers pair them. Read from the text, so it works where
//! macros keep the parser from making sense of the code.

/// Lines above the name that may still be its declaration: a return type,
/// `static`, a template line, an export macro.
const MAX_SIGNATURE_LINES: usize = 4;
/// Blank lines allowed between a comment and what it documents.
const MAX_GAP_LINES: usize = 1;
/// A longer comment is not read past this many lines.
const MAX_COMMENT_LINES: usize = 300;
/// Comment text kept, in bytes.
const MAX_TEXT_BYTES: usize = 16 * 1024;
/// Separator comments (`/* ===== */`) skipped on the way up.
const MAX_SEPARATORS: usize = 2;
/// Words that mark a file's license header rather than a function's doc.
const LICENSE_WORDS: [&str; 4] = [
    "Copyright",
    "SPDX-License",
    "Permission is hereby granted",
    "Licensed under",
];

/// A comment documenting a declaration, without its comment markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocComment {
    pub text: String,
    /// Written for Doxygen: `/**`, `/*!`, `///` or `//!`.
    pub is_doxygen: bool,
    /// 1-based line it starts on.
    pub line: u32,
}

/// The comment documenting the declaration whose name is on `line`
/// (1-based) of `source`, if there is one.
pub fn comment_above(source: &str, line: u32) -> Option<DocComment> {
    let wanted = usize::try_from(line).ok()?;
    let lines: Vec<&str> = source.split('\n').take(wanted).collect();
    let name_line = wanted.checked_sub(1).filter(|&i| i < lines.len())?;
    let mut end = skip_signature(&lines, name_line)?;
    for _ in 0..=MAX_SEPARATORS {
        let (start, raw) = comment_ending_at(&lines, end)?;
        let comment = cleaned(&raw);
        if !is_separator(&comment.text) {
            let is_license =
                !comment.is_doxygen && LICENSE_WORDS.iter().any(|word| comment.text.contains(word));
            let line = u32::try_from(start + 1).ok()?;
            return (!is_license && !comment.text.is_empty())
                .then_some(DocComment { line, ..comment });
        }
        end = skip_gap(&lines, start)?;
    }
    None
}

/// The index of the line just above the declaration's signature (blank
/// lines skipped), where its comment would end.
fn skip_signature(lines: &[&str], name_line: usize) -> Option<usize> {
    let mut above = name_line;
    for _ in 0..MAX_SIGNATURE_LINES {
        let candidate = above.checked_sub(1)?;
        if !is_signature_line(lines[candidate]) {
            break;
        }
        above = candidate;
    }
    skip_gap(lines, above)
}

/// The index of the first line above `index` that isn't blank, allowing
/// `MAX_GAP_LINES` blank ones.
fn skip_gap(lines: &[&str], index: usize) -> Option<usize> {
    let mut at = index.checked_sub(1)?;
    for _ in 0..MAX_GAP_LINES {
        if !lines[at].trim().is_empty() {
            break;
        }
        at = at.checked_sub(1)?;
    }
    (!lines[at].trim().is_empty()).then_some(at)
}

/// Part of a declaration spread over lines (a return type, `static`, an
/// attribute): not blank, not a comment, a directive, or the end of
/// something else.
fn is_signature_line(line: &str) -> bool {
    let trimmed = without_trailing_comment(line).trim();
    !trimmed.is_empty()
        && !trimmed.starts_with('#')
        && !trimmed.starts_with("//")
        && !trimmed.ends_with("*/")
        && !trimmed.ends_with(';')
        && !trimmed.ends_with('}')
        && !trimmed.ends_with('{')
}

/// `line` without a comment at its end (`int x; // counter`), so what it
/// ends in is the code's. A line that is all comment is left as it is.
fn without_trailing_comment(line: &str) -> &str {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with("/*") {
        return line;
    }
    let cut = [line.find("//"), line.find("/*")]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(line.len());
    &line[..cut]
}

/// The comment whose last line is `end`: where it starts, and its lines.
fn comment_ending_at(lines: &[&str], end: usize) -> Option<(usize, Vec<String>)> {
    let last = lines[end].trim();
    if last.starts_with("//") {
        let start = (0..=end)
            .rev()
            .take(MAX_COMMENT_LINES)
            .take_while(|&i| lines[i].trim().starts_with("//"))
            .last()?;
        let raw = lines[start..=end].iter().map(|l| l.trim().to_string());
        return Some((start, raw.collect()));
    }
    if !last.ends_with("*/") {
        return None;
    }
    let start = (0..=end)
        .rev()
        .take(MAX_COMMENT_LINES)
        .find(|&i| lines[i].contains("/*"))?;
    let opening = lines[start];
    let at = opening.rfind("/*")?;
    // `int x; /* about x */` is about the code before it.
    if !opening[..at].trim().is_empty() {
        return None;
    }
    let first = opening[at..].to_string();
    let rest = lines[start + 1..=end].iter().map(|l| l.to_string());
    Some((start, std::iter::once(first).chain(rest).collect()))
}

/// The comment's text without its markers, and whether it was written for
/// Doxygen.
fn cleaned(raw: &[String]) -> DocComment {
    let first = raw.first().map(String::as_str).unwrap_or_default();
    let is_line_comment = first.starts_with("//");
    let is_doxygen = match is_line_comment {
        true => {
            (first.starts_with("///") && !first.starts_with("////")) || first.starts_with("//!")
        }
        false => {
            (first.starts_with("/**") && !first.starts_with("/***")) || first.starts_with("/*!")
        }
    };
    let lines: Vec<String> = raw
        .iter()
        .enumerate()
        .map(|(i, line)| match is_line_comment {
            true => strip_line_comment(line),
            false => strip_block_line(line, i == 0, i + 1 == raw.len()),
        })
        .collect();
    let text = trimmed_lines(&lines);
    DocComment {
        text: cut(text),
        is_doxygen,
        line: 0,
    }
}

fn strip_line_comment(line: &str) -> String {
    let body = line
        .trim()
        .trim_start_matches('/')
        .strip_prefix('!')
        .unwrap_or_else(|| line.trim().trim_start_matches('/'));
    body.strip_prefix(' ')
        .unwrap_or(body)
        .trim_end()
        .to_string()
}

fn strip_block_line(line: &str, is_first: bool, is_last: bool) -> String {
    let mut body = line.trim();
    if is_first {
        body = body.trim_start_matches("/*");
        body = body.trim_start_matches(['*', '!']);
    }
    if is_last {
        body = body.trim_end().trim_end_matches("*/").trim_end_matches('*');
    }
    if !is_first {
        // ` * text`: the leading star of a decorated block.
        body = body
            .strip_prefix('*')
            .filter(|rest| !rest.starts_with('/'))
            .unwrap_or(body);
    }
    body.strip_prefix(' ')
        .unwrap_or(body)
        .trim_end()
        .to_string()
}

/// `lines` joined, without the blank ones at either end.
fn trimmed_lines(lines: &[String]) -> String {
    let is_text = |l: &&String| !l.trim().is_empty();
    let start = lines.iter().position(|l| is_text(&l));
    let end = lines.iter().rposition(|l| is_text(&l));
    match (start, end) {
        (Some(start), Some(end)) => lines[start..=end].join("\n"),
        _ => String::new(),
    }
}

/// `===`, `---`, `***`: a line drawn across the file, not a doc.
fn is_separator(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_whitespace() || matches!(c, '=' | '-' | '*' | '/' | '#' | '~' | '+'))
}

fn cut(text: String) -> String {
    if text.len() <= MAX_TEXT_BYTES {
        return text;
    }
    let mut end = MAX_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
#[path = "comments_tests.rs"]
mod tests;

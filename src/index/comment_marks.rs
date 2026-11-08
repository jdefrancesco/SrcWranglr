//! Comments worth a look, found while indexing: ones telling an analyzer
//! not to report (`// nosemgrep`, `/* Flawfinder: ignore */`,
//! `// cppcheck-suppress nullPointer`, `// NOLINT`, ...), and tags left in
//! the code (`TODO`, `FIXME`, `XXX`, `HACK`, `BUG`, Doxygen's `\todo` and
//! `\bug`).
//!
//! Tags must be written in capitals, as whole words, so prose ("todo
//! list", "debugging") isn't taken for one. Directives are matched the way
//! their tools read them, ignoring case where the tool does.

use std::collections::HashMap;
use std::ops::Range;

use tree_sitter::Node;

use super::placement;
use super::resolve::SourceFile;
use crate::model::{Analyzer, CommentMark, CommentMarkKind, FunctionMetrics, SourceTag, Span};

/// Marks of each kind (tags, silencing comments) kept per file: a file of
/// generated TODOs shouldn't take over, nor crowd out the other kind.
pub const MAX_MARKS_PER_FILE: usize = 2_000;
/// Marks of each kind kept in all, so many files of them can't either.
pub const MAX_MARKS_TOTAL: usize = 100_000;
/// A mark's text longer than this, in characters, is cut short (with `…`).
pub const MAX_MARK_CHARS: usize = 200;

/// One mark in a file, before it is placed in a function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkFact {
    pub kind: CommentMarkKind,
    pub span: Span,
    pub text: String,
}

/// How many marks of each kind a file may keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkLimits {
    pub tags: usize,
    pub silencers: usize,
}

/// A file's marks, and of which kinds there were more than were kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileMarks {
    pub marks: Vec<MarkFact>,
    pub are_tags_cut_short: bool,
    pub are_silencers_cut_short: bool,
}

impl FileMarks {
    /// How many marks of each kind were kept: tags, then silencers.
    pub fn counts(&self) -> (usize, usize) {
        let tags = self
            .marks
            .iter()
            .filter(|m| matches!(m.kind, CommentMarkKind::Tag(_)))
            .count();
        (tags, self.marks.len() - tags)
    }
}

/// The marks in the comments of the tree at `root` (of `src`), in file
/// order; at most `limits` of each kind.
pub fn collect(root: Node, src: &str, limits: MarkLimits) -> FileMarks {
    let mut found = FileMarks::default();
    let (mut tags, mut silencers) = (0, 0);
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        if node.kind() == "comment" {
            for mark in comment_marks(node, src) {
                let (kept, limit, cut_short) = match mark.kind {
                    CommentMarkKind::Tag(_) => {
                        (&mut tags, limits.tags, &mut found.are_tags_cut_short)
                    }
                    CommentMarkKind::Silences(_) => (
                        &mut silencers,
                        limits.silencers,
                        &mut found.are_silencers_cut_short,
                    ),
                };
                if *kept >= limit {
                    *cut_short = true;
                    continue;
                }
                *kept += 1;
                found.marks.push(mark);
            }
            if found.are_tags_cut_short && found.are_silencers_cut_short {
                return found;
            }
        } else if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return found;
            }
        }
    }
}

/// The marks in the comment at `node`, line by line.
fn comment_marks<'s>(node: Node, src: &'s str) -> impl Iterator<Item = MarkFact> + 's {
    let text = src.get(node.byte_range()).unwrap_or("");
    let start = node.start_position();
    text.split('\n').enumerate().flat_map(move |(i, line)| {
        // Lines after the first start at the start of a source line.
        let column = if i == 0 { start.column } else { 0 };
        let row = start.row.checked_add(i);
        marks_on_line(line)
            .into_iter()
            .filter_map(move |(kind, found)| mark_at(kind, line, found, row?, column))
    })
}

/// A mark of `kind` found at `found` in `line`, which starts at `row`,
/// `column` (0-based) of the file.
fn mark_at(
    kind: CommentMarkKind,
    line: &str,
    found: Range<usize>,
    row: usize,
    column: usize,
) -> Option<MarkFact> {
    let start = u32::try_from(column.checked_add(found.start)?).ok()?;
    let span = Span {
        line: u32::try_from(row.checked_add(1)?).ok()?,
        start,
        end: start.checked_add(u32::try_from(found.len()).ok()?)?,
    };
    let rest = line.get(found.start..)?;
    let rest = rest.trim_end();
    let rest = rest.strip_suffix("*/").unwrap_or(rest).trim_end();
    Some(MarkFact {
        kind,
        span,
        text: cut_short(rest),
    })
}

/// What `line` of a comment marks, each with where it is written, in
/// line order; each kind once.
pub fn marks_on_line(line: &str) -> Vec<(CommentMarkKind, Range<usize>)> {
    let lower = line.to_ascii_lowercase();
    // Most comment lines hold none of this: skip them cheaply.
    if !MAY_MARK.iter().any(|needle| lower.contains(needle)) {
        return Vec::new();
    }
    let directives = ANALYZERS.iter().filter_map(|&analyzer| {
        Some((
            CommentMarkKind::Silences(analyzer),
            directive(analyzer, line, &lower)?,
        ))
    });
    let tags = SourceTag::ALL
        .iter()
        .filter_map(|&tag| Some((CommentMarkKind::Tag(tag), tag_at(tag, line, &lower)?)));
    let mut found: Vec<(CommentMarkKind, Range<usize>)> = directives.chain(tags).collect();
    found.sort_by_key(|(_, at)| at.start);
    found
}

/// Lower-case text every mark contains some of.
const MAY_MARK: [&str; 15] = [
    "nosem",
    "ignore",
    "cppcheck-suppress",
    "nolint",
    "coverity[",
    "codeql[",
    "lgtm[",
    "nosonar",
    "//-v",
    "todo",
    "fixme",
    "xxx",
    "hack",
    "bug",
    "@ignore@",
];

const ANALYZERS: [Analyzer; 9] = [
    Analyzer::Semgrep,
    Analyzer::Flawfinder,
    Analyzer::Cppcheck,
    Analyzer::ClangTidy,
    Analyzer::Coverity,
    Analyzer::CodeQl,
    Analyzer::SonarQube,
    Analyzer::PvsStudio,
    Analyzer::Splint,
];

/// Where `line` (and `lower`, the same in lower case) tells `analyzer` not
/// to report.
fn directive(analyzer: Analyzer, line: &str, lower: &str) -> Option<Range<usize>> {
    match analyzer {
        // `nosem` / `nosemgrep`, with or without `: rule-id`.
        Analyzer::Semgrep => find_word(lower, "nosemgrep").or_else(|| find_word(lower, "nosem")),
        // `Flawfinder: ignore` (also RATS's and ITS4's, which it reads too).
        Analyzer::Flawfinder => ["flawfinder", "rats", "its4"]
            .iter()
            .find_map(|name| colon_ignore(lower, name)),
        // `cppcheck-suppress`, `-file`, `-begin`, `-macro`, ...
        Analyzer::Cppcheck => token_from(lower, "cppcheck-suppress"),
        // `NOLINT`, `NOLINTNEXTLINE`, `NOLINTBEGIN`: capitals only.
        Analyzer::ClangTidy => {
            let at = find_word_start(line, "NOLINT")?;
            Some(at.start..token_end(line, at.start))
        }
        Analyzer::Coverity => token_from(lower, "coverity["),
        Analyzer::CodeQl => token_from(lower, "codeql[").or_else(|| token_from(lower, "lgtm[")),
        Analyzer::SonarQube => find_word(lower, "nosonar"),
        // `//-V547`, `//-V::1037`.
        Analyzer::PvsStudio => {
            let at = lower.find("//-v")?;
            let next = lower.as_bytes().get(at + 4)?;
            (next.is_ascii_digit() || *next == b':').then(|| at + 2..token_end(lower, at))
        }
        Analyzer::Splint => token_from(lower, "@ignore@"),
    }
}

/// Where `tag` is written in `line`: in capitals as a whole word, or as a
/// Doxygen command (`\todo`, `@bug`).
fn tag_at(tag: SourceTag, line: &str, lower: &str) -> Option<Range<usize>> {
    let word = find_word(line, tag.label());
    let doxygen = match tag {
        SourceTag::Todo => doxygen_command(lower, ["\\todo", "@todo"]),
        SourceTag::Bug => doxygen_command(lower, ["\\bug", "@bug"]),
        _ => None,
    };
    match (word, doxygen) {
        (Some(a), Some(b)) => Some(if a.start <= b.start { a } else { b }),
        (a, b) => a.or(b),
    }
}

/// One of `commands` (`\\name` or `@name`) as a whole word in `lower`.
fn doxygen_command(lower: &str, commands: [&str; 2]) -> Option<Range<usize>> {
    commands.iter().find_map(|command| {
        let at = find_word_start(lower, command)?;
        is_word_end(lower, at.end).then_some(at)
    })
}

/// `name` as a word, then `:` and `ignore`, spaces allowed around the `:`.
fn colon_ignore(lower: &str, name: &str) -> Option<Range<usize>> {
    words(lower, name).find_map(|at| {
        let rest = lower.get(at.end..)?;
        let after_name = rest.trim_start();
        let after_colon = after_name.strip_prefix(':')?.trim_start();
        after_colon.starts_with("ignore").then(|| {
            let end = lower.len() - after_colon.len() + "ignore".len();
            at.start..end
        })
    })
}

/// `prefix` and the rest of the word it starts.
fn token_from(lower: &str, prefix: &str) -> Option<Range<usize>> {
    let start = lower.find(prefix)?;
    Some(start..token_end(lower, start))
}

/// Where the run of non-blank characters from `start` ends.
fn token_end(text: &str, start: usize) -> usize {
    text.get(start..)
        .and_then(|rest| rest.find(|c: char| c.is_whitespace()))
        .map_or(text.len(), |length| start + length)
}

/// The first whole-word `word` in `text`.
fn find_word(text: &str, word: &str) -> Option<Range<usize>> {
    words(text, word).next()
}

/// Each place `word` is written in `text` as a whole word.
fn words<'t>(text: &'t str, word: &'t str) -> impl Iterator<Item = Range<usize>> + 't {
    starts(text, word).filter(move |at| is_word_end(text, at.end))
}

/// The first `word` in `text` that starts a word (it may go on).
fn find_word_start(text: &str, word: &str) -> Option<Range<usize>> {
    starts(text, word).next()
}

fn starts<'t>(text: &'t str, word: &'t str) -> impl Iterator<Item = Range<usize>> + 't {
    text.match_indices(word)
        .map(|(at, found)| at..at + found.len())
        .filter(move |at| at.start == 0 || !is_word_byte(text.as_bytes()[at.start - 1]))
}

fn is_word_end(text: &str, end: usize) -> bool {
    text.as_bytes().get(end).is_none_or(|&b| !is_word_byte(b))
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn cut_short(text: &str) -> String {
    let mut chars = text.chars();
    let kept: String = chars.by_ref().take(MAX_MARK_CHARS).collect();
    match chars.next() {
        Some(_) => kept + "…",
        None => kept,
    }
}

/// The graph's comment marks, moved out of `files`: each file's (that has
/// any), placed in the function they are in.
pub fn by_file(
    files: &mut [SourceFile],
    functions: &[FunctionMetrics],
) -> HashMap<String, Vec<CommentMark>> {
    let functions_in = placement::by_file(functions);
    files
        .iter_mut()
        .filter_map(|file| {
            let facts = std::mem::take(&mut file.facts.comment_marks.marks);
            if facts.is_empty() {
                return None;
            }
            let in_file = functions_in
                .get(file.path.as_str())
                .map_or(&[][..], Vec::as_slice);
            let lines: Vec<u32> = facts.iter().map(|fact| fact.span.line).collect();
            let holding = placement::functions_holding(&lines, in_file);
            let placed = facts
                .into_iter()
                .zip(holding)
                .map(|(fact, function)| CommentMark {
                    kind: fact.kind,
                    span: fact.span,
                    text: fact.text,
                    function,
                })
                .collect();
            Some((file.path.clone(), placed))
        })
        .collect()
}

/// The files with more tags, and with more silencing comments, than were
/// kept.
pub fn files_cut_short(files: &[SourceFile]) -> (Vec<String>, Vec<String>) {
    let which = |is_cut: fn(&FileMarks) -> bool| -> Vec<String> {
        files
            .iter()
            .filter(|file| is_cut(&file.facts.comment_marks))
            .map(|file| file.path.clone())
            .collect()
    };
    (
        which(|m| m.are_tags_cut_short),
        which(|m| m.are_silencers_cut_short),
    )
}

#[cfg(test)]
#[path = "comment_marks_tests.rs"]
mod tests;

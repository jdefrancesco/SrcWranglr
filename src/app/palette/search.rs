//! What the quick-open palette lists for a query: symbols by fuzzy match on
//! their qualified name, or app commands when the query starts with `>`.

use std::cmp::Reverse;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};

use crate::model::SymbolId;

/// Typing this first switches the palette from symbols to commands.
pub const COMMAND_PREFIX: char = '>';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    OpenProject,
    Reindex,
    Overview,
    Back,
    Fit,
    TrailView,
    CallHierarchy,
    Quit,
}

impl Command {
    pub const ALL: [Command; 8] = [
        Command::OpenProject,
        Command::Reindex,
        Command::Overview,
        Command::Back,
        Command::Fit,
        Command::TrailView,
        Command::CallHierarchy,
        Command::Quit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Command::OpenProject => "Open Project…",
            Command::Reindex => "Re-index Project",
            Command::Overview => "Show Overview",
            Command::Back => "Go Back",
            Command::Fit => "Fit Graph to Window",
            Command::TrailView => "View: Trail (callers and callees)",
            Command::CallHierarchy => "View: Call Hierarchy",
            Command::Quit => "Quit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Symbol(SymbolId),
    Command(Command),
}

/// Up to `limit` entries for `query`, best first. `symbols` are
/// `(id, qualified name)` in display order, used as-is for an empty query
/// and as the tie-breaker otherwise.
pub fn search<S: AsRef<str>>(symbols: &[(SymbolId, S)], query: &str, limit: usize) -> Vec<Entry> {
    match query.trim_start().strip_prefix(COMMAND_PREFIX) {
        Some(command_query) => search_commands(command_query, limit),
        None => search_symbols(symbols, query, limit),
    }
}

fn search_commands(query: &str, limit: usize) -> Vec<Entry> {
    let labels = Command::ALL.map(|c| (c, c.label()));
    ranked(&labels, query, limit)
        .into_iter()
        .map(Entry::Command)
        .collect()
}

fn search_symbols<S: AsRef<str>>(
    symbols: &[(SymbolId, S)],
    query: &str,
    limit: usize,
) -> Vec<Entry> {
    ranked(symbols, query, limit)
        .into_iter()
        .map(Entry::Symbol)
        .collect()
}

/// Fuzzy-ranked keys: names whose last `::` segment is exactly the query
/// first, then higher score, then shorter text, then input order.
fn ranked<K: Copy, S: AsRef<str>>(items: &[(K, S)], query: &str, limit: usize) -> Vec<K> {
    let query = query.trim();
    if query.is_empty() {
        return items.iter().take(limit).map(|(k, _)| *k).collect();
    }
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let is_exact = |text: &str| {
        let last = text.rsplit("::").next().unwrap_or(text);
        last.eq_ignore_ascii_case(query)
    };
    let mut keys: Vec<RankKey> = pattern
        .match_list(
            items
                .iter()
                .enumerate()
                .map(|(i, (_, text))| Indexed(i, text.as_ref())),
            &mut matcher,
        )
        .into_iter()
        .map(|(Indexed(i, text), score)| (Reverse(is_exact(text)), Reverse(score), text.len(), i))
        .collect();
    // Only the best `limit` need sorting.
    if keys.len() > limit {
        keys.select_nth_unstable(limit);
        keys.truncate(limit);
    }
    keys.sort_unstable();
    keys.into_iter().map(|(.., i)| items[i].0).collect()
}

/// Sort key: exact last segment, score, shorter text, input position.
type RankKey = (Reverse<bool>, Reverse<u32>, usize, usize);

/// A candidate's text tagged with its position in the input.
struct Indexed<'a>(usize, &'a str);

impl AsRef<str> for Indexed<'_> {
    fn as_ref(&self) -> &str {
        self.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYMBOLS: &[(SymbolId, &str)] = &[
        (0, "leveldb::DBImpl"),
        (1, "leveldb::DBImpl::MakeRoomForWrite"),
        (2, "leveldb::DBImpl::Write"),
        (3, "leveldb::WriteBatch"),
        (4, "main"),
    ];

    fn symbols(query: &str) -> Vec<SymbolId> {
        search(SYMBOLS, query, 10)
            .into_iter()
            .map(|e| match e {
                Entry::Symbol(id) => id,
                Entry::Command(c) => panic!("unexpected command {c:?}"),
            })
            .collect()
    }

    #[test]
    fn fuzzy_matches_scattered_letters() {
        assert_eq!(symbols("dbimplwr").first(), Some(&2));
        assert!(symbols("mkroom").contains(&1));
    }

    #[test]
    fn prefers_tighter_and_shorter_matches() {
        assert_eq!(symbols("write").first(), Some(&2));
        assert_eq!(symbols("main"), [4]);
    }

    #[test]
    fn empty_query_lists_symbols_in_order_up_to_the_limit() {
        let first_three: Vec<Entry> = [0, 1, 2].map(Entry::Symbol).into();
        assert_eq!(search(SYMBOLS, "  ", 3), first_three);
    }

    #[test]
    fn no_match_gives_nothing() {
        assert!(symbols("zzzq").is_empty());
    }

    #[test]
    fn prefix_switches_to_commands() {
        assert_eq!(
            search(SYMBOLS, ">hier", 10),
            [Entry::Command(Command::CallHierarchy)]
        );
        assert_eq!(search(SYMBOLS, ">", 100).len(), Command::ALL.len());
        assert!(search(SYMBOLS, " >quit", 10).contains(&Entry::Command(Command::Quit)));
    }

    #[test]
    fn limit_caps_results_keeping_the_best() {
        assert_eq!(search(SYMBOLS, "l", 2).len(), 2);
        assert_eq!(search(SYMBOLS, "write", 1), [Entry::Symbol(2)]);
    }
}

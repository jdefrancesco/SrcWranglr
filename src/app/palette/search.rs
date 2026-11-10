//! What the quick-open palette lists for a query: symbols by fuzzy match on
//! their qualified name, app commands when the query starts with `>`, or
//! notes and bookmarks when it starts with `#`.

use std::cmp::Reverse;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};

use crate::app::pane_bar::Pane;
use crate::app::survey_view::SurveyTab;
use crate::model::SymbolId;

/// Typing this first switches the palette from symbols to commands.
pub const COMMAND_PREFIX: char = '>';
/// Typing this first switches the palette to notes and bookmarks.
pub const NOTE_PREFIX: char = '#';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    OpenProject,
    /// Reopens the project at this position in the recent list.
    OpenRecent(usize),
    OpenDemo,
    Reindex,
    Overview,
    GoToEntryPoint,
    OpenInEditor,
    ToggleBookmark,
    EditNote,
    /// Shows the pane if hidden, hides it if shown.
    TogglePane(Pane),
    MarkDangerous,
    TrustProject,
    RunSemgrep,
    SemgrepRules,
    RunCppcheck,
    RunFlawfinder,
    Settings,
    SaveSession,
    OpenSession,
    ExportBundle,
    ImportBundle,
    /// Opens the project's README in a window.
    ShowReadme,
    About,
    Back,
    Fit,
    TrailView,
    CallHierarchy,
    SwapSourcePane,
    ResetLayout,
    /// Opens the Project Triage window on this tab.
    Triage(SurveyTab),
    Quit,
}

impl Command {
    pub const ALL: [Command; 44] = [
        Command::OpenProject,
        Command::OpenDemo,
        Command::Reindex,
        Command::Overview,
        Command::GoToEntryPoint,
        Command::OpenInEditor,
        Command::ToggleBookmark,
        Command::EditNote,
        Command::TogglePane(Pane::Notes),
        Command::TogglePane(Pane::Findings),
        Command::TogglePane(Pane::Strings),
        Command::TogglePane(Pane::History),
        Command::TogglePane(Pane::Matching),
        Command::MarkDangerous,
        Command::TrustProject,
        Command::RunSemgrep,
        Command::SemgrepRules,
        Command::RunCppcheck,
        Command::RunFlawfinder,
        Command::Settings,
        Command::SaveSession,
        Command::OpenSession,
        Command::ExportBundle,
        Command::ImportBundle,
        Command::ShowReadme,
        Command::TogglePane(Pane::Todos),
        Command::TogglePane(Pane::Trace),
        Command::About,
        Command::Back,
        Command::Fit,
        Command::TrailView,
        Command::CallHierarchy,
        Command::SwapSourcePane,
        Command::TogglePane(Pane::Symbols),
        Command::TogglePane(Pane::Source),
        Command::ResetLayout,
        Command::Triage(SurveyTab::Overview),
        Command::Triage(SurveyTab::Languages),
        Command::Triage(SurveyTab::Links),
        Command::Triage(SurveyTab::Binaries),
        Command::Triage(SurveyTab::Ipc),
        Command::Triage(SurveyTab::Complexity),
        Command::Triage(SurveyTab::Fuzz),
        Command::Quit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Command::OpenProject => "Open Project…",
            Command::OpenRecent(_) => "Open Recent Project",
            Command::OpenDemo => "Open the Built-in Demo",
            Command::Reindex => "Re-index Project",
            Command::Overview => "Show Overview",
            Command::GoToEntryPoint => "Go to Entry Point (main)",
            Command::OpenInEditor => "Open in Editor",
            Command::ToggleBookmark => "Toggle Bookmark on Symbol",
            Command::EditNote => "Note on Symbol…",
            Command::TogglePane(pane) => pane.title(),
            Command::MarkDangerous => "Mark Selected Function as Dangerous…",
            Command::TrustProject => "Trust This Project's Verdicts, List and Rules",
            Command::RunSemgrep => "Run semgrep",
            Command::SemgrepRules => "Semgrep Rules…",
            Command::RunCppcheck => "Run cppcheck",
            Command::RunFlawfinder => "Run flawfinder",
            Command::Settings => "Settings…",
            Command::SaveSession => "Save Session As…",
            Command::OpenSession => "Open Session…",
            Command::ExportBundle => "Export Audit Bundle…",
            Command::ImportBundle => "Import Audit Bundle…",
            Command::ShowReadme => "Show Project README",
            Command::About => "About SrcWranglr",
            Command::Back => "Go Back",
            Command::Fit => "Fit Graph to Window",
            Command::TrailView => "View: Trail (callers and callees)",
            Command::CallHierarchy => "View: Call Hierarchy",
            Command::SwapSourcePane => "View: Move Source Pane Below or Beside the Graph",
            Command::ResetLayout => "View: Reset Layout",
            Command::Triage(tab) => tab.command_label(),
            Command::Quit => "Quit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Symbol(SymbolId),
    Command(Command),
    /// A note, by its index in the notes passed to `search`.
    Note(usize),
}

/// Up to `limit` entries for `query`, best first. `symbols` are
/// `(id, qualified name)` and `notes` `(index, label)`, in display order,
/// used as-is for an empty query and as the tie-breaker otherwise.
pub fn search<S: AsRef<str>, N: AsRef<str>>(
    symbols: &[(SymbolId, S)],
    notes: &[(usize, N)],
    query: &str,
    limit: usize,
) -> Vec<Entry> {
    let query = query.trim_start();
    if let Some(command_query) = query.strip_prefix(COMMAND_PREFIX) {
        return search_commands(command_query, limit);
    }
    if let Some(note_query) = query.strip_prefix(NOTE_PREFIX) {
        return ranked(notes, note_query, limit)
            .into_iter()
            .map(Entry::Note)
            .collect();
    }
    search_symbols(symbols, query, limit)
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

    const NO_NOTES: &[(usize, &str)] = &[];
    const NOTES: &[(usize, &str)] = &[
        (0, "★ leveldb::DBImpl::Write — takes the mutex"),
        (1, "net.c:40 — length unchecked"),
    ];

    const SYMBOLS: &[(SymbolId, &str)] = &[
        (0, "leveldb::DBImpl"),
        (1, "leveldb::DBImpl::MakeRoomForWrite"),
        (2, "leveldb::DBImpl::Write"),
        (3, "leveldb::WriteBatch"),
        (4, "main"),
    ];

    fn symbols(query: &str) -> Vec<SymbolId> {
        search(SYMBOLS, NO_NOTES, query, 10)
            .into_iter()
            .map(|e| match e {
                Entry::Symbol(id) => id,
                other => panic!("unexpected {other:?}"),
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
        assert_eq!(search(SYMBOLS, NO_NOTES, "  ", 3), first_three);
    }

    #[test]
    fn no_match_gives_nothing() {
        assert!(symbols("zzzq").is_empty());
    }

    #[test]
    fn prefix_switches_to_commands() {
        assert_eq!(
            search(SYMBOLS, NO_NOTES, ">hier", 10).first(),
            Some(&Entry::Command(Command::CallHierarchy))
        );
        assert_eq!(
            search(SYMBOLS, NO_NOTES, ">", 100).len(),
            Command::ALL.len()
        );
        assert!(search(SYMBOLS, NO_NOTES, " >quit", 10).contains(&Entry::Command(Command::Quit)));
    }

    #[test]
    fn about_is_a_command() {
        assert_eq!(
            search(SYMBOLS, NO_NOTES, ">about", 10).first(),
            Some(&Entry::Command(Command::About))
        );
        assert_eq!(Command::About.label(), "About SrcWranglr");
    }

    #[test]
    fn hash_prefix_searches_notes() {
        assert_eq!(search(SYMBOLS, NOTES, "#unchecked", 10), [Entry::Note(1)]);
        assert_eq!(
            search(SYMBOLS, NOTES, "#", 10),
            [Entry::Note(0), Entry::Note(1)]
        );
        assert!(search(SYMBOLS, NO_NOTES, "# write", 10).is_empty());
        // Without the prefix, notes aren't searched.
        assert!(!search(SYMBOLS, NOTES, "unchecked", 10).contains(&Entry::Note(1)));
    }

    #[test]
    fn limit_caps_results_keeping_the_best() {
        assert_eq!(search(SYMBOLS, NO_NOTES, "l", 2).len(), 2);
        assert_eq!(search(SYMBOLS, NO_NOTES, "write", 1), [Entry::Symbol(2)]);
    }
}

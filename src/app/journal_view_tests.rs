use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use super::*;

const NOW: u64 = 1_700_000_600;
const SAVED: &str = "Saved in .srcwranglr/journal.json";

struct State {
    view: JournalView,
    journal: Journal,
    here: Option<Place>,
    storage: Result<&'static str, &'static str>,
    action: Option<JournalAction>,
}

fn here() -> Place {
    Place {
        file: "net/parse.c".into(),
        line: 42,
        symbol: Some("parse_header".into()),
    }
}

fn sample() -> Journal {
    Journal::default()
        .with_added(Entry::new(
            "Looked at **the parser**.",
            Some(here()),
            1_700_000_000,
        ))
        .and_then(|j| {
            j.with_added(Entry::new(
                "Theory: TLV length trusted",
                None,
                1_700_000_060,
            ))
        })
        .expect("valid")
}

fn harness(
    journal: Journal,
    storage: Result<&'static str, &'static str>,
) -> Harness<'static, State> {
    let mut harness = crate::app::fonts::test_harness().build_ui_state(
        |ui, state: &mut State| {
            let journal = state.journal.clone();
            let context = JournalContext {
                journal: &journal,
                here: state.here.as_ref(),
                storage: state.storage,
                now: NOW,
            };
            let Some(action) = state.view.show(ui, &context) else {
                return;
            };
            match &action {
                JournalAction::Changed(changed) => state.journal = changed.clone(),
                JournalAction::Added(added) => {
                    state.journal = added.clone();
                    state.view.clear_draft();
                }
                JournalAction::GoTo { .. } => {}
            }
            state.action = Some(action);
        },
        State {
            view: JournalView::default(),
            journal,
            here: Some(here()),
            storage,
            action: None,
        },
    );
    harness.run();
    harness
}

fn type_draft(harness: &mut Harness<'static, State>, text: &str) {
    harness.get_by_role(egui::Role::MultilineTextInput).focus();
    harness.run();
    harness
        .get_by_role(egui::Role::MultilineTextInput)
        .type_text(text);
    harness.run();
}

#[test]
fn lists_entries_newest_first_with_when_and_where() {
    let harness = harness(sample(), Ok(SAVED));
    harness.get_by_label(SAVED);
    harness.get_by_label("2023-11-14 22:13 UTC");
    harness.get_by_label("net/parse.c:42 · parse_header");
    harness.get_by_label_contains("the parser");
    let newer = harness.get_by_label("2023-11-14 22:14 UTC").rect();
    let older = harness.get_by_label("2023-11-14 22:13 UTC").rect();
    assert!(newer.top() < older.top(), "newest first");
}

#[test]
fn an_entry_added_is_linked_to_where_you_are() {
    let mut harness = harness(Journal::default(), Ok(SAVED));
    harness.get_by_label_contains("Nothing written yet");
    type_draft(&mut harness, "Checked the length.");
    harness.get_by_label("Add Entry").click();
    harness.run();
    let entries = harness.state().journal.entries().to_vec();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].text, "Checked the length.");
    assert_eq!(entries[0].place(), Some(("net/parse.c", 42)));
    assert_eq!(entries[0].symbol.as_deref(), Some("parse_header"));
    assert_eq!(entries[0].created, NOW);
}

#[test]
fn unticking_the_link_adds_it_unlinked_with_the_shortcut() {
    let mut harness = harness(Journal::default(), Ok(SAVED));
    harness
        .get_by_label("Link to net/parse.c:42 · parse_header")
        .click();
    harness.run();
    type_draft(&mut harness, "A thought.");
    harness.key_press_modifiers(ADD_SHORTCUT.modifiers, ADD_SHORTCUT.logical_key);
    harness.run();
    let entries = harness.state().journal.entries().to_vec();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].place(), None);
}

#[test]
fn delete_asks_first() {
    let mut harness = harness(sample(), Ok(SAVED));
    harness
        .get_all_by_label("Delete…")
        .next()
        .expect("delete")
        .click();
    harness.run();
    assert_eq!(harness.state().journal.entries().len(), 2, "not yet");
    harness.get_by_label("Delete It").click();
    harness.run();
    let entries = harness.state().journal.entries().to_vec();
    // The newest (listed first) went.
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].created, 1_700_000_000);
}

#[test]
fn an_edit_is_saved_as_of_now() {
    let mut harness = harness(sample(), Ok(SAVED));
    harness
        .get_all_by_label("Edit")
        .next()
        .expect("edit")
        .click();
    harness.run();
    harness
        .get_all_by_role(egui::Role::MultilineTextInput)
        .nth(1)
        .expect("the entry's text")
        .focus();
    harness.run();
    harness
        .get_all_by_role(egui::Role::MultilineTextInput)
        .nth(1)
        .expect("the entry's text")
        .type_text(", confirmed");
    harness.run();
    harness.get_by_label("Save").click();
    harness.run();
    let edited = &harness.state().journal.entries()[1];
    assert_eq!(edited.text, "Theory: TLV length trusted, confirmed");
    assert_eq!(edited.updated, NOW);
    harness.get_by_label("2023-11-14 22:14 UTC (edited)");
}

#[test]
fn clicking_a_place_goes_there() {
    let mut harness = harness(sample(), Ok(SAVED));
    harness
        .get_by_label("net/parse.c:42 · parse_header")
        .click();
    harness.run();
    let expected = JournalAction::GoTo {
        file: "net/parse.c".into(),
        line: 42,
        symbol: Some("parse_header".into()),
    };
    assert_eq!(harness.state().action, Some(expected));
}

#[test]
fn a_read_only_journal_offers_no_changes() {
    let harness = harness(sample(), Err("Read-only: bad file"));
    harness.get_by_label("Read-only: bad file");
    assert!(harness.query_by_label("Edit").is_none());
    assert!(harness.query_by_label("Delete…").is_none());
}

#[test]
fn search_matches_text_file_or_symbol_newest_first() {
    let journal = sample();
    let entries = journal.entries();
    assert_eq!(shown_entries(entries, ""), [1, 0]);
    assert_eq!(shown_entries(entries, " tlv "), [1]);
    assert_eq!(shown_entries(entries, "PARSE.C"), [0]);
    assert_eq!(shown_entries(entries, "parse_header"), [0]);
    assert!(shown_entries(entries, "nowhere").is_empty());
}

#[test]
fn only_the_entries_in_view_are_drawn() {
    let journal = (0..200).fold(Journal::default(), |journal, i| {
        let text = format!("Entry {i:03}.\n\nSome more\nlines.");
        journal
            .with_added(Entry::new(&text, None, i))
            .expect("valid")
    });
    let harness = harness(journal, Ok(SAVED));
    assert!(
        harness.query_by_label("Entry 199.").is_some(),
        "newest, at the top"
    );
    assert!(harness.query_by_label("Entry 000.").is_none(), "far below");
}

#[test]
fn the_fingerprint_changes_with_every_change() {
    let journal = sample();
    let before = fingerprint(journal.entries());
    let edited = journal.with_text(1, "Theory: changed", NOW).expect("valid");
    assert_ne!(fingerprint(edited.entries()), before);
    assert_ne!(fingerprint(journal.without(0).entries()), before);
    assert_eq!(fingerprint(sample().entries()), before);
}

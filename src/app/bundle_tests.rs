//! Audit bundles: written and read back, checked, and merged without
//! overwriting.

use super::*;
use crate::app::session::Place;
use crate::notes::NoteTarget;

fn line(file: &str, line: u32) -> NoteTarget {
    NoteTarget::Line {
        file: file.into(),
        line,
    }
}

fn note(target: NoteTarget, text: &str, is_starred: bool) -> Note {
    Note {
        target,
        text: text.into(),
        is_starred,
        anchor: None,
        created: 1,
        updated: 1,
    }
}

fn verdict(rule: &str, line: u32, status: TriageStatus) -> TriageEntry {
    TriageEntry {
        rule: rule.into(),
        file: "a.c".into(),
        line,
        column: 4,
        anchor: "strcpy(d, s);".into(),
        status,
        note: String::new(),
        updated: 1,
    }
}

fn sample() -> Bundle {
    Bundle {
        project: "zlib".into(),
        exported: 1_700_000_000,
        session: Some(Session {
            place: Some(Place {
                file: "a.c".into(),
                line: 3,
            }),
            ..Session::default()
        }),
        notes: vec![
            note(line("a.c", 3), "length is unchecked", true),
            note(
                NoteTarget::Symbol("parse".into()),
                "entry from the network",
                false,
            ),
        ],
        todos: vec![Todo::new("fuzz parse()", Some(("a.c".into(), 3)))],
        verdicts: vec![
            verdict("strcpy", 9, TriageStatus::FalsePositive),
            verdict("memcpy", 12, TriageStatus::ConfirmedBug),
        ],
    }
}

#[test]
fn a_bundle_round_trips_and_names_the_project_by_folder_only() {
    let bundle = sample();
    let text = bundle.to_text().expect("written");
    assert!(text.contains("\"srcwranglr_audit\": 1"), "{text}");
    assert!(!text.contains("/Users"), "no paths from this machine");
    assert_eq!(Bundle::from_text(&text), Ok(bundle));
}

#[test]
fn a_bundle_from_elsewhere_is_checked_part_by_part() {
    let with = |field: &str, value: &str| {
        let mut json: serde_json::Value =
            serde_json::from_str(&sample().to_text().expect("text")).expect("json");
        json[field] = serde_json::from_str(value).expect("value");
        json.to_string()
    };
    for (text, says) in [
        ("not json".to_string(), "not a SrcWranglr audit bundle"),
        (with("srcwranglr_audit", "9"), "version 9"),
        (with("project", "\"\""), "project name"),
        (
            with("notes", r#"[{"file": "../../etc/passwd", "line": 1}]"#),
            "note 1",
        ),
        (with("notes", r#"[{"text": "where?"}]"#), "note 1"),
        (
            with(
                "notes",
                r#"[{"symbol": "f", "text": "a"}, {"symbol": "f", "text": "b"}]"#,
            ),
            "more than one note",
        ),
        (with("todos", r#"[{"text": ""}]"#), "TODO 1"),
        (
            with(
                "verdicts",
                r#"[{"rule": "r", "file": "/etc/passwd", "line": 1, "status": "reviewed"}]"#,
            ),
            "verdict 1",
        ),
        (
            with("session", r#"{"place": {"file": "a.c", "line": 0}}"#),
            "its session",
        ),
    ] {
        let error = Bundle::from_text(&text).expect_err(&text);
        assert!(error.contains(says), "{says}: {error}");
    }
}

#[test]
fn reading_refuses_a_huge_file_and_reads_a_saved_one() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("audit.srcwranglr-audit");
    std::fs::write(&path, sample().to_text().expect("text")).expect("write");
    assert_eq!(read(&path), Ok(sample()));
    let huge = dir.path().join("huge.srcwranglr-audit");
    let file = std::fs::File::create(&huge).expect("create");
    file.set_len(MAX_FILE_BYTES + 1).expect("grow");
    assert!(read(&huge).expect_err("too big").contains("at most"));
    assert!(read(dir.path()).is_err(), "a folder");
}

/// `bundle` merged into these, every part.
fn merged(bundle: &Bundle, notes: &Notes, todos: &Todos, triage: &Triage) -> Merged {
    merge(
        bundle,
        (notes, todos, triage),
        Parts::ALL,
        "— From them:",
        9,
    )
}

fn verdict_on(triage: &Triage, rule: &str, line: u32) -> Option<TriageStatus> {
    triage
        .entries()
        .iter()
        .find(|v| v.rule == rule && v.line == line)
        .map(|v| v.status)
}

#[test]
fn notes_are_added_appended_under_yours_starred_or_already_there() {
    let mine = Notes::try_from_notes(vec![
        note(line("a.c", 3), "I saw this too", false),
        note(
            NoteTarget::Symbol("parse".into()),
            "entry from the network",
            false,
        ),
        note(line("c.c", 1), "same words", true),
    ])
    .expect("notes");
    let bundle = Bundle {
        notes: vec![
            note(line("a.c", 3), "length is unchecked", true),
            note(
                NoteTarget::Symbol("parse".into()),
                "entry from the network",
                true,
            ),
            note(line("c.c", 1), "same words", false),
            note(line("b.c", 7), "new one", false),
        ],
        ..sample()
    };
    let none = (Todos::default(), Triage::default());
    let first = merged(&bundle, &mine, &none.0, &none.1);
    let counts = &first.counts;
    assert_eq!(
        (
            counts.notes_added,
            counts.notes_appended,
            counts.notes_starred,
            counts.notes_already
        ),
        (1, 1, 1, 1)
    );
    let appended = first.notes.get(&line("a.c", 3)).expect("kept");
    assert_eq!(
        appended.text,
        "I saw this too\n\n— From them:\nlength is unchecked"
    );
    assert!(appended.is_starred, "a star is added, never removed");
    let starred = first
        .notes
        .get(&NoteTarget::Symbol("parse".into()))
        .expect("kept");
    assert_eq!(starred.text, "entry from the network");
    assert!(starred.is_starred);
    assert!(first.notes.get(&line("c.c", 1)).expect("kept").is_starred);
    assert_eq!(
        first.notes.get(&line("b.c", 7)).expect("added").text,
        "new one"
    );
    let again = merged(&bundle, &first.notes, &none.0, &none.1);
    assert_eq!(
        again.counts.notes_already, 4,
        "importing twice changes nothing"
    );
    assert!(!again.counts.changes_notes());
    assert_eq!(again.notes, first.notes);
}

#[test]
fn a_short_note_inside_your_words_is_still_added() {
    let mine = Notes::try_from_notes(vec![note(line("a.c", 3), "debugging the parser", false)])
        .expect("notes");
    let bundle = Bundle {
        notes: vec![note(line("a.c", 3), "bug", false)],
        ..sample()
    };
    let result = merged(&bundle, &mine, &Todos::default(), &Triage::default());
    assert_eq!(result.counts.notes_appended, 1);
    assert!(result
        .notes
        .get(&line("a.c", 3))
        .expect("kept")
        .text
        .ends_with("\nbug"));
}

#[test]
fn a_note_too_long_to_append_is_skipped_and_said() {
    let long = "x".repeat(MAX_TEXT_BYTES - 10);
    let mine = Notes::try_from_notes(vec![note(line("a.c", 3), &long, false)]).expect("notes");
    let bundle = Bundle {
        notes: vec![note(line("a.c", 3), "and more text here", false)],
        ..sample()
    };
    let result = merged(&bundle, &mine, &Todos::default(), &Triage::default());
    assert_eq!(result.counts.notes_appended, 0);
    assert_eq!(result.counts.skipped.len(), 1);
    assert_eq!(result.notes, mine);
}

#[test]
fn todos_already_on_the_list_are_not_added_again() {
    let mine = Todos::default()
        .with_added(Todo::new("fuzz parse()", Some(("a.c".into(), 3))))
        .expect("todo");
    let bundle = Bundle {
        todos: vec![
            Todo::new("fuzz parse()", Some(("a.c".into(), 3))),
            Todo::new("fuzz parse()", None),
            Todo::new("fuzz parse()", None),
        ],
        ..sample()
    };
    let result = merged(&bundle, &Notes::default(), &mine, &Triage::default());
    assert_eq!(
        (result.counts.todos_added, result.counts.todos_already),
        (1, 2)
    );
    assert_eq!(result.todos.items().len(), 2);
}

#[test]
fn your_verdicts_are_kept_and_each_kind_is_merged_only_when_asked() {
    let mut mine = Triage::default();
    mine.set(verdict("strcpy", 9, TriageStatus::ConfirmedBug))
        .expect("set");
    let all = merged(&sample(), &Notes::default(), &Todos::default(), &mine);
    let counts = &all.counts;
    assert_eq!(
        (
            counts.bugs_added,
            counts.dismissing_added,
            counts.verdicts_kept
        ),
        (1, 0, 1)
    );
    assert_eq!(
        verdict_on(&all.triage, "strcpy", 9),
        Some(TriageStatus::ConfirmedBug),
        "yours kept"
    );
    let fresh = Triage::default();
    let bugs_only = Parts {
        dismissing_verdicts: false,
        ..Parts::ALL
    };
    let result = merge(
        &sample(),
        (&Notes::default(), &Todos::default(), &fresh),
        bugs_only,
        "h",
        9,
    );
    assert_eq!(
        (result.counts.bugs_added, result.counts.dismissing_added),
        (1, 0)
    );
    assert_eq!(verdict_on(&result.triage, "strcpy", 9), None);
    assert_eq!(
        verdict_on(&result.triage, "memcpy", 12),
        Some(TriageStatus::ConfirmedBug)
    );
    let everything = merged(&sample(), &Notes::default(), &Todos::default(), &fresh);
    assert_eq!(everything.counts.dismissing_added, 1);
}

#[test]
fn merging_many_is_quick() {
    let many: Vec<TriageEntry> = (1..=20_000)
        .map(|line| verdict("strcpy", line, TriageStatus::Reviewed))
        .collect();
    let mine = Triage::default()
        .with_all_added(many[..10_000].to_vec())
        .expect("mine");
    let bundle = Bundle {
        verdicts: many,
        ..sample()
    };
    let started = std::time::Instant::now();
    let result = merged(&bundle, &Notes::default(), &Todos::default(), &mine);
    assert_eq!(
        (result.counts.dismissing_added, result.counts.verdicts_kept),
        (10_000, 10_000)
    );
    assert!(started.elapsed().as_secs() < 2, "{:?}", started.elapsed());
}

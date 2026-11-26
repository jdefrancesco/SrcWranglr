//! The audit report: a human-readable Markdown summary of the open
//! project's audit — the verdicts (confirmed bugs first), notes and
//! bookmarks, open TODOs, and the journal. Unlike the audit bundle (a
//! machine-readable file for re-importing into another checkout), this is a
//! document to read and hand off. File › Generate Report… writes it where
//! you pick; `markdown` here builds the text.

use super::visible::{visible, visible_lines};
use crate::notes::journal::{date_time, Entry};
use crate::notes::todo::Todo;
use crate::notes::triage::{TriageEntry, TriageStatus};
use crate::notes::{Note, NoteTarget};

/// Everything a report is built from, borrowed from the app.
pub struct Report<'a> {
    pub project: &'a str,
    /// Unix seconds the report was generated at.
    pub generated: u64,
    /// How many findings the project has in all (for context).
    pub findings_total: usize,
    pub notes: Vec<&'a Note>,
    pub todos: &'a [Todo],
    pub verdicts: &'a [TriageEntry],
    pub journal: &'a [Entry],
}

impl Report<'_> {
    /// The report as Markdown, ready to save. Empty sections are left out.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        self.heading(&mut out);
        self.summary(&mut out);
        self.verdicts_section(&mut out, "Confirmed bugs", TriageStatus::ConfirmedBug);
        self.verdicts_section(&mut out, "Reviewed", TriageStatus::Reviewed);
        self.verdicts_section(
            &mut out,
            "Marked false positive",
            TriageStatus::FalsePositive,
        );
        self.notes_section(&mut out);
        self.todos_section(&mut out);
        self.journal_section(&mut out);
        out
    }

    fn heading(&self, out: &mut String) {
        out.push_str(&format!("# Audit report — {}\n\n", visible(self.project)));
        out.push_str(&format!("Generated {} UTC.\n\n", date_time(self.generated)));
    }

    fn summary(&self, out: &mut String) {
        let (bugs, reviewed, false_positives) = self.verdict_counts();
        let bookmarks = self.notes.iter().filter(|n| n.is_starred).count();
        let open_todos = self.todos.iter().filter(|t| !t.is_done).count();
        out.push_str("## Summary\n\n");
        let mut line = |label: &str, n: usize| out.push_str(&format!("- {label}: {n}\n"));
        line("Findings in all", self.findings_total);
        line("Confirmed bugs", bugs);
        line("Reviewed", reviewed);
        line("Marked false positive", false_positives);
        line("Notes", self.notes.len());
        line("Bookmarks", bookmarks);
        line("Open TODOs", open_todos);
        line("Journal entries", self.journal.len());
        out.push('\n');
    }

    /// Confirmed bugs, reviewed, or false positives, one `### ` section.
    fn verdicts_section(&self, out: &mut String, title: &str, status: TriageStatus) {
        let picked: Vec<&TriageEntry> = self
            .verdicts
            .iter()
            .filter(|v| v.status == status)
            .collect();
        if picked.is_empty() {
            return;
        }
        out.push_str(&format!("## {title} ({})\n\n", picked.len()));
        for verdict in picked {
            out.push_str(&format!(
                "- **{}** — `{}:{}`\n",
                visible(&verdict.rule),
                visible(&verdict.file),
                verdict.line
            ));
            if !verdict.anchor.trim().is_empty() {
                out.push_str(&format!("  - `{}`\n", visible(verdict.anchor.trim())));
            }
            for note_line in note_lines(&verdict.note) {
                out.push_str(&format!("  - {note_line}\n"));
            }
        }
        out.push('\n');
    }

    fn notes_section(&self, out: &mut String) {
        if self.notes.is_empty() {
            return;
        }
        out.push_str(&format!("## Notes & bookmarks ({})\n\n", self.notes.len()));
        for note in &self.notes {
            let star = if note.is_starred { "⭐ " } else { "" };
            out.push_str(&format!("- {star}**{}**\n", note_place(&note.target)));
            for note_line in note_lines(&note.text) {
                out.push_str(&format!("  - {note_line}\n"));
            }
        }
        out.push('\n');
    }

    fn todos_section(&self, out: &mut String) {
        if self.todos.is_empty() {
            return;
        }
        out.push_str("## TODO\n\n");
        for todo in self.todos {
            let box_ = if todo.is_done { "[x]" } else { "[ ]" };
            let place = match (&todo.file, todo.line) {
                (Some(file), Some(line)) => format!(" (`{}:{}`)", visible(file), line),
                (Some(file), None) => format!(" (`{}`)", visible(file)),
                _ => String::new(),
            };
            out.push_str(&format!("- {box_} {}{place}\n", visible(todo.text.trim())));
        }
        out.push('\n');
    }

    fn journal_section(&self, out: &mut String) {
        if self.journal.is_empty() {
            return;
        }
        out.push_str(&format!("## Journal ({})\n\n", self.journal.len()));
        // Newest first, as the Journal tab shows them.
        for entry in self.journal.iter().rev() {
            let edited = if entry.updated == entry.created {
                ""
            } else {
                " (edited)"
            };
            let place = match entry.place() {
                Some((file, line)) => format!(" — `{}:{}`", visible(file), line),
                None => String::new(),
            };
            out.push_str(&format!(
                "### {} UTC{edited}{place}\n\n",
                date_time(entry.created)
            ));
            out.push_str(&visible_lines(entry.text.trim()));
            out.push_str("\n\n");
        }
    }

    /// (confirmed bugs, reviewed, false positives).
    fn verdict_counts(&self) -> (usize, usize, usize) {
        let count = |status| self.verdicts.iter().filter(|v| v.status == status).count();
        (
            count(TriageStatus::ConfirmedBug),
            count(TriageStatus::Reviewed),
            count(TriageStatus::FalsePositive),
        )
    }
}

/// A finding's or TODO's place, as inline code.
fn note_place(target: &NoteTarget) -> String {
    match target {
        NoteTarget::Symbol(symbol) => format!("`{}`", visible(symbol)),
        NoteTarget::Line { file, line } => format!("`{}:{}`", visible(file), line),
    }
}

/// A note or verdict note split into shown lines (blank ones dropped), so a
/// multi-line note stays readable as sub-bullets.
fn note_lines(text: &str) -> Vec<String> {
    visible_lines(text.trim())
        .lines()
        .map(|l| l.trim_end().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// A short title for the saved file, e.g. `proj-audit-report.md`.
pub fn file_name(project: &str) -> String {
    let slug: String = project
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "project" } else { slug };
    format!("{slug}-audit-report.md")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::journal::Entry;

    fn bug(rule: &str, file: &str, line: u32, note: &str) -> TriageEntry {
        TriageEntry {
            rule: rule.into(),
            file: file.into(),
            line,
            column: 0,
            anchor: "strcpy(d, s);".into(),
            status: TriageStatus::ConfirmedBug,
            note: note.into(),
            updated: 1,
        }
    }

    fn note(target: NoteTarget, text: &str, starred: bool) -> Note {
        Note {
            target,
            text: text.into(),
            is_starred: starred,
            anchor: None,
            created: 1,
            updated: 1,
        }
    }

    #[test]
    fn a_full_report_has_every_section_bugs_first() {
        let verdicts = vec![
            bug("strcpy", "net/parse.c", 42, "attacker controls len"),
            TriageEntry {
                status: TriageStatus::FalsePositive,
                ..bug("sprintf", "ui/x.c", 9, "bounded by a constant")
            },
        ];
        let notes = [note(
            NoteTarget::Line {
                file: "net/parse.c".into(),
                line: 42,
            },
            "length unchecked",
            true,
        )];
        let todos = [Todo::new(
            "fuzz parse_header",
            Some(("net/parse.c".into(), 42)),
        )];
        let journal = [Entry::new("Looked at the parser.", None, 1_700_000_000)];
        let note_refs: Vec<&Note> = notes.iter().collect();
        let report = Report {
            project: "demo",
            generated: 1_700_000_600,
            findings_total: 7,
            notes: note_refs,
            todos: &todos,
            verdicts: &verdicts,
            journal: &journal,
        };
        let md = report.markdown();
        assert!(md.starts_with("# Audit report — demo\n"), "{md}");
        // Confirmed bugs come before the false-positive section.
        let bugs_at = md.find("## Confirmed bugs (1)").expect("bugs section");
        let fp_at = md.find("## Marked false positive (1)").expect("fp section");
        assert!(bugs_at < fp_at, "bugs first:\n{md}");
        assert!(md.contains("**strcpy** — `net/parse.c:42`"), "{md}");
        assert!(md.contains("attacker controls len"), "{md}");
        assert!(md.contains("## Notes & bookmarks (1)"), "{md}");
        assert!(md.contains("⭐ **`net/parse.c:42`**"), "{md}");
        assert!(
            md.contains("- [ ] fuzz parse_header (`net/parse.c:42`)"),
            "{md}"
        );
        assert!(md.contains("## Journal (1)"), "{md}");
        assert!(md.contains("Looked at the parser."), "{md}");
    }

    #[test]
    fn an_empty_audit_is_just_a_heading_and_summary() {
        let report = Report {
            project: "demo",
            generated: 1_700_000_600,
            findings_total: 0,
            notes: Vec::new(),
            todos: &[],
            verdicts: &[],
            journal: &[],
        };
        let md = report.markdown();
        assert!(md.contains("## Summary"), "{md}");
        assert!(!md.contains("## Confirmed bugs"), "{md}");
        assert!(!md.contains("## Journal"), "{md}");
        assert!(md.contains("- Findings in all: 0"), "{md}");
    }

    #[test]
    fn hostile_text_is_shown_safely() {
        // A right-to-left override in a file name can't flip the report.
        let verdicts = [bug("strcpy", "net/\u{202e}cexe.c", 1, "x")];
        let report = Report {
            project: "demo",
            generated: 1,
            findings_total: 1,
            notes: Vec::new(),
            todos: &[],
            verdicts: &verdicts,
            journal: &[],
        };
        let md = report.markdown();
        assert!(!md.contains('\u{202e}'), "the override is marked, not raw");
        assert!(md.contains("U+202E"), "{md}");
    }

    #[test]
    fn the_file_name_is_a_safe_slug() {
        assert_eq!(file_name("my proj!"), "my-proj-audit-report.md");
        assert_eq!(file_name("///"), "project-audit-report.md");
    }
}

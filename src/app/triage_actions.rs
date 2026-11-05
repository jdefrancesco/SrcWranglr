//! Your verdicts on findings: recording them (and saving), and keeping
//! them attached to the findings as those change.

use super::findings::line_text;
use super::{SrcWranglrApp, Status};
use crate::analysis::Finding;
use crate::notes::triage::{self, TriageEntry, TriageStatus};
use crate::notes::NoteTarget;

impl SrcWranglrApp {
    /// Records (or, with `None`, clears) your verdict on `finding` and saves
    /// it; nothing changes if it can't be saved. A confirmed bug is also
    /// added to the line's note, starred.
    pub(super) fn apply_verdict(
        &mut self,
        finding: &Finding,
        status: Option<TriageStatus>,
        note: String,
    ) {
        if let Some(error) = &self.triage_error {
            self.status = Status::Error(format!("Verdicts are read-only: {error}"));
            return;
        }
        let column = finding.columns.map_or(0, |(start, _)| start);
        let before = self.triage.clone();
        let changed = match status {
            Some(status) => self.triage.set(TriageEntry {
                rule: finding.rule.clone(),
                file: finding.file.clone(),
                line: finding.line,
                column,
                anchor: storable_anchor(line_text(
                    &self.project.graph,
                    &finding.file,
                    finding.line,
                )),
                status,
                note: note.clone(),
                updated: now(),
            }),
            None => {
                self.triage
                    .clear(&finding.rule, &finding.file, finding.line, column);
                Ok(())
            }
        };
        let saved = changed.and_then(|()| match &self.project.root {
            Some(root) => triage::save(root, &self.triage),
            None => Ok(()),
        });
        if let Err(error) = saved {
            self.triage = before;
            self.status = Status::Error(format!("Verdict not saved: {error}"));
            return;
        }
        self.trust_project_files_we_wrote();
        self.refresh_verdicts();
        if status == Some(TriageStatus::ConfirmedBug) {
            self.note_confirmed_bug(finding, &note);
            return;
        }
        let applied = if self.are_project_files_trusted {
            ""
        } else {
            " (Not applied until you trust this project's files.)"
        };
        self.status = Status::Info(match status {
            Some(status) => format!(
                "Marked {} as {}.{applied}",
                finding.rule,
                status.label().to_lowercase()
            ),
            None => format!("Cleared the verdict on {}.", finding.rule),
        });
    }

    /// Adds "Confirmed bug: …" to the line's note (keeping what's there)
    /// and stars it.
    fn note_confirmed_bug(&mut self, finding: &Finding, note: &str) {
        let target = NoteTarget::Line {
            file: finding.file.clone(),
            line: finding.line,
        };
        let bug = match note {
            "" => format!("Confirmed bug: {}", finding.rule),
            note => format!("Confirmed bug: {}: {note}", finding.rule),
        };
        let existing = self
            .notebook
            .notes()
            .get(&target)
            .map(|n| n.text.clone())
            .unwrap_or_default();
        let text = match existing.as_str() {
            "" => bug,
            same if same.contains(&bug) => existing.clone(),
            other => format!("{other}\n{bug}"),
        };
        self.status = match self.notebook.set(&self.project, target, &text, true) {
            Ok(()) => Status::Info(format!(
                "Confirmed bug saved, and bookmarked in Notes ({}:{}).",
                finding.file, finding.line
            )),
            Err(error) => Status::Error(format!("Verdict saved, but the note wasn't: {error}")),
        };
    }

    /// Re-attaches your verdicts to the current findings.
    pub(super) fn refresh_verdicts(&mut self) {
        let apply = self.are_project_files_trusted;
        self.findings = std::mem::take(&mut self.findings).with_triage(
            &self.triage,
            &self.project.graph,
            apply,
        );
    }
}

/// A line's trimmed text, if short enough to keep as a verdict's anchor.
fn storable_anchor(line: &str) -> String {
    let text = line.trim();
    match crate::notes::store::check_text(text, "anchor") {
        Ok(()) => text.to_string(),
        Err(_) => String::new(),
    }
}

/// Unix seconds now (0 if the clock is before 1970).
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use crate::analysis::dangerous::DangerousList;
    use crate::app::findings::Findings;
    use crate::app::launch::LaunchOptions;
    use crate::app::SrcWranglrApp;
    use crate::index::{build_graph, Extractor, SourceFile};
    use crate::notes::triage::TriageStatus;
    use crate::notes::NoteTarget;

    /// The demo app, showing a one-file project with a `strcpy` on line 2.
    fn app_with_strcpy() -> SrcWranglrApp {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, LaunchOptions::default());
        let source = "void f(char *d, char *s) {\nstrcpy(d, s);\n}\n";
        let facts = Extractor::new()
            .expect("grammar")
            .extract(source)
            .expect("parse");
        let graph = build_graph(vec![SourceFile {
            path: "a.c".into(),
            contents: source.into(),
            facts,
        }]);
        app.findings = Findings::for_graph(&graph, &DangerousList::default());
        app.project = crate::app::project::Project::new("t", None, graph);
        app.notebook = crate::app::notebook::Notebook::open(&app.project);
        app
    }

    #[test]
    fn verdicts_shipped_with_a_project_are_shown_but_not_applied() {
        let project = tempfile::tempdir().expect("tmp");
        let dir = project.path().join(crate::notes::store::NOTES_DIR);
        std::fs::create_dir(&dir).expect("mkdir");
        std::fs::write(
            dir.join(crate::notes::triage::TRIAGE_FILE),
            r#"{"version": 1, "entries": [{"rule": "strcpy", "file": "a.c", "line": 2,
                "column": 0, "anchor": "strcpy(d, s);", "status": "false_positive"}]}"#,
        )
        .expect("write");
        let source = "void f(char *d, char *s) {\nstrcpy(d, s);\n}\n";
        let facts = Extractor::new()
            .expect("grammar")
            .extract(source)
            .expect("parse");
        let graph = build_graph(vec![SourceFile {
            path: "a.c".into(),
            contents: source.into(),
            facts,
        }]);
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, LaunchOptions::default());
        let outcome = crate::index::IndexOutcome {
            graph,
            indexed_files: 1,
            warnings: Vec::new(),
        };
        app.load_project(project.path().to_path_buf(), outcome, &ctx);
        if crate::app::trust::is_trusted(project.path()) {
            return; // can't happen for a fresh temp folder
        }
        assert!(!app.are_project_files_trusted);
        assert_eq!(app.findings.verdict(0), Some(TriageStatus::FalsePositive));
        assert_eq!(
            app.findings.snippets_in("a.c").count(),
            1,
            "still highlighted"
        );
        assert!(matches!(&app.status, crate::app::Status::Info(t) if t.contains("not applied")));
    }

    #[test]
    fn a_false_positive_stops_being_highlighted() {
        let mut app = app_with_strcpy();
        let finding = app.findings.all()[0].clone();
        app.apply_verdict(
            &finding,
            Some(TriageStatus::FalsePositive),
            "dest is 4K".into(),
        );
        assert_eq!(app.findings.verdict(0), Some(TriageStatus::FalsePositive));
        assert_eq!(app.findings.snippets_in("a.c").count(), 0);
        assert_eq!(
            app.findings.verdict_and_note(0),
            Some((TriageStatus::FalsePositive, "dest is 4K".into()))
        );
        app.apply_verdict(&finding, None, String::new());
        assert_eq!(app.findings.snippets_in("a.c").count(), 1);
    }

    #[test]
    fn a_confirmed_bug_keeps_the_lines_existing_note() {
        let mut app = app_with_strcpy();
        let line = NoteTarget::Line {
            file: "a.c".into(),
            line: 2,
        };
        app.notebook
            .set(&app.project, line.clone(), "my earlier thought", false)
            .expect("note");
        let finding = app.findings.all()[0].clone();
        app.apply_verdict(&finding, Some(TriageStatus::ConfirmedBug), String::new());
        let note = app.notebook.notes().get(&line).expect("note");
        assert_eq!(note.text, "my earlier thought\nConfirmed bug: strcpy");
        assert!(note.is_starred);
    }

    #[test]
    fn a_confirmed_bug_becomes_a_bookmarked_note() {
        let mut app = app_with_strcpy();
        let finding = app.findings.all()[0].clone();
        app.apply_verdict(
            &finding,
            Some(TriageStatus::ConfirmedBug),
            "len from packet".into(),
        );
        let note = app.notebook.notes().get(&NoteTarget::Line {
            file: "a.c".into(),
            line: 2,
        });
        assert!(note
            .is_some_and(|n| n.is_starred && n.text == "Confirmed bug: strcpy: len from packet"));
        // Still highlighted: it's a real bug.
        assert_eq!(app.findings.snippets_in("a.c").count(), 1);
    }
}

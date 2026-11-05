//! Your verdicts on findings: reviewed, false positive, or confirmed bug,
//! with a note. Kept in `.slurcetrail/triage.json`, keyed by the rule, the
//! file, and the flagged line's text, so a verdict follows its line when
//! code above it changes.

use std::path::Path;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::store::{check_target, check_text, NOTES_DIR};
use super::NoteTarget;
use crate::safe_fs::{read_regular_file, write_atomically};

pub const TRIAGE_FILE: &str = "triage.json";
const FORMAT_VERSION: u32 = 1;
const MAX_ENTRIES: usize = 50_000;
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageStatus {
    Reviewed,
    FalsePositive,
    ConfirmedBug,
}

impl TriageStatus {
    pub const ALL: [TriageStatus; 3] = [
        TriageStatus::Reviewed,
        TriageStatus::FalsePositive,
        TriageStatus::ConfirmedBug,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TriageStatus::Reviewed => "Reviewed",
            TriageStatus::FalsePositive => "False positive",
            TriageStatus::ConfirmedBug => "Confirmed bug",
        }
    }

    /// Reviewed and false positives stop being highlighted.
    pub fn is_dismissed(self) -> bool {
        matches!(self, TriageStatus::Reviewed | TriageStatus::FalsePositive)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageEntry {
    pub rule: String,
    pub file: String,
    pub line: u32,
    /// Byte column where the flagged code starts on its line, which tells
    /// apart two findings of one rule on one line.
    #[serde(default)]
    pub column: u32,
    /// The flagged line's trimmed text, to find it after it moves.
    #[serde(default)]
    pub anchor: String,
    pub status: TriageStatus,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub updated: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Triage {
    entries: Vec<TriageEntry>,
}

/// What identifies a finding for matching verdicts to it.
#[derive(Debug, Clone, Copy)]
pub struct FindingKey<'a> {
    pub rule: &'a str,
    pub file: &'a str,
    pub line: u32,
    pub column: u32,
    /// The finding's line as it reads now.
    pub text: &'a str,
}

#[derive(Serialize, Deserialize)]
struct TriageFile {
    version: u32,
    entries: Vec<TriageEntry>,
}

impl Triage {
    /// The verdict for each of `findings`, matched one to one: first one
    /// made on that very spot whose line still reads the same, then (for
    /// code that moved) one whose recorded line no longer has its text,
    /// at the nearest line that does. `line_text(file, line)` reads the
    /// current source.
    pub fn match_findings<'a>(
        &self,
        findings: &[FindingKey<'_>],
        line_text: impl Fn(&str, u32) -> &'a str,
    ) -> Vec<Option<&TriageEntry>> {
        let mut by_finding: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for (i, e) in self.entries.iter().enumerate() {
            by_finding
                .entry((e.rule.clone(), e.file.clone()))
                .or_default()
                .push(i);
        }
        let candidates =
            |key: &FindingKey| by_finding.get(&(key.rule.to_string(), key.file.to_string()));
        let mut used = vec![false; self.entries.len()];
        let mut matched: Vec<Option<usize>> = vec![None; findings.len()];
        for (slot, key) in matched.iter_mut().zip(findings) {
            let exact = candidates(key).into_iter().flatten().copied().find(|&i| {
                let e = &self.entries[i];
                !used[i]
                    && (e.line, e.column) == (key.line, key.column)
                    && (e.anchor.is_empty() || e.anchor == key.text.trim())
            });
            if let Some(i) = exact {
                used[i] = true;
                *slot = Some(i);
            }
        }
        for (slot, key) in matched.iter_mut().zip(findings) {
            if slot.is_some() || key.text.trim().is_empty() {
                continue;
            }
            let moved = candidates(key)
                .into_iter()
                .flatten()
                .copied()
                .filter(|&i| {
                    let e = &self.entries[i];
                    !used[i]
                        && e.anchor == key.text.trim()
                        && line_text(&e.file, e.line).trim() != e.anchor
                })
                .min_by_key(|&i| self.entries[i].line.abs_diff(key.line));
            if let Some(i) = moved {
                used[i] = true;
                *slot = Some(i);
            }
        }
        matched
            .into_iter()
            .map(|m| m.map(|i| &self.entries[i]))
            .collect()
    }

    /// Records `entry`, replacing any verdict on the same spot, if the
    /// verdicts file can hold it.
    pub fn set(&mut self, entry: TriageEntry) -> Result<(), String> {
        check_entry(&entry)?;
        self.clear(&entry.rule, &entry.file, entry.line, entry.column);
        self.entries.push(entry);
        Ok(())
    }

    /// Forgets the verdict on `rule` at `file:line`, column `column`.
    pub fn clear(&mut self, rule: &str, file: &str, line: u32, column: u32) {
        self.entries.retain(|e| {
            !(e.rule == rule && e.file == file && e.line == line && e.column == column)
        });
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The project's verdicts (none if it has no triage file).
pub fn load(root: &Path) -> Result<Triage, String> {
    let path = root.join(NOTES_DIR).join(TRIAGE_FILE);
    let invalid = |message: String| format!("{}: {message}", path.display());
    let Some(json) =
        read_regular_file(&path, MAX_FILE_BYTES).map_err(|e| invalid(e.to_string()))?
    else {
        return Ok(Triage::default());
    };
    let file: TriageFile = serde_json::from_str(&json).map_err(|e| invalid(e.to_string()))?;
    if file.version != FORMAT_VERSION {
        return Err(invalid(format!("unsupported version {}", file.version)));
    }
    if file.entries.len() > MAX_ENTRIES {
        return Err(invalid(format!("more than {MAX_ENTRIES} verdicts")));
    }
    for (i, entry) in file.entries.iter().enumerate() {
        check_entry(entry).map_err(|e| invalid(format!("verdict {}: {e}", i + 1)))?;
    }
    Ok(Triage {
        entries: file.entries,
    })
}

/// Saves the verdicts atomically; writes nothing for none when there is no
/// file yet.
pub fn save(root: &Path, triage: &Triage) -> Result<(), String> {
    let dir = root.join(NOTES_DIR);
    if triage.entries.is_empty() && !dir.join(TRIAGE_FILE).exists() {
        return Ok(());
    }
    for entry in &triage.entries {
        check_entry(entry)?;
    }
    let file = TriageFile {
        version: FORMAT_VERSION,
        entries: triage.entries.clone(),
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    if file.entries.len() > MAX_ENTRIES || json.len() >= MAX_FILE_BYTES {
        return Err("Too many verdicts to save; clear some first".into());
    }
    write_atomically(&dir, TRIAGE_FILE, &format!("{json}\n"))
        .map_err(|e| format!("Could not save {}: {e}", dir.join(TRIAGE_FILE).display()))
}

/// What load accepts (and so save must keep to).
pub fn check_entry(entry: &TriageEntry) -> Result<(), String> {
    check_target(&NoteTarget::Symbol(entry.rule.clone()))?;
    check_target(&NoteTarget::Line {
        file: entry.file.clone(),
        line: entry.line,
    })?;
    check_text(&entry.anchor, "anchor")?;
    check_text(&entry.note, "note")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(rule: &str, line: u32, anchor: &str, status: TriageStatus) -> TriageEntry {
        TriageEntry {
            rule: rule.into(),
            file: "a.c".into(),
            line,
            column: 2,
            anchor: anchor.into(),
            status,
            note: String::new(),
            updated: 1,
        }
    }

    fn key<'a>(rule: &'a str, line: u32, column: u32, text: &'a str) -> FindingKey<'a> {
        FindingKey {
            rule,
            file: "a.c",
            line,
            column,
            text,
        }
    }

    /// The source, one entry per line (1-based).
    fn source<'a>(lines: &'a [&'a str]) -> impl Fn(&str, u32) -> &'a str {
        move |_, line| lines.get(line as usize - 1).copied().unwrap_or("")
    }

    fn statuses(triage: &Triage, keys: &[FindingKey], lines: &[&str]) -> Vec<Option<TriageStatus>> {
        triage
            .match_findings(keys, source(lines))
            .into_iter()
            .map(|e| e.map(|e| e.status))
            .collect()
    }

    #[test]
    fn a_verdict_stays_on_its_spot_and_follows_moved_code() {
        let mut triage = Triage::default();
        triage
            .set(entry(
                "strcpy",
                2,
                "strcpy(d, s);",
                TriageStatus::FalsePositive,
            ))
            .expect("set");
        let same = ["x", "  strcpy(d, s);"];
        let key_now = [key("strcpy", 2, 2, "  strcpy(d, s);")];
        assert_eq!(
            statuses(&triage, &key_now, &same),
            [Some(TriageStatus::FalsePositive)]
        );
        // Two lines were added above: it follows its text.
        let moved = ["// a", "// b", "x", "  strcpy(d, s);"];
        let key_moved = [key("strcpy", 4, 2, "  strcpy(d, s);")];
        assert_eq!(
            statuses(&triage, &key_moved, &moved),
            [Some(TriageStatus::FalsePositive)]
        );
    }

    #[test]
    fn changed_code_on_the_same_line_does_not_inherit_the_verdict() {
        let mut triage = Triage::default();
        triage
            .set(entry(
                "strcpy",
                2,
                "strcpy(d, s);",
                TriageStatus::FalsePositive,
            ))
            .expect("set");
        let changed = ["x", "  strcpy(d, user_input);"];
        let keys = [key("strcpy", 2, 2, "  strcpy(d, user_input);")];
        assert_eq!(statuses(&triage, &keys, &changed), [None]);
    }

    #[test]
    fn a_verdict_is_not_shared_with_identical_lines_elsewhere() {
        let mut triage = Triage::default();
        triage
            .set(entry(
                "strcpy",
                1,
                "strcpy(b, s);",
                TriageStatus::FalsePositive,
            ))
            .expect("set");
        let lines = ["  strcpy(b, s);", "x", "  strcpy(b, s);"];
        let keys = [
            key("strcpy", 1, 2, "  strcpy(b, s);"),
            key("strcpy", 3, 2, "  strcpy(b, s);"),
        ];
        assert_eq!(
            statuses(&triage, &keys, &lines),
            [Some(TriageStatus::FalsePositive), None]
        );
    }

    #[test]
    fn two_findings_on_one_line_have_their_own_verdicts() {
        let mut triage = Triage::default();
        let line = "strcpy(a, x); strcpy(b, y);";
        let first = TriageEntry {
            column: 0,
            ..entry("strcpy", 1, line, TriageStatus::Reviewed)
        };
        triage.set(first).expect("set");
        let keys = [key("strcpy", 1, 0, line), key("strcpy", 1, 14, line)];
        assert_eq!(
            statuses(&triage, &keys, &[line]),
            [Some(TriageStatus::Reviewed), None]
        );
    }

    #[test]
    fn set_replaces_clear_forgets_and_bad_entries_are_refused() {
        let mut triage = Triage::default();
        triage
            .set(entry("strcpy", 10, "x", TriageStatus::Reviewed))
            .expect("set");
        triage
            .set(entry("strcpy", 10, "x", TriageStatus::ConfirmedBug))
            .expect("set");
        assert_eq!(triage.len(), 1);
        triage.clear("strcpy", "a.c", 10, 2);
        assert_eq!(triage.len(), 0);
        let huge = "y".repeat(crate::notes::store::MAX_TEXT_BYTES + 1);
        let bad = TriageEntry {
            note: huge,
            ..entry("strcpy", 1, "x", TriageStatus::Reviewed)
        };
        assert!(triage.set(bad).is_err());
        assert_eq!(triage.len(), 0);
    }

    #[test]
    fn saves_and_loads_and_writes_nothing_for_nothing() {
        let dir = tempfile::tempdir().expect("tmp");
        save(dir.path(), &Triage::default()).expect("save none");
        assert!(!dir.path().join(NOTES_DIR).exists());
        let mut triage = Triage::default();
        triage
            .set(entry(
                "strcpy",
                3,
                "strcpy(d, s);",
                TriageStatus::ConfirmedBug,
            ))
            .expect("set");
        save(dir.path(), &triage).expect("save");
        assert_eq!(load(dir.path()).expect("load"), triage);
    }

    #[test]
    fn rejects_bad_files() {
        for bad in [
            "nope",
            r#"{"version": 9, "entries": []}"#,
            r#"{"version": 1, "entries": [{"rule": "r", "file": "../x.c", "line": 1, "status": "reviewed"}]}"#,
            r#"{"version": 1, "entries": [{"rule": "r", "file": "a.c", "line": 0, "status": "reviewed"}]}"#,
            r#"{"version": 1, "entries": [{"rule": "r", "file": "a.c", "line": 1, "status": "maybe"}]}"#,
        ] {
            let dir = tempfile::tempdir().expect("tmp");
            let notes = dir.path().join(NOTES_DIR);
            std::fs::create_dir(&notes).expect("mkdir");
            std::fs::write(notes.join(TRIAGE_FILE), bad).expect("write");
            assert!(load(dir.path()).is_err(), "accepted {bad}");
        }
    }
}

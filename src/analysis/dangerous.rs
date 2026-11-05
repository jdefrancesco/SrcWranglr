//! Your list of dangerous or troublesome functions: flagged wherever they
//! are called, whether they're library functions or the project's own.
//!
//! Kept as TOML, in a global file used for every project and, optionally,
//! in the project (whose entries win):
//!
//! ```toml
//! [[function]]
//! name = "net::parse_packet"   # qualified, or a plain name for any scope
//! severity = "high"            # high | medium | low | off (silences it)
//! reason = "Trusts the length field from the wire"
//! fix = "Check len against the buffer first"   # optional
//! cwe = 20                                     # optional
//! ```

use std::collections::HashMap;

use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};

use super::Severity;

const TABLE: &str = "function";
/// Most entries a list may have (it's read on every project load).
const MAX_ENTRIES: usize = 10_000;
/// What a new list file starts with.
const NEW_FILE_HEADER: &str = "\
# Functions SrcWranglr flags wherever they are called.
# name: qualified (ns::f) or plain (any scope); severity: high | medium | low | off
";

/// One listed function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DangerousFunction {
    pub name: String,
    /// `None` means "off": never flag calls to it (silences a built-in).
    pub severity: Option<Severity>,
    pub reason: String,
    pub fix: Option<String>,
    pub cwe: Option<u32>,
    /// From a project list you haven't trusted: it may add a finding or
    /// raise its severity, never silence or lower one.
    pub is_advisory: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DangerousList {
    entries: Vec<DangerousFunction>,
    /// Entry index by name (the last entry of a name wins).
    by_name: HashMap<String, usize>,
}

impl DangerousList {
    /// The list in a TOML file's text.
    pub fn parse(text: &str) -> Result<Self, String> {
        let doc: DocumentMut = text.parse().map_err(|e| format!("not valid TOML: {e}"))?;
        let Some(item) = doc.get(TABLE) else {
            return Ok(Self::default());
        };
        let tables = item
            .as_array_of_tables()
            .ok_or("`function` must be a list of [[function]] entries")?;
        if tables.len() > MAX_ENTRIES {
            return Err(format!("more than {MAX_ENTRIES} entries"));
        }
        let entries = tables
            .iter()
            .enumerate()
            .map(|(i, table)| entry_of(table).map_err(|e| format!("entry {}: {e}", i + 1)))
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self::from_entries(entries))
    }

    fn from_entries(entries: Vec<DangerousFunction>) -> Self {
        let by_name = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.clone(), i))
            .collect();
        Self { entries, by_name }
    }

    /// This list as advice only: "off" entries dropped (returned as a
    /// count), the rest marked advisory.
    pub fn advisory(self) -> (Self, usize) {
        let total = self.entries.len();
        let kept: Vec<DangerousFunction> = self
            .entries
            .into_iter()
            .filter(|e| e.severity.is_some())
            .map(|e| DangerousFunction {
                is_advisory: true,
                ..e
            })
            .collect();
        let dropped = total - kept.len();
        (Self::from_entries(kept), dropped)
    }

    /// `global` with `project`'s entries added, replacing same-named ones.
    pub fn merged(global: &Self, project: &Self) -> Self {
        let kept = global
            .entries
            .iter()
            .filter(|g| !project.by_name.contains_key(&g.name));
        Self::from_entries(kept.chain(&project.entries).cloned().collect())
    }

    /// The entry for a call to `called` (a qualified name): an exact match,
    /// else a plain-named entry matching its last part. Later entries win.
    pub fn lookup(&self, called: &str) -> Option<&DangerousFunction> {
        let last = called.rsplit("::").next().unwrap_or(called);
        let exact = self.by_name.get(called);
        let plain = || self.by_name.get(last).filter(|_| !last.contains("::"));
        exact.or_else(plain).map(|i| &self.entries[*i])
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// `text` (a list file, possibly empty) with `entry` added, or replacing
/// the entry of the same name. Everything else, comments included, stays:
/// a new entry is appended after the existing text, untouched.
pub fn with_entry(text: &str, entry: &DangerousFunction) -> Result<String, String> {
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("not valid TOML: {e}"))?;
    let tables = match doc.get_mut(TABLE) {
        None => None,
        Some(item) => Some(
            item.as_array_of_tables_mut()
                .ok_or("`function` must be a list of [[function]] entries")?,
        ),
    };
    // The last entry of a name is the one in effect.
    let existing = tables.and_then(|tables| {
        let index = tables
            .iter()
            .enumerate()
            .filter(|(_, t)| t.get("name").and_then(Item::as_str) == Some(entry.name.as_str()))
            .map(|(i, _)| i)
            .last()?;
        tables.get_mut(index)
    });
    if let Some(table) = existing {
        fill(table, entry);
        return Ok(doc.to_string());
    }
    let mut table = Table::new();
    fill(&mut table, entry);
    let mut appended = DocumentMut::new();
    let mut tables = ArrayOfTables::new();
    tables.push(table);
    appended.insert(TABLE, Item::ArrayOfTables(tables));
    let before = match text.trim_end() {
        "" => NEW_FILE_HEADER.trim_end(),
        existing => existing,
    };
    Ok(format!("{before}\n\n{appended}"))
}

fn fill(table: &mut Table, entry: &DangerousFunction) {
    table["name"] = value(entry.name.as_str());
    table["severity"] = value(entry.severity.map_or("off", Severity::label));
    table["reason"] = value(entry.reason.as_str());
    match &entry.fix {
        Some(fix) => table["fix"] = value(fix.as_str()),
        None => {
            table.remove("fix");
        }
    }
    match entry.cwe {
        Some(cwe) => table["cwe"] = value(i64::from(cwe)),
        None => {
            table.remove("cwe");
        }
    }
}

fn entry_of(table: &Table) -> Result<DangerousFunction, String> {
    let text = |key: &str| table.get(key).and_then(Item::as_str).map(str::trim);
    let name = text("name")
        .filter(|n| !n.is_empty())
        .ok_or("needs a `name`")?;
    let severity = match text("severity").ok_or("needs a `severity`")? {
        "high" => Some(Severity::High),
        "medium" => Some(Severity::Medium),
        "low" => Some(Severity::Low),
        "off" => None,
        other => {
            return Err(format!(
                "unknown severity `{other}` (high, medium, low, off)"
            ))
        }
    };
    let reason = text("reason")
        .filter(|r| !r.is_empty())
        .ok_or("needs a `reason`")?;
    let cwe = match table.get("cwe") {
        None => None,
        Some(item) => Some(
            item.as_integer()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or("`cwe` must be a positive number")?,
        ),
    };
    Ok(DangerousFunction {
        name: name.to_string(),
        severity,
        reason: reason.to_string(),
        fix: text("fix").filter(|f| !f.is_empty()).map(String::from),
        cwe,
        is_advisory: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"
# Things this codebase gets wrong.
[[function]]
name = "net::parse_packet"
severity = "high"
reason = "Trusts the length field"
fix = "Check len first"
cwe = 20

[[function]]
name = "xfree"   # double frees seen before
severity = "medium"
reason = "Doesn't null the pointer"

[[function]]
name = "memcpy"
severity = "off"
reason = "Reviewed: all sizes are constants"
"#;

    fn entry(name: &str, severity: Option<Severity>, reason: &str) -> DangerousFunction {
        DangerousFunction {
            name: name.into(),
            severity,
            reason: reason.into(),
            fix: None,
            cwe: None,
            is_advisory: false,
        }
    }

    #[test]
    fn parses_entries_with_optional_fields() {
        let list = DangerousList::parse(LIST).expect("parse");
        let parse = list.lookup("net::parse_packet").expect("listed");
        assert_eq!(parse.severity, Some(Severity::High));
        assert_eq!(
            (parse.fix.as_deref(), parse.cwe),
            (Some("Check len first"), Some(20))
        );
        assert_eq!(list.lookup("memcpy").map(|e| e.severity), Some(None));
        assert!(DangerousList::parse("").expect("empty").is_empty());
    }

    #[test]
    fn rejects_bad_entries() {
        for bad in [
            "[[function]]\nseverity = \"high\"\nreason = \"no name\"\n",
            "[[function]]\nname = \"f\"\nseverity = \"urgent\"\nreason = \"r\"\n",
            "[[function]]\nname = \"f\"\nseverity = \"high\"\n",
            "[[function]]\nname = \"f\"\nseverity = \"high\"\nreason = \"r\"\ncwe = -1\n",
            "not toml at all = = =",
        ] {
            assert!(DangerousList::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn plain_names_match_any_scope_and_qualified_names_match_exactly() {
        let list = DangerousList::parse(LIST).expect("parse");
        assert!(list.lookup("xfree").is_some());
        assert!(list.lookup("mem::xfree").is_some());
        assert!(list.lookup("net::parse_packet").is_some());
        assert!(list.lookup("parse_packet").is_none());
        assert!(list.lookup("other::parse_packet").is_none());
    }

    #[test]
    fn project_entries_replace_global_ones() {
        let global = DangerousList::parse(LIST).expect("parse");
        let project = DangerousList::from_entries(vec![entry(
            "xfree",
            Some(Severity::High),
            "project says so",
        )]);
        let merged = DangerousList::merged(&global, &project);
        let xfree = merged.lookup("xfree").expect("listed");
        assert_eq!(
            (xfree.severity, xfree.reason.as_str()),
            (Some(Severity::High), "project says so")
        );
        assert!(merged.lookup("net::parse_packet").is_some());
    }

    #[test]
    fn adding_an_entry_keeps_the_rest_of_the_file() {
        let added = with_entry(LIST, &entry("strdup", Some(Severity::Low), "leaks")).expect("add");
        assert!(added.contains("# Things this codebase gets wrong."));
        assert!(added.contains("# double frees seen before"));
        let list = DangerousList::parse(&added).expect("parse");
        assert_eq!(
            list.lookup("strdup").map(|e| e.reason.as_str()),
            Some("leaks")
        );
        assert!(list.lookup("xfree").is_some());
    }

    #[test]
    fn adding_an_existing_name_replaces_it() {
        let updated = with_entry(LIST, &entry("xfree", Some(Severity::Low), "fixed upstream"))
            .expect("update");
        let list = DangerousList::parse(&updated).expect("parse");
        let xfree = list.lookup("xfree").expect("listed");
        assert_eq!(
            (xfree.severity, xfree.reason.as_str()),
            (Some(Severity::Low), "fixed upstream")
        );
        assert_eq!(updated.matches("name = \"xfree\"").count(), 1);
        let fresh = with_entry("", &entry("f", None, "silenced")).expect("new file");
        assert_eq!(
            DangerousList::parse(&fresh)
                .expect("parse")
                .lookup("f")
                .map(|e| e.severity),
            Some(None)
        );
    }
}

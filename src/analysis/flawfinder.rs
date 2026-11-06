//! Reading flawfinder's report (`--csv`) into findings. Columns are found by
//! their header names, so a flawfinder that adds or reorders them still
//! reads.

use std::path::Path;

use super::tools::{cwe_number, project_relative};
use super::{Finding, Severity};

pub const TOOL: &str = "flawfinder";
/// Records past this many aren't read (a report that size is noise).
const MAX_RECORDS: usize = 200_000;

/// The findings in flawfinder's CSV report, with paths relative to `root`.
pub fn parse(csv: &str, root: &Path) -> Result<Vec<Finding>, String> {
    let mut rows = records(csv).into_iter();
    let Some(header) = rows.next() else {
        return Err("flawfinder printed no report".into());
    };
    let column = |name: &str| header.iter().position(|h| h == name);
    let required = (
        column("File"),
        column("Line"),
        column("Name"),
        column("Warning"),
    );
    let (Some(file), Some(line), Some(name), Some(warning)) = required else {
        return Err("flawfinder's report has no File, Line, Name and Warning columns".into());
    };
    let columns = Columns {
        file,
        line,
        name,
        warning,
        column: column("Column"),
        level: column("Level"),
        suggestion: column("Suggestion"),
        cwes: column("CWEs"),
    };
    Ok(rows.filter_map(|row| columns.finding(&row, root)).collect())
}

/// Where each field is in a record.
struct Columns {
    file: usize,
    line: usize,
    name: usize,
    warning: usize,
    column: Option<usize>,
    level: Option<usize>,
    suggestion: Option<usize>,
    cwes: Option<usize>,
}

impl Columns {
    fn finding(&self, row: &[String], root: &Path) -> Option<Finding> {
        let field = |i: usize| row.get(i).map(String::as_str);
        let optional = |i: Option<usize>| i.and_then(field).filter(|f| !f.is_empty());
        let name = field(self.name).filter(|n| !n.is_empty())?;
        let file = field(self.file)?;
        let file = project_relative(file.strip_prefix("./").unwrap_or(file), root)?;
        let line = field(self.line)?.parse().ok().filter(|l| *l > 0)?;
        let level: u8 = optional(self.level)
            .and_then(|l| l.parse().ok())
            .unwrap_or(1);
        let start = optional(self.column)
            .and_then(|c| c.parse::<u32>().ok())
            .and_then(|c| c.checked_sub(1));
        let width = u32::try_from(name.len()).ok()?;
        Some(Finding {
            tool: TOOL.into(),
            rule: name.into(),
            severity: severity(level),
            message: field(self.warning)?.into(),
            fix: optional(self.suggestion).map(String::from),
            cwe: optional(self.cwes).and_then(cwe_number),
            file,
            line,
            columns: start.map(|s| (s, s.saturating_add(width))),
            symbol: None,
        })
    }
}

/// flawfinder's risk level, 0 (none) to 5 (highest).
fn severity(level: u8) -> Severity {
    match level {
        4.. => Severity::High,
        3 => Severity::Medium,
        _ => Severity::Low,
    }
}

/// The records of CSV text: fields split at commas, `"…"` quoting commas,
/// line breaks and doubled quotes (`""`).
fn records(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut is_quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, is_quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            ('"', _) => is_quoted = !is_quoted,
            (',', false) => row.push(std::mem::take(&mut field)),
            ('\n', false) => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                if rows.len() >= MAX_RECORDS {
                    return rows;
                }
            }
            ('\r', false) => {}
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What flawfinder 2.0.20 prints for `--csv --quiet --columns` on a
    /// project at `/src/proj`.
    const REPORT: &str = "File,Line,Column,DefaultLevel,Level,Category,Name,Warning,Suggestion,Note,CWEs,Context,Fingerprint,ToolVersion,RuleId,HelpUri\n\
/src/proj/a.c,5,3,4,4,buffer,strcpy,Does not check for buffer overflows when copying to destination [MS-banned] (CWE-120).,\"Consider using snprintf, strcpy_s, or strlcpy (warning: strncpy easily misused).\",,CWE-120,\"  strcpy(b, s);\",2c33,2.0.20,FF1001,https://cwe.mitre.org/data/definitions/120.html\n\
/src/proj/a.c,7,3,4,4,shell,system,This causes a new program to execute and is difficult to use safely (CWE-78).,try using a library call that implements the same functionality if available.,,CWE-78,\"  system(\"\"ls, \\\"\"x\\\"\"\"\");\",0f81,2.0.20,FF1044,https://cwe.mitre.org/data/definitions/78.html\n\
/src/proj/a.c,4,3,2,2,buffer,char,\"Statically-sized arrays can be improperly restricted (CWE-119!/CWE-120).\",\"Perform bounds checking.\",,CWE-119!/CWE-120,  char b[8];,55cc,2.0.20,FF1013,https://cwe.mitre.org/data/definitions/119.html\n";

    #[test]
    fn reads_flawfinders_report() {
        let findings = parse(REPORT, Path::new("/src/proj")).expect("parsed");
        assert_eq!(findings.len(), 3);
        let strcpy = &findings[0];
        assert_eq!(strcpy.tool, "flawfinder");
        assert_eq!(strcpy.rule, "strcpy");
        assert_eq!(strcpy.severity, Severity::High);
        assert_eq!((strcpy.file.as_str(), strcpy.line), ("a.c", 5));
        assert_eq!(strcpy.columns, Some((2, 8)));
        assert_eq!(strcpy.cwe, Some(120));
        assert_eq!(
            strcpy.fix.as_deref(),
            Some(
                "Consider using snprintf, strcpy_s, or strlcpy (warning: strncpy easily misused)."
            )
        );
        assert_eq!(findings[1].rule, "system");
        assert_eq!(findings[2].severity, Severity::Low);
        assert_eq!(findings[2].cwe, Some(119));
    }

    #[test]
    fn reads_quoted_fields_with_commas_quotes_and_line_breaks() {
        let rows = records("a,\"b, \"\"c\"\"\",\"d\ne\"\r\nf,,g\n");
        assert_eq!(rows, [vec!["a", "b, \"c\"", "d\ne"], vec!["f", "", "g"]]);
    }

    #[test]
    fn a_report_without_its_columns_is_an_error_and_outside_paths_are_dropped() {
        assert!(parse("", Path::new("/p")).is_err());
        assert!(parse("Name,Level\nstrcpy,4\n", Path::new("/p")).is_err());
        let outside = "File,Line,Name,Warning\n/elsewhere/a.c,3,gets,bad\n./b.c,2,gets,bad\n";
        let findings = parse(outside, Path::new("/p")).expect("parsed");
        assert_eq!(findings.len(), 1);
        assert_eq!(
            (findings[0].file.as_str(), findings[0].columns),
            ("b.c", None)
        );
    }
}

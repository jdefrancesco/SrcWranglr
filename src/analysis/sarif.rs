//! Reading SARIF 2.1 (what CodeQL, and many other analyzers, write) into
//! findings: each run's results, under its tool's name, with their
//! severity, CWE and place. SrcWranglr never builds or analyzes the code
//! itself for this: you run CodeQL where and how you trust, then import
//! its report.
//!
//! A report's paths are kept only if they name one of the project's
//! files: absolute ones inside the project, or relative ones that don't
//! climb out of it. A report quotes the project it is about, so its text
//! is cut to size, shown escaped, and its tools may not take the names of
//! SrcWranglr's own (which would replace, or pass for, what they found).

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use super::{Finding, Severity};

/// Reports larger than this aren't read.
pub const MAX_SARIF_BYTES: u64 = 64 * 1024 * 1024;
/// Results kept from one report, at most, and on one line of a file.
pub const MAX_RESULTS: usize = 200_000;
pub const MAX_RESULTS_PER_LINE: usize = 50;
/// Tools kept apart; the results of any more go under one name.
const MAX_TOOLS: usize = 50;
const OTHER_TOOLS: &str = "SARIF (other tools)";
/// A result's message is cut to this many characters (and to twice that
/// before its links are read, which looks ahead).
const MAX_MESSAGE_CHARS: usize = 2_000;
const MAX_RAW_MESSAGE_CHARS: usize = 2 * MAX_MESSAGE_CHARS;
/// A tool's name is cut to this many characters, a rule id to twice that.
const MAX_TOOL_CHARS: usize = 60;
/// What a run's tool is called if it doesn't say.
const UNNAMED_TOOL: &str = "SARIF";
/// The names SrcWranglr's own findings go under: an import may not take
/// them, or it would replace (or pass for) what they found.
const RESERVED_TOOLS: [&str; 7] = [
    super::risky::TOOL,
    super::risky::LIST_TOOL,
    super::silenced::TOOL,
    crate::history::findings::TOOL,
    super::flawfinder::TOOL,
    "semgrep",
    "cppcheck",
];

/// What a report held.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Imported {
    /// Each tool's findings, in the order its runs came.
    pub tools: Vec<(String, Vec<Finding>)>,
    /// Results about files outside the project, left out.
    pub outside: usize,
    /// Results with no place in a file, left out.
    pub placeless: usize,
    /// More results than were kept (in all, or on one line).
    pub is_cut_short: bool,
}

impl Imported {
    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.tools.iter().map(|(_, found)| found.len()).sum()
    }
}

/// The findings in the SARIF report `json`, for the project at `root`
/// whose files `text_of` knows (each one's text, by project-relative
/// path).
pub fn parse<'t>(
    json: &str,
    root: &Path,
    text_of: impl Fn(&str) -> Option<&'t str>,
) -> Result<Imported, String> {
    let log: Log = serde_json::from_str(json).map_err(|e| format!("not a SARIF report: {e}"))?;
    if log.runs.is_empty() {
        return Err("not a SARIF report: it has no runs".into());
    }
    let roots: Vec<String> = [std::fs::canonicalize(root).ok(), Some(root.to_path_buf())]
        .into_iter()
        .flatten()
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .collect();
    let mut imported = Imported::default();
    let mut kept = 0usize;
    let mut per_line: HashMap<(String, u32), usize> = HashMap::new();
    for run in &log.runs {
        let tool = match imported.tools.len() < MAX_TOOLS {
            true => tool_name(&run.tool.driver.name),
            false => OTHER_TOOLS.to_string(),
        };
        let is_code_points = run.column_kind.as_deref() == Some("unicodeCodePoints");
        let mut found = Vec::new();
        for result in &run.results {
            if kept >= MAX_RESULTS {
                imported.is_cut_short = true;
                break;
            }
            let Some((file, region)) = place_of(result, run, &roots) else {
                imported.placeless += 1;
                continue;
            };
            let Some((file, text)) = file.and_then(|f| text_of(&f).map(|text| (f, text))) else {
                imported.outside += 1;
                continue;
            };
            let on_line = per_line.entry((file.clone(), region.line())).or_default();
            *on_line += 1;
            if *on_line > MAX_RESULTS_PER_LINE {
                imported.is_cut_short = true;
                continue;
            }
            let place = Place {
                file,
                text,
                region: &region,
                is_code_points,
            };
            found.push(finding(&tool, run, result, place));
            kept += 1;
        }
        match imported.tools.iter_mut().find(|(name, _)| *name == tool) {
            Some((_, earlier)) => earlier.extend(found),
            None => imported.tools.push((tool, found)),
        }
    }
    Ok(imported)
}

/// Where a result is: its project file, that file's text, its region.
struct Place<'p> {
    file: String,
    text: &'p str,
    region: &'p Region,
    /// Columns count code points (else UTF-16 units, SARIF's default).
    is_code_points: bool,
}

fn finding(tool: &str, run: &Run, result: &SarifResult, place: Place) -> Finding {
    let rule = rule_of(result, run);
    let rule_id = result
        .rule_id
        .clone()
        .or_else(|| result.rule.as_ref().and_then(|r| r.id.clone()))
        .or_else(|| rule.map(|r| r.id.clone()))
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| "result".into());
    let level = result
        .level
        .as_deref()
        .or_else(|| rule.and_then(|r| r.default_configuration.as_ref()?.level.as_deref()));
    let security = rule.and_then(|r| r.properties.security_severity.as_ref().and_then(number));
    let region = place.region;
    let line = region.line();
    let is_one_line = region.end_line().is_none_or(|end| end == line);
    let columns = match (column(region.start_column), column(region.end_column)) {
        (Some(start), Some(end)) if is_one_line && end > start => {
            byte_columns(place.text, line, start..end, place.is_code_points)
        }
        _ => None,
    };
    // Cut before its links are read: reading them looks ahead.
    let message = without_link_targets(&cut(&result.message.text, MAX_RAW_MESSAGE_CHARS));
    Finding {
        tool: tool.to_string(),
        rule: cut(&rule_id, MAX_TOOL_CHARS * 2),
        severity: severity(level, security),
        message: cut(&message, MAX_MESSAGE_CHARS),
        fix: rule
            .and_then(|r| r.short_description.as_ref())
            .map(|d| cut(&d.text, MAX_MESSAGE_CHARS))
            .filter(|text| !text.is_empty()),
        cwe: rule
            .and_then(|r| cwe_in(&r.properties))
            .or_else(|| cwe_in(&result.properties)),
        file: place.file,
        line,
        columns,
        symbol: None,
    }
}

/// The rule a result is for, by index or id.
fn rule_of<'r>(result: &SarifResult, run: &'r Run) -> Option<&'r Rule> {
    let rules = &run.tool.driver.rules;
    // SARIF writes -1 for "no rule index".
    let index = result
        .rule_index
        .or_else(|| result.rule.as_ref().and_then(|r| r.index))
        .and_then(|i| usize::try_from(i).ok());
    index.and_then(|i| rules.get(i)).or_else(|| {
        let id = result.rule_id.as_deref()?;
        rules.iter().find(|r| r.id == id)
    })
}

/// `security-severity` (CVSS-like, 0 to 10) if given, else the level.
fn severity(level: Option<&str>, security: Option<f64>) -> Severity {
    match (security, level) {
        (Some(score), _) if score >= 7.0 => Severity::High,
        (Some(score), _) if score >= 4.0 => Severity::Medium,
        (Some(_), _) => Severity::Low,
        (None, Some("error")) => Severity::High,
        (None, Some("note" | "none")) => Severity::Low,
        // SARIF's default level is a warning.
        (None, _) => Severity::Medium,
    }
}

fn number(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
}

/// The CWE a rule's or result's tags name (`external/cwe/cwe-120`).
fn cwe_in(properties: &Properties) -> Option<u32> {
    properties.tags.iter().find_map(|tag| {
        let tag = tag.to_ascii_lowercase();
        let number = tag.rsplit("cwe-").next().filter(|_| tag.contains("cwe-"))?;
        number.trim_start_matches('0').parse().ok()
    })
}

/// A 1-based column, if it is one.
fn column(column: Option<i64>) -> Option<u32> {
    column
        .and_then(|c| u32::try_from(c).ok())
        .filter(|c| *c > 0)
}

/// Columns `columns` (1-based, the end not included, counted in UTF-16
/// units or in code points) of `line` of `text`, as byte columns from 0.
fn byte_columns(
    text: &str,
    line: u32,
    columns: std::ops::Range<u32>,
    is_code_points: bool,
) -> Option<(u32, u32)> {
    let row = usize::try_from(line.checked_sub(1)?).ok()?;
    let line_text = text.lines().nth(row)?;
    let byte_at = |wanted: u32| -> Option<usize> {
        let mut unit = 1u32;
        for (byte, c) in line_text.char_indices() {
            if unit == wanted {
                return Some(byte);
            }
            unit += match is_code_points {
                true => 1,
                false => c.len_utf16() as u32,
            };
        }
        (unit == wanted).then_some(line_text.len())
    };
    let (start, end) = (byte_at(columns.start)?, byte_at(columns.end)?);
    Some((u32::try_from(start).ok()?, u32::try_from(end).ok()?))
}

/// The file (project-relative, if it is in the project) and region of a
/// result's first place; `None` if it has none.
fn place_of(result: &SarifResult, run: &Run, roots: &[String]) -> Option<(Option<String>, Region)> {
    let physical = result.locations.first()?.physical_location.as_ref()?;
    let artifact = physical.artifact_location.as_ref()?;
    let base = artifact
        .uri_base_id
        .as_deref()
        .and_then(|id| run.original_uri_base_ids.get(id))
        .map(|base| base.uri.as_str());
    let file = project_path(&artifact.uri, base, roots);
    Some((file, physical.region.clone().unwrap_or_default()))
}

/// `uri` (maybe relative to `base`) as a path relative to one of `roots`
/// (the project's folder as written and canonical), if it is inside it.
fn project_path(uri: &str, base: Option<&str>, roots: &[String]) -> Option<String> {
    let path = local_path(uri)?;
    let is_absolute = path.starts_with('/') || path.get(1..3) == Some(":/");
    let joined = match (is_absolute, base) {
        (true, _) => path,
        (false, Some(base)) => format!("{}/{path}", local_path(base)?.trim_end_matches('/')),
        (false, None) => return relative(&path),
    };
    roots.iter().find_map(|root| {
        let inside = joined.strip_prefix(root.trim_end_matches('/'))?;
        inside.starts_with('/').then(|| relative(inside)).flatten()
    })
}

/// `uri` as a path with `/` between its parts: a `file:` URI of this
/// machine (no host), or a path.
fn local_path(uri: &str) -> Option<String> {
    let path = match uri.strip_prefix("file://") {
        Some(rest) if rest.starts_with('/') => rest,
        Some(_) => return None,
        None => uri,
    };
    Some(decoded(path).replace('\\', "/"))
}

/// `path` without `.` steps or leading `/`, if it doesn't climb (`..`).
fn relative(path: &str) -> Option<String> {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    let is_inside = !parts.is_empty() && !parts.contains(&"..");
    is_inside.then(|| parts.join("/"))
}

/// `%xx` escapes decoded (as UTF-8, lossily).
fn decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A message's `[text](1)` links (to related places) as their text.
fn without_link_targets(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let target = after.find("](").and_then(|close| {
            let inner = &after[close + 2..];
            let end = inner.find(')')?;
            let target = &inner[..end];
            let is_index = !target.is_empty() && target.chars().all(|c| c.is_ascii_digit());
            is_index.then_some((close, close + 2 + end + 1))
        });
        match target {
            Some((close, end)) => {
                out.push_str(&rest[..open]);
                out.push_str(&after[..close]);
                rest = &after[end..];
            }
            None => {
                out.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A run's tool's name as findings go under: hidden characters left out,
/// cut short, and never one of SrcWranglr's own.
fn tool_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|c| !c.is_control() && !crate::app::visible::is_hidden(*c))
        .take(MAX_TOOL_CHARS)
        .collect();
    let name = name.trim();
    let is_reserved = RESERVED_TOOLS.iter().any(|r| r.eq_ignore_ascii_case(name));
    match (name, is_reserved) {
        ("", _) => UNNAMED_TOOL.to_string(),
        (name, true) => format!("{name} (imported)"),
        (name, false) => name.to_string(),
    }
}

fn cut(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let kept: String = chars.by_ref().take(max_chars).collect();
    match chars.next() {
        Some(_) => kept + "…",
        None => kept,
    }
}

#[derive(Deserialize)]
struct Log {
    #[serde(default)]
    runs: Vec<Run>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Run {
    tool: SarifTool,
    #[serde(default)]
    results: Vec<SarifResult>,
    #[serde(default)]
    original_uri_base_ids: HashMap<String, ArtifactLocation>,
    #[serde(default)]
    column_kind: Option<String>,
}

/// The properties read: tags (for the CWE) and CodeQL's
/// `security-severity`; the rest are skipped unread.
#[derive(Deserialize, Default)]
struct Properties {
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default, rename = "security-severity")]
    security_severity: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct SarifTool {
    driver: Driver,
}

#[derive(Deserialize)]
struct Driver {
    #[serde(default)]
    name: String,
    #[serde(default)]
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rule {
    #[serde(default)]
    id: String,
    #[serde(default)]
    short_description: Option<Message>,
    #[serde(default)]
    default_configuration: Option<Configuration>,
    #[serde(default)]
    properties: Properties,
}

#[derive(Deserialize)]
struct Configuration {
    #[serde(default)]
    level: Option<String>,
}

#[derive(Deserialize, Default)]
struct Message {
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SarifResult {
    #[serde(default)]
    rule_id: Option<String>,
    #[serde(default)]
    rule_index: Option<i64>,
    #[serde(default)]
    rule: Option<RuleReference>,
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    message: Message,
    #[serde(default)]
    locations: Vec<Location>,
    #[serde(default)]
    properties: Properties,
}

#[derive(Deserialize)]
struct RuleReference {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    index: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    #[serde(default)]
    physical_location: Option<PhysicalLocation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhysicalLocation {
    #[serde(default)]
    artifact_location: Option<ArtifactLocation>,
    #[serde(default)]
    region: Option<Region>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactLocation {
    #[serde(default)]
    uri: String,
    #[serde(default)]
    uri_base_id: Option<String>,
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct Region {
    #[serde(default)]
    start_line: Option<i64>,
    #[serde(default)]
    start_column: Option<i64>,
    #[serde(default)]
    end_line: Option<i64>,
    #[serde(default)]
    end_column: Option<i64>,
}

impl Region {
    /// Its first line; 1 if it has none (or one that can't be).
    fn line(&self) -> u32 {
        self.start_line
            .and_then(|l| u32::try_from(l).ok())
            .unwrap_or(1)
            .max(1)
    }

    fn end_line(&self) -> Option<u32> {
        self.end_line.and_then(|l| u32::try_from(l).ok())
    }
}

#[cfg(test)]
#[path = "sarif_tests.rs"]
mod tests;

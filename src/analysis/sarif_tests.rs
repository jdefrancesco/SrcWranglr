use super::*;

const REPORT: &str = r#"{
  "version": "2.1.0",
  "runs": [{
    "tool": {"driver": {"name": "CodeQL", "rules": [
      {"id": "cpp/overflow-buffer",
       "shortDescription": {"text": "Call to memory access function may overflow buffer"},
       "properties": {"tags": ["security", "external/cwe/cwe-120"], "security-severity": "9.3"}},
      {"id": "cpp/unused-local", "defaultConfiguration": {"level": "note"}}
    ]}},
    "originalUriBaseIds": {"%SRCROOT%": {"uri": "file:///work/proj/"}},
    "results": [
      {"ruleId": "cpp/overflow-buffer", "ruleIndex": 0,
       "message": {"text": "This [copy](1) may overflow [the buffer](2)."},
       "locations": [{"physicalLocation": {
         "artifactLocation": {"uri": "src/net.c", "uriBaseId": "%SRCROOT%"},
         "region": {"startLine": 12, "startColumn": 3, "endColumn": 9}}}]},
      {"ruleId": "cpp/unused-local", "ruleIndex": 1, "message": {"text": "Unused."},
       "locations": [{"physicalLocation": {
         "artifactLocation": {"uri": "file:///work/proj/src/my%20file.c"},
         "region": {"startLine": 4}}}]},
      {"ruleId": "cpp/overflow-buffer", "message": {"text": "Outside."},
       "locations": [{"physicalLocation": {
         "artifactLocation": {"uri": "/usr/include/string.h"}, "region": {"startLine": 1}}}]},
      {"ruleId": "cpp/overflow-buffer", "message": {"text": "Climbs out."},
       "locations": [{"physicalLocation": {"artifactLocation": {"uri": "../../etc/passwd"}}}]},
      {"ruleId": "cpp/overflow-buffer", "message": {"text": "Nowhere."}}
    ]
  }, {
    "tool": {"driver": {"name": "Semgrep OSS"}},
    "results": [{"ruleId": "x", "level": "error", "message": {"text": "m"},
      "locations": [{"physicalLocation": {"artifactLocation": {"uri": "./src/net.c"},
        "region": {"startLine": 2, "endLine": 4, "startColumn": 1, "endColumn": 5}}}]}]
  }]
}"#;

/// Line 12 of net.c holds the overflowing copy.
const NET_C: &str = "\n\n\n\n\n\n\n\n\n\n\n  memcpy(buf, src, n);\n";

fn text_of(file: &str) -> Option<&'static str> {
    match file {
        "src/net.c" => Some(NET_C),
        "src/my file.c" => Some("a\nb\nc\nint unused;\n"),
        _ => None,
    }
}

fn imported() -> Imported {
    parse(REPORT, Path::new("/work/proj"), text_of).expect("parsed")
}

#[test]
fn each_tools_results_become_findings_in_the_project() {
    let imported = imported();
    let names: Vec<&str> = imported.tools.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["CodeQL", "Semgrep OSS"]);
    assert_eq!(imported.count(), 3);
    assert_eq!((imported.outside, imported.placeless), (2, 1));
    let overflow = &imported.tools[0].1[0];
    assert_eq!(overflow.tool, "CodeQL");
    assert_eq!(overflow.rule, "cpp/overflow-buffer");
    assert_eq!(overflow.severity, Severity::High);
    assert_eq!(overflow.cwe, Some(120));
    assert_eq!((overflow.file.as_str(), overflow.line), ("src/net.c", 12));
    assert_eq!(overflow.columns, Some((2, 8)));
    assert_eq!(overflow.message, "This copy may overflow the buffer.");
    assert_eq!(
        overflow.fix.as_deref(),
        Some("Call to memory access function may overflow buffer")
    );
}

#[test]
fn levels_and_paths_are_read_as_sarif_writes_them() {
    let imported = imported();
    let unused = &imported.tools[0].1[1];
    assert_eq!(unused.file, "src/my file.c");
    assert_eq!(unused.severity, Severity::Low);
    assert_eq!(unused.columns, None);
    let other = &imported.tools[1].1[0];
    assert_eq!(
        (other.file.as_str(), other.severity),
        ("src/net.c", Severity::High)
    );
    // Over several lines: no columns.
    assert_eq!(other.columns, None);
}

#[test]
fn paths_never_leave_the_project() {
    let roots = vec!["/work/proj".to_string()];
    let path = |uri: &str, base: Option<&str>| project_path(uri, base, &roots);
    assert_eq!(path("a/./b.c", None).as_deref(), Some("a/b.c"));
    assert_eq!(path("a/../../b.c", None), None);
    assert_eq!(path("a/%2e%2e/%2e%2e/b.c", None), None);
    assert_eq!(path("file:///work/projector/x.c", None), None);
    assert_eq!(path("/work/proj/x.c", None).as_deref(), Some("x.c"));
    assert_eq!(path("x.c", Some("file:///elsewhere/")), None);
    // A file on another machine isn't one of the project's.
    assert_eq!(path("file://host/work/proj/x.c", None), None);
}

#[test]
fn what_isnt_sarif_is_refused() {
    let none = |_: &str| Some("");
    assert!(parse("{}", Path::new("/p"), none).is_err());
    assert!(parse("not json", Path::new("/p"), none).is_err());
    let unnamed = r#"{"runs": [{"tool": {"driver": {"name": "\u0007"}}, "results": []}]}"#;
    let imported = parse(unnamed, Path::new("/p"), none).expect("parsed");
    assert_eq!(imported.tools[0].0, "SARIF");
}

#[test]
fn a_tool_may_not_take_srcwranglrs_own_names() {
    for name in ["semgrep", "SrcWranglr", "git history", "Flawfinder"] {
        assert_eq!(tool_name(name), format!("{name} (imported)"));
    }
    assert_eq!(tool_name("Code\u{202E}QL"), "CodeQL");
}

#[test]
fn a_missing_rule_index_and_odd_numbers_dont_spoil_the_report() {
    let report = r#"{"runs": [{"tool": {"driver": {"name": "T", "rules": [{"id": "r"}]}},
      "results": [{"ruleId": "r", "ruleIndex": -1, "message": {"text": "m"},
        "locations": [{"physicalLocation": {"artifactLocation": {"uri": "src/net.c"},
          "region": {"startLine": -3, "startColumn": -1}}}]}]}]}"#;
    let imported = parse(report, Path::new("/work/proj"), text_of).expect("parsed");
    let found = &imported.tools[0].1[0];
    assert_eq!(
        (found.rule.as_str(), found.line, found.columns),
        ("r", 1, None)
    );
}

#[test]
fn columns_in_utf16_units_become_byte_columns() {
    let text = "x\n/* é😀 */ copy(a);\n";
    // `copy` starts at UTF-16 column 11 (é is 1 unit, 😀 is 2).
    assert_eq!(byte_columns(text, 2, 11..15, false), Some((13, 17)));
    assert_eq!(byte_columns(text, 2, 10..14, true), Some((13, 17)));
    assert_eq!(byte_columns(text, 9, 1..2, false), None);
}

#[test]
fn a_huge_message_full_of_brackets_is_read_quickly() {
    let message = "[".repeat(1_000_000);
    let report = format!(
        r#"{{"runs": [{{"tool": {{"driver": {{"name": "T"}}}}, "results": [{{"message": {{"text": "{message}"}},
          "locations": [{{"physicalLocation": {{"artifactLocation": {{"uri": "src/net.c"}}}}}}]}}]}}]}}"#
    );
    let started = std::time::Instant::now();
    let imported = parse(&report, Path::new("/work/proj"), text_of).expect("parsed");
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    assert!(imported.tools[0].1[0].message.chars().count() <= 2_001);
}

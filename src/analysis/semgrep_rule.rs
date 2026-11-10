//! The YAML of the semgrep rules SrcWranglr writes: drafted from code you
//! pick, from examples, or for the project's wrappers of risky calls.
//!
//! What goes in comes from the code being audited, so every value is
//! written double-quoted with anything but printable ASCII escaped. A
//! pattern over several lines of plain printable text is written as a
//! block of its own instead, to stay readable; every line of it is
//! indented under its key, so none can end the block or start a key.

use super::Severity;

/// How serious a rule says its matches are, as semgrep writes it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RuleSeverity {
    Error,
    #[default]
    Warning,
    Info,
}

impl RuleSeverity {
    fn as_str(self) -> &'static str {
        match self {
            RuleSeverity::Error => "ERROR",
            RuleSeverity::Warning => "WARNING",
            RuleSeverity::Info => "INFO",
        }
    }
}

impl From<Severity> for RuleSeverity {
    /// The other way from how semgrep's results are read.
    fn from(severity: Severity) -> Self {
        match severity {
            Severity::High => RuleSeverity::Error,
            Severity::Medium => RuleSeverity::Warning,
            Severity::Low => RuleSeverity::Info,
        }
    }
}

/// One clause of a rule's `patterns`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clause {
    Pattern(String),
    /// Matches that are also this aren't reported.
    Not(String),
    /// Matches inside this aren't reported.
    NotInside(String),
    /// Any of these.
    Either(Vec<String>),
    /// The metavariable's code must match this regular expression (from
    /// its start).
    Regex {
        metavariable: String,
        regex: String,
    },
}

/// What an exception to a rule leaves out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExceptionKind {
    /// Matches that look like this: the fixed form of the same code.
    #[default]
    LooksLike,
    /// Matches after this: a check earlier in the function.
    After,
    /// Matches inside this.
    Inside,
}

impl ExceptionKind {
    pub const ALL: [ExceptionKind; 3] = [
        ExceptionKind::LooksLike,
        ExceptionKind::After,
        ExceptionKind::Inside,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ExceptionKind::LooksLike => "Not when it is",
            ExceptionKind::After => "Not after",
            ExceptionKind::Inside => "Not inside",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            ExceptionKind::LooksLike => {
                "Leave out matches that are this code: the fixed form (pattern-not)"
            }
            ExceptionKind::After => {
                "Leave out matches that come after this code, such as a check of the size \
                 (pattern-not-inside, followed by ...)"
            }
            ExceptionKind::Inside => "Leave out matches inside this code (pattern-not-inside)",
        }
    }
}

/// An exception to a rule: matches it covers aren't reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Exception {
    pub kind: ExceptionKind,
    pub pattern: String,
}

impl Exception {
    /// The clause that leaves the exception out; `None` if it is blank.
    pub fn clause(&self) -> Option<Clause> {
        let pattern = self.pattern.trim();
        if pattern.is_empty() {
            return None;
        }
        Some(match self.kind {
            ExceptionKind::LooksLike => Clause::Not(pattern.to_string()),
            ExceptionKind::After => Clause::NotInside(format!("{pattern}\n...")),
            ExceptionKind::Inside => Clause::NotInside(pattern.to_string()),
        })
    }
}

/// A rule to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSpec {
    /// Letters, digits, `-` and `_` only: also a file name.
    pub id: String,
    pub message: String,
    pub severity: RuleSeverity,
    pub clauses: Vec<Clause>,
}

/// A rules file holding `rules`.
pub fn rules_yaml(rules: &[RuleSpec]) -> String {
    rules.iter().fold(String::from("rules:\n"), |out, rule| {
        out + &rule_entry(rule)
    })
}

fn rule_entry(rule: &RuleSpec) -> String {
    let head = format!(
        concat!(
            "  - id: {id}\n",
            "    languages: [c, cpp]\n",
            "    severity: {severity}\n",
            "    message: {message}\n",
        ),
        id = rule.id,
        severity = rule.severity.as_str(),
        message = quoted(&rule.message),
    );
    match rule.clauses.as_slice() {
        [Clause::Pattern(pattern)] => head + &format!("    pattern: {}\n", value(pattern, 6)),
        clauses => clauses
            .iter()
            .fold(head + "    patterns:\n", |out, clause| {
                out + &clause_yaml(clause)
            }),
    }
}

fn clause_yaml(clause: &Clause) -> String {
    const INDENT: usize = 10;
    match clause {
        Clause::Pattern(p) => format!("      - pattern: {}\n", value(p, INDENT)),
        Clause::Not(p) => format!("      - pattern-not: {}\n", value(p, INDENT)),
        Clause::NotInside(p) => format!("      - pattern-not-inside: {}\n", value(p, INDENT)),
        Clause::Either(patterns) => patterns
            .iter()
            .fold(String::from("      - pattern-either:\n"), |out, p| {
                out + &format!("          - pattern: {}\n", value(p, INDENT + 4))
            }),
        Clause::Regex {
            metavariable,
            regex,
        } => format!(
            "      - metavariable-regex:\n          metavariable: {}\n          regex: {}\n",
            quoted(metavariable),
            quoted(regex)
        ),
    }
}

/// `text` as a YAML value at `indent` spaces: a block for several lines
/// of plain printable text that start flush, else double-quoted.
fn value(text: &str, indent: usize) -> String {
    let is_plain = text.chars().all(|c| c == '\n' || (' '..='~').contains(&c));
    let is_flush = text.lines().all(|line| !line.starts_with(' '));
    if !text.contains('\n') || !is_plain || !is_flush {
        return quoted(text);
    }
    let pad = " ".repeat(indent);
    text.lines()
        .fold(String::from("|"), |out, line| match line {
            "" => out + "\n",
            line => out + "\n" + &pad + line,
        })
}

/// `text` as a YAML double-quoted string: anything but printable ASCII
/// escaped, so nothing in it can end the string or hide in it.
pub fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            ' '..='~' => out.push(c),
            c if u32::from(c) <= 0xFFFF => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push_str(&format!("\\U{:08X}", u32::from(c))),
        }
    }
    out.push('"');
    out
}

/// `raw` made into a rule id (also a file name): lowercase letters,
/// digits and single `-`, at most `max_chars` long.
pub fn rule_id(raw: &str, max_chars: usize) -> String {
    let mut id = String::new();
    for c in raw.chars().take(max_chars) {
        let c = c.to_ascii_lowercase();
        let keep = if c.is_ascii_alphanumeric() { c } else { '-' };
        if !(keep == '-' && id.ends_with('-')) {
            id.push(keep);
        }
    }
    id.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(clauses: Vec<Clause>) -> RuleSpec {
        RuleSpec {
            id: "r".into(),
            message: "m".into(),
            severity: RuleSeverity::Warning,
            clauses,
        }
    }

    #[test]
    fn one_pattern_is_written_on_its_own() {
        let yaml = rules_yaml(&[spec(vec![Clause::Pattern("f($A)".into())])]);
        assert_eq!(
            yaml,
            "rules:\n  - id: r\n    languages: [c, cpp]\n    severity: WARNING\n    \
             message: \"m\"\n    pattern: \"f($A)\"\n"
        );
    }

    #[test]
    fn exceptions_make_a_list_of_patterns() {
        let after = Exception {
            kind: ExceptionKind::After,
            pattern: "if (<... $C > 4 ...>) ...".into(),
        };
        let clauses = vec![
            Clause::Pattern("memcpy($A, $B, $C)".into()),
            after.clause().expect("clause"),
            Clause::Regex {
                metavariable: "$C".into(),
                regex: ".*[a-z]".into(),
            },
        ];
        let yaml = rules_yaml(&[spec(clauses)]);
        assert!(yaml.contains("    patterns:\n      - pattern: \"memcpy($A, $B, $C)\"\n"));
        assert!(yaml.contains(
            "      - pattern-not-inside: |\n          if (<... $C > 4 ...>) ...\n          ...\n"
        ));
        assert!(yaml.contains("          metavariable: \"$C\"\n          regex: \".*[a-z]\"\n"));
    }

    #[test]
    fn a_blank_exception_leaves_nothing_out() {
        let blank = Exception {
            kind: ExceptionKind::LooksLike,
            pattern: "  ".into(),
        };
        assert_eq!(blank.clause(), None);
    }

    #[test]
    fn lines_with_anything_unusual_are_quoted_instead() {
        assert_eq!(value("a;\n...\nb;", 4), "|\n    a;\n    ...\n    b;");
        assert_eq!(value("a;\n\tb;", 4), "\"a;\\n\\tb;\"");
        assert_eq!(value(" a;\nb;", 4), "\" a;\\nb;\"");
        assert_eq!(value("a;\n- id: x\u{202E}", 4), "\"a;\\n- id: x\\u202E\"");
    }

    #[test]
    fn nothing_from_the_code_can_break_out_of_a_quoted_value() {
        assert_eq!(
            quoted("a\"b\\c\nd\u{1F600}"),
            "\"a\\\"b\\\\c\\nd\\U0001F600\""
        );
    }

    #[test]
    fn ids_keep_letters_and_digits() {
        assert_eq!(rule_id("like-ns::Copy-2", 60), "like-ns-copy-2");
        assert_eq!(rule_id("--a!!b--", 60), "a-b");
        assert_eq!(rule_id("abcdef", 3), "abc");
    }
}

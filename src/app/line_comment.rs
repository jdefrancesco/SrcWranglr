//! Toggling `//` line comments over a range of lines, the way an editor does:
//! comment the lines unless every non-blank one is already commented, in which
//! case uncomment them instead. Indentation and line endings are preserved,
//! blank lines are left alone.

/// The C/C++ line-comment marker, written with a trailing space when added.
const MARKER: &str = "//";

/// `text` with 1-based lines `first..=last` (inclusive) commented or
/// uncommented. Returns it unchanged when the range holds no non-blank line or
/// falls outside the file.
pub fn toggle(text: &str, first: u32, last: u32) -> String {
    if first == 0 || last < first {
        return text.to_string();
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let first = first as usize;
    if first > lines.len() {
        return text.to_string();
    }
    let last = (last as usize).min(lines.len());
    let targets = &lines[first - 1..last];
    // Nothing to toggle if every target line is blank.
    if targets.iter().all(|line| content(line).trim().is_empty()) {
        return text.to_string();
    }
    // Uncomment only when every non-blank target line is already commented;
    // otherwise comment them all (the familiar editor toggle).
    let uncomment = targets
        .iter()
        .filter(|line| !content(line).trim().is_empty())
        .all(|line| is_commented(line));

    let mut out = String::with_capacity(text.len() + targets.len() * (MARKER.len() + 1));
    for (i, line) in lines.iter().enumerate() {
        let n = i + 1;
        if (first..=last).contains(&n) && !content(line).trim().is_empty() {
            out.push_str(&transform(line, uncomment));
        } else {
            out.push_str(line);
        }
    }
    out
}

/// The line without its `\n` or `\r\n` terminator.
fn content(line: &str) -> &str {
    let without_nl = line.strip_suffix('\n').unwrap_or(line);
    without_nl.strip_suffix('\r').unwrap_or(without_nl)
}

/// The `\n` / `\r\n` the line ends with (empty on the last, unterminated line).
fn terminator(line: &str) -> &str {
    &line[content(line).len()..]
}

/// Whether the line's first non-blank text starts with `//`.
fn is_commented(line: &str) -> bool {
    content(line).trim_start().starts_with(MARKER)
}

/// Add or remove the `//` marker, keeping indentation and the terminator.
fn transform(line: &str, uncomment: bool) -> String {
    let content = content(line);
    let term = terminator(line);
    let rest = content.trim_start();
    let indent = &content[..content.len() - rest.len()];
    if uncomment {
        // Drop the `//` and a single following space (what commenting added).
        let Some(after) = rest.strip_prefix(MARKER) else {
            return line.to_string();
        };
        let after = after.strip_prefix(' ').unwrap_or(after);
        format!("{indent}{after}{term}")
    } else {
        format!("{indent}{MARKER} {rest}{term}")
    }
}

#[cfg(test)]
mod tests {
    use super::toggle;

    #[test]
    fn commenting_a_line_adds_a_marker_and_keeps_indentation() {
        assert_eq!(toggle("    free(p);\n", 1, 1), "    // free(p);\n");
    }

    #[test]
    fn commenting_then_uncommenting_round_trips() {
        let original = "\tint x = 1;\n";
        let commented = toggle(original, 1, 1);
        assert_eq!(commented, "\t// int x = 1;\n");
        assert_eq!(toggle(&commented, 1, 1), original);
    }

    #[test]
    fn a_range_comments_every_non_blank_line() {
        let src = "a();\nb();\nc();\n";
        assert_eq!(toggle(src, 1, 3), "// a();\n// b();\n// c();\n");
    }

    #[test]
    fn a_fully_commented_range_is_uncommented() {
        let src = "// a();\n// b();\n";
        assert_eq!(toggle(src, 1, 2), "a();\nb();\n");
    }

    #[test]
    fn a_partly_commented_range_comments_the_rest() {
        // Not every line is commented, so the toggle comments them all.
        let src = "// a();\nb();\n";
        assert_eq!(toggle(src, 1, 2), "// // a();\n// b();\n");
    }

    #[test]
    fn blank_lines_in_the_range_are_left_alone() {
        let src = "a();\n\n   \nb();\n";
        assert_eq!(toggle(src, 1, 4), "// a();\n\n   \n// b();\n");
    }

    #[test]
    fn crlf_line_endings_are_preserved() {
        assert_eq!(toggle("a();\r\n", 1, 1), "// a();\r\n");
        assert_eq!(toggle("// a();\r\n", 1, 1), "a();\r\n");
    }

    #[test]
    fn a_final_line_without_a_newline_is_handled() {
        assert_eq!(toggle("a();", 1, 1), "// a();");
    }

    #[test]
    fn a_range_past_the_end_clamps_to_the_last_line() {
        assert_eq!(toggle("a();\nb();\n", 2, 99), "a();\n// b();\n");
    }

    #[test]
    fn an_out_of_range_or_empty_selection_changes_nothing() {
        assert_eq!(toggle("a();\n", 0, 0), "a();\n");
        assert_eq!(toggle("a();\n", 5, 9), "a();\n");
        assert_eq!(toggle("a();\n", 2, 1), "a();\n");
    }

    #[test]
    fn only_lines_in_the_range_change() {
        let src = "keep();\ntoggle();\nkeep();\n";
        assert_eq!(toggle(src, 2, 2), "keep();\n// toggle();\nkeep();\n");
    }

    #[test]
    fn uncommenting_tolerates_a_missing_space_after_the_marker() {
        assert_eq!(toggle("//tight();\n", 1, 1), "tight();\n");
    }
}

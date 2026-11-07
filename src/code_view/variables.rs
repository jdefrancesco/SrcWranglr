//! The names on a line of code that may be variables, for the line's
//! trace menus. Lexical and quick: the trace itself reads the
//! syntax and says when a name isn't a variable after all.

/// Entries past this many aren't listed.
const MAX_VARIABLES: usize = 20;

/// Words that are never variables.
const NOT_VARIABLES: &[&str] = &[
    "if",
    "else",
    "for",
    "while",
    "do",
    "switch",
    "case",
    "return",
    "break",
    "continue",
    "goto",
    "sizeof",
    "struct",
    "class",
    "union",
    "enum",
    "typedef",
    "const",
    "static",
    "volatile",
    "extern",
    "inline",
    "unsigned",
    "signed",
    "char",
    "short",
    "int",
    "long",
    "float",
    "double",
    "void",
    "bool",
    "auto",
    "register",
    "new",
    "delete",
    "this",
    "true",
    "false",
    "nullptr",
    "public",
    "private",
    "protected",
    "virtual",
    "override",
    "template",
    "typename",
    "namespace",
    "using",
    "operator",
    "default",
    "explicit",
    "friend",
    "mutable",
    "constexpr",
    "noexcept",
    "throw",
    "try",
    "catch",
    "static_cast",
    "dynamic_cast",
    "reinterpret_cast",
    "const_cast",
    "decltype",
    "alignof",
    "wchar_t",
];

/// The names on `line` that may be variables, each with the byte column to
/// trace it at: `buf`, `conn`, and `conn->len` (at `len`). Left out are
/// keywords, calls (`f(`), scopes (`std::`), types (a name followed by
/// another), `_t` types, upper-case macros, and anything in a string,
/// character literal or comment. Each name once, first place first.
pub(super) fn variables_in(line: &str) -> Vec<(String, usize)> {
    let mut found: Vec<(String, usize)> = Vec::new();
    let mut chain: Option<String> = None;
    let mut member_op: Option<&str> = None;
    let mut i = 0;
    while let Some(c) = line[i..].chars().next() {
        let rest = &line[i..];
        if rest.starts_with("//") {
            break;
        }
        if rest.starts_with("/*") {
            i = rest.find("*/").map_or(line.len(), |end| i + end + 2);
            chain = None;
            continue;
        }
        if c == '"' || c == '\'' {
            i = literal_end(line, i, c);
            chain = None;
            continue;
        }
        if rest.starts_with("->") || c == '.' {
            let op = if c == '.' { "." } else { "->" };
            member_op = chain.as_ref().map(|_| op);
            i += op.len();
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let (end, name) = name_at(line, i, member_op.take().zip(chain.take()));
            if let Some(name) = name {
                if !found.iter().any(|(n, _)| *n == name) {
                    found.push((name.clone(), i));
                }
                chain = Some(name);
            }
            i = end;
            continue;
        }
        if !c.is_whitespace() {
            chain = None;
            member_op = None;
        }
        i += c.len_utf8();
    }
    found.truncate(MAX_VARIABLES);
    found
}

/// The word starting at byte `start` of `line`: where it ends, and the
/// variable it names, if any. After `member` (an operator and the object
/// before it) it is that object's field.
fn name_at(line: &str, start: usize, member: Option<(&str, String)>) -> (usize, Option<String>) {
    let end = line[start..]
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(line.len(), |n| start + n);
    let word = &line[start..end];
    let (before, after) = (line[..start].trim_end(), line[end..].trim_start());
    let name = match member {
        Some((op, object)) => Some(format!("{object}{op}{word}")),
        None if is_variable(word, before, after) => Some(word.to_string()),
        None => None,
    };
    let is_call = after.starts_with('(');
    (end, name.filter(|_| !is_call))
}

/// Whether `word`, between `before` and `after`, can be a variable.
fn is_variable(word: &str, before: &str, after: &str) -> bool {
    let is_macro = word.chars().any(|c| c.is_ascii_uppercase())
        && word
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    let is_scope = after.starts_with("::") || before.ends_with("::");
    // `Foo *p` and `Foo &r` starting a statement declare `p` and `r`.
    let before_comment = match before.strip_suffix("*/") {
        Some(inside) => inside
            .rfind("/*")
            .map_or(before, |at| before[..at].trim_end()),
        None => before,
    };
    let is_statement_start = before_comment.is_empty() || before_comment.ends_with([';', '{', '}']);
    let declares = after
        .trim_start_matches(['*', '&'])
        .trim_start()
        .starts_with(|c: char| c.is_alphabetic() || c == '_');
    let is_declared_type = is_statement_start && after.starts_with(['*', '&']) && declares;
    let is_type = is_declared_type
        || after.starts_with(|c: char| c.is_alphabetic() || c == '_')
        || word.ends_with("_t")
        || before.ends_with("struct")
        || before.ends_with("class")
        || before.ends_with("enum")
        || before.ends_with("union");
    !(NOT_VARIABLES.contains(&word) || is_macro || is_scope || is_type)
}

/// Just past the literal opening at `start` with `quote` (or the line's
/// end if it doesn't close).
fn literal_end(line: &str, start: usize, quote: char) -> usize {
    let mut is_escaped = false;
    for (offset, c) in line[start + 1..].char_indices() {
        match c {
            _ if is_escaped => is_escaped = false,
            '\\' => is_escaped = true,
            c if c == quote => return start + 1 + offset + 1,
            _ => {}
        }
    }
    line.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(line: &str) -> Vec<String> {
        variables_in(line).into_iter().map(|(n, _)| n).collect()
    }

    #[test]
    fn lists_the_variables_on_a_line() {
        assert_eq!(names("    strcpy(buf, name);"), ["buf", "name"]);
        assert_eq!(names("char *name = trim(raw);"), ["name", "raw"]);
        assert_eq!(names("int n = atoi(argv[1]); // n is it"), ["n", "argv"]);
    }

    #[test]
    fn leaves_out_types_scopes_macros_and_literals() {
        assert_eq!(
            names("std::string s = \"a b\"; size_t len = MAX_LEN;"),
            ["s", "len"]
        );
        assert_eq!(names("struct req q; /* r */ Foo *p = 0;"), ["q", "p"]);
        assert_eq!(names("c = 'x'; if (c) return;"), ["c"]);
    }

    #[test]
    fn lists_a_field_with_its_object() {
        let found = variables_in("memcpy(conn->buf, src.data, n);");
        let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["conn", "conn->buf", "src", "src.data", "n"]);
        let column = found[1].1;
        assert_eq!(
            &"memcpy(conn->buf, src.data, n);"[column..column + 3],
            "buf"
        );
    }

    #[test]
    fn a_method_call_is_not_a_variable() {
        assert_eq!(names("strcpy(d, s.c_str());"), ["d", "s"]);
    }
}

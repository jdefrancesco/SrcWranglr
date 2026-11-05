//! Reading C/C++ text without a parser: blanking literals and comments,
//! finding a call's argument list, and the variable an assignment sets.
//! Text keeps its byte offsets throughout, so results line up with the
//! source and the indexer's byte columns.

/// How far any scan looks before giving up.
const MAX_SCAN: usize = 4096;
/// Words in code that are never the input itself.
const NOT_NAMES: &[&str] = &[
    "char",
    "int",
    "unsigned",
    "signed",
    "short",
    "long",
    "void",
    "const",
    "volatile",
    "struct",
    "union",
    "enum",
    "sizeof",
    "NULL",
    "nullptr",
    "uint8_t",
    "size_t",
    "static_cast",
    "reinterpret_cast",
    "const_cast",
    "auto",
    "std",
];

/// A source file with its string and character literals and comments
/// blanked to spaces, byte for byte, and where each line starts.
#[derive(Debug, Clone)]
pub(super) struct Code {
    text: String,
    line_starts: Vec<usize>,
}

impl Code {
    pub(super) fn of(source: &str) -> Self {
        let text = blanked(source);
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self { text, line_starts }
    }

    /// The text, literals and comments blanked.
    pub(super) fn text(&self) -> &str {
        &self.text
    }

    /// The byte offset of 1-based `line`, byte `column`.
    pub(super) fn offset(&self, line: u32, column: u32) -> Option<usize> {
        let start = *self.line_starts.get(line.checked_sub(1)? as usize)?;
        let at = start.checked_add(column as usize)?;
        (at <= self.text.len()).then_some(at)
    }

    /// The argument list of the call whose name ends at `at`, without its
    /// parentheses; `None` if there is none or it doesn't close soon.
    pub(super) fn arguments_at(&self, at: usize) -> Option<&str> {
        let (open, close) = self.parentheses_at(at)?;
        self.text.get(open + 1..close)
    }

    /// Where the argument list of the call whose name ends at `at` opens
    /// and closes (template arguments may come first).
    fn parentheses_at(&self, at: usize) -> Option<(usize, usize)> {
        let text = self.text.as_str();
        let mut open = skip_space(text, at);
        if text.get(open..)?.starts_with('<') {
            open = skip_space(text, matching(text, open, '<', '>')? + 1);
        }
        if !text.get(open..)?.starts_with('(') {
            return None;
        }
        Some((open, matching(text, open, '(', ')')?))
    }

    /// The call that starts at `at` (its name through its `)`), or the
    /// rest of that line if no call starts there.
    pub(super) fn call_at(&self, at: usize) -> &str {
        let text = self.text.as_str();
        let name_end = text
            .get(at..)
            .and_then(|rest| rest.find(|c: char| !(c.is_alphanumeric() || "_:".contains(c))))
            .map_or(text.len(), |i| at + i);
        if let Some((_, close)) = self.parentheses_at(name_end) {
            return text.get(at..=close).unwrap_or("");
        }
        let line_end = text
            .get(at..)
            .and_then(|rest| rest.find('\n'))
            .map_or(text.len(), |i| at + i);
        text.get(at..line_end).unwrap_or("")
    }

    /// The variable the value of the call at `at` is assigned to:
    /// `h = getenv(…)`, `char *h = (char *)std::getenv(…)`,
    /// `buf[n++] = getchar()`, `if ((c = getc(f)) …`, even across lines.
    pub(super) fn assigned_at(&self, at: usize) -> Option<String> {
        let text = self.text.as_str();
        let lower = at.saturating_sub(MAX_SCAN);
        let mut before = text.get(lower..at)?.trim_end();
        // A qualified name: `std::getenv` with `at` on `getenv`.
        while let Some(rest) = before.strip_suffix("::") {
            before = rest
                .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
                .trim_end();
        }
        // A cast: `(char *) getenv(…)`.
        if before.ends_with(')') {
            let open = matching_back(before, before.len() - 1, '(', ')')?;
            before = before[..open].trim_end();
        }
        let before = before.strip_suffix('=')?;
        if before.ends_with(['=', '!', '<', '>']) {
            return None;
        }
        let start = before.rfind([';', '{', '}', '(', ',']).map_or(0, |i| i + 1);
        let target = without_indexes(&before[start..]);
        words(&target).last().map(str::to_string)
    }
}

/// Whether `code` calls `name` itself (not `ungetc` for `getc`).
pub(super) fn calls(code: &str, name: &str) -> bool {
    code.match_indices(name).any(|(i, _)| {
        let is_word_start = code[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        let after = code[i + name.len()..].trim_start();
        is_word_start && after.starts_with('(')
    })
}

/// The names written in code: `(char *)conn->buf + 4` → `conn`, `buf`.
pub(super) fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| w.starts_with(|c: char| c.is_alphabetic() || c == '_'))
        .filter(|w| !NOT_NAMES.contains(w))
}

/// `text` with literals and comments blanked: each of their bytes becomes a
/// space (newlines stay). A `'` after a letter or digit is a C++14 digit
/// separator (`1'000`), not a quote.
fn blanked(text: &str) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Code,
        LineComment,
        BlockComment,
        Literal(char),
        Escaped(char),
    }
    let mut out = String::with_capacity(text.len());
    let mut state = State::Code;
    let mut chars = text.chars().peekable();
    let mut previous: Option<char> = None;
    while let Some(c) = chars.next() {
        let next = chars.peek().copied();
        let (keep, next_state, pair) = match state {
            State::Code => match (c, next) {
                ('/', Some('/')) => (false, State::LineComment, true),
                ('/', Some('*')) => (false, State::BlockComment, true),
                ('"', _) => (false, State::Literal('"'), false),
                ('\'', _) if !previous.is_some_and(|p| p.is_alphanumeric()) => {
                    (false, State::Literal('\''), false)
                }
                _ => (true, State::Code, false),
            },
            State::LineComment if c == '\n' => (true, State::Code, false),
            State::LineComment => (false, state, false),
            State::BlockComment if c == '*' && next == Some('/') => (false, State::Code, true),
            State::BlockComment => (false, state, false),
            State::Literal(_) if c == '\n' => (true, State::Code, false),
            State::Literal(quote) if c == '\\' => (false, State::Escaped(quote), false),
            State::Literal(quote) if c == quote => (false, State::Code, false),
            State::Literal(_) => (false, state, false),
            State::Escaped(quote) => (false, State::Literal(quote), false),
        };
        push_kept(&mut out, c, keep);
        if pair {
            if let Some(second) = chars.next() {
                push_kept(&mut out, second, false);
            }
        }
        previous = Some(c);
        state = next_state;
    }
    out
}

fn push_kept(out: &mut String, c: char, keep: bool) {
    if keep || c == '\n' {
        out.push(c);
    } else {
        out.extend(std::iter::repeat_n(' ', c.len_utf8()));
    }
}

fn skip_space(text: &str, at: usize) -> usize {
    text.get(at..)
        .map_or(at, |rest| at + (rest.len() - rest.trim_start().len()))
}

/// The offset of the `close` matching the `open` at `at`, within
/// `MAX_SCAN` bytes.
fn matching(text: &str, at: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in text.get(at..)?.char_indices() {
        if i > MAX_SCAN {
            return None;
        }
        if c == open {
            depth += 1;
        } else if c == close {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(at + i);
            }
        }
    }
    None
}

/// The offset of the `open` matching the `close` at `at`, looking back.
fn matching_back(text: &str, at: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in text.get(..=at)?.char_indices().rev() {
        if c == close {
            depth += 1;
        } else if c == open {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// `buf[n++]` → `buf`: an index isn't what is assigned.
fn without_indexes(text: &str) -> String {
    let mut depth = 0usize;
    text.chars()
        .filter(|c| match c {
            '[' => {
                depth += 1;
                false
            }
            ']' => {
                depth = depth.saturating_sub(1);
                false
            }
            _ => depth == 0,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_and_comments_are_blanked_byte_for_byte() {
        let source = "f(\"a(b\", 'x', 1'000) // é)\n/* ) */ g(é)";
        let code = Code::of(source);
        assert_eq!(code.text.len(), source.len());
        // Only the parentheses of `f(…)` and `g(…)` are left.
        assert_eq!(code.text.matches('(').count(), 2);
        assert_eq!(code.text.matches(')').count(), 2);
        assert!(!code.text.contains("a(b") && !code.text.contains('x'));
        assert!(code.text.contains("1'000)"));
        assert!(code.text.ends_with("g(é)"));
    }

    #[test]
    fn finds_an_argument_list_and_gives_up_on_an_unclosed_one() {
        let code = Code::of("x = handle(b, f(c), \"(\") + 1; open(a");
        let at = "x = handle".len();
        assert_eq!(code.arguments_at(at), Some("b, f(c),    "));
        assert_eq!(code.arguments_at(code.text.len() - 2), None);
        let template = Code::of("parse<int>(buf);");
        assert_eq!(template.arguments_at(5), Some("buf"));
    }

    #[test]
    fn a_call_runs_to_its_closing_parenthesis_across_lines() {
        let code = Code::of("memcpy(dst,\n    buf, 8);\nnext();");
        assert_eq!(code.call_at(0), "memcpy(dst,\n    buf, 8)");
    }

    #[test]
    fn finds_the_assigned_variable() {
        let case = |source: &str, call: &str| {
            let code = Code::of(source);
            code.assigned_at(source.find(call).expect("call"))
        };
        assert_eq!(case("buf[n++] = getchar();", "getchar"), Some("buf".into()));
        assert_eq!(
            case(
                "const char *h = (const char *)std::getenv(\"X\");",
                "getenv"
            ),
            Some("h".into())
        );
        assert_eq!(
            case("{ char *h =\n   getenv(\"X\");", "getenv"),
            Some("h".into())
        );
        assert_eq!(case("if ((c = getc(f)) != EOF)", "getc"), Some("c".into()));
        assert_eq!(case("if (x == getc(f))", "getc"), None);
        assert_eq!(case("use(getenv(\"X\"));", "getenv"), None);
    }

    #[test]
    fn a_call_must_be_to_the_name_itself() {
        assert!(calls("push(getc (f))", "getc"));
        assert!(!calls("push(ungetc(c, f))", "getc"));
        assert!(!calls("getc_count", "getc"));
    }
}

//! A doc comment's parts, from its Doxygen commands: the brief, the
//! description, each parameter, what it returns, and notes such as
//! warnings or "see also". A plain comment is all description, its first
//! paragraph the brief.

/// The parts of a doc comment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Doc {
    pub brief: String,
    /// Paragraphs.
    pub details: Vec<String>,
    pub params: Vec<Param>,
    /// What it returns, and each `@retval`.
    pub returns: Vec<String>,
    /// Other sections, e.g. ("Warning", "not thread-safe").
    pub notes: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    /// `in`, `out`, `in,out`, or `template` for a `@tparam`.
    pub direction: Option<String>,
    pub text: String,
}

/// Where the text being read goes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Brief,
    Details,
    Param(usize),
    Returns(usize),
    Note(usize),
}

/// Commands shown as a labeled note.
const NOTES: [(&str, &str); 17] = [
    ("note", "Note"),
    ("remark", "Note"),
    ("remarks", "Note"),
    ("attention", "Attention"),
    ("warning", "Warning"),
    ("bug", "Bug"),
    ("todo", "To do"),
    ("deprecated", "Deprecated"),
    ("pre", "Before"),
    ("post", "After"),
    ("invariant", "Invariant"),
    ("throws", "Throws"),
    ("throw", "Throws"),
    ("exception", "Throws"),
    ("see", "See also"),
    ("sa", "See also"),
    ("since", "Since"),
];

/// Commands about structure, not documentation: the rest of their line
/// isn't shown.
const SKIPPED: [&str; 16] = [
    "fn",
    "def",
    "file",
    "ingroup",
    "defgroup",
    "addtogroup",
    "weakgroup",
    "name",
    "struct",
    "class",
    "typedef",
    "var",
    "author",
    "version",
    "date",
    "copyright",
];

/// Commands inside a sentence, dropped with the word they mark kept:
/// `@p len`, `\c NULL`, `@ref parse`.
const INLINE: [&str; 9] = ["p", "c", "a", "b", "e", "em", "ref", "link", "endlink"];

/// The parts of the doc comment `text` (its markers already removed).
pub fn parse(text: &str) -> Doc {
    let mut doc = Doc::default();
    let mut part = Part::Details;
    let mut paragraph = String::new();
    let mut has_brief_command = false;
    for line in text.lines().flat_map(split_commands) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut doc, &part, &mut paragraph);
            if part != Part::Details {
                part = Part::Details;
            }
            continue;
        }
        match command_at(trimmed) {
            Some((command, rest)) => {
                flush(&mut doc, &part, &mut paragraph);
                has_brief_command |= matches!(command.as_str(), "brief" | "short");
                let (next, text) = start(&mut doc, &command, rest);
                part = next;
                append(&mut paragraph, &text);
            }
            None => append(&mut paragraph, trimmed),
        }
    }
    flush(&mut doc, &part, &mut paragraph);
    if !has_brief_command && doc.brief.is_empty() && !doc.details.is_empty() {
        doc.brief = doc.details.remove(0);
    }
    doc
}

/// `line` cut before each command that starts a part (`@param`,
/// `@return`, …) written after other text, as Doxygen reads them.
fn split_commands(line: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (at, _) in line.match_indices([' ', '\t']) {
        let next = &line[at + 1..];
        let is_part = command_at(next).is_some_and(|(command, _)| starts_part(&command));
        if is_part && at > start {
            parts.push(&line[start..at]);
            start = at + 1;
        }
    }
    parts.push(&line[start..]);
    parts
}

/// Whether `command` starts a part of its own, rather than marking a word.
fn starts_part(command: &str) -> bool {
    let name = command.split('[').next().unwrap_or(command);
    matches!(
        name,
        "brief"
            | "short"
            | "details"
            | "param"
            | "tparam"
            | "return"
            | "returns"
            | "result"
            | "retval"
    ) || NOTES.iter().any(|(note, _)| *note == name)
}

/// `@param name` at the start of `line`: the command and the rest.
fn command_at(line: &str) -> Option<(String, &str)> {
    let rest = line.strip_prefix('@').or_else(|| line.strip_prefix('\\'))?;
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '[' || c == ']' || c == ','))
        .unwrap_or(rest.len());
    let word = &rest[..end];
    let name = word.split('[').next().unwrap_or(word).to_ascii_lowercase();
    if name.is_empty() || INLINE.contains(&name.as_str()) {
        return None;
    }
    let direction = word.find('[').map(|i| &word[i..]);
    let command = match direction {
        Some(direction) => format!("{name}{direction}"),
        None => name,
    };
    Some((command, rest[end..].trim()))
}

/// Begins the part `command` starts: where its text goes, and the text
/// to start it with.
fn start(doc: &mut Doc, command: &str, rest: &str) -> (Part, String) {
    let name = command.split('[').next().unwrap_or(command);
    match name {
        "brief" | "short" => (Part::Brief, rest.to_string()),
        "details" | "par" | "paragraph" => (Part::Details, rest.to_string()),
        "param" | "tparam" => {
            let direction = match name {
                "tparam" => Some("template".to_string()),
                _ => command
                    .find('[')
                    .map(|i| command[i..].trim_matches(['[', ']']).to_string()),
            };
            let (param, text) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            doc.params.push(Param {
                name: param.to_string(),
                direction,
                text: String::new(),
            });
            (Part::Param(doc.params.len() - 1), text.trim().to_string())
        }
        "return" | "returns" | "result" => {
            doc.returns.push(String::new());
            (Part::Returns(doc.returns.len() - 1), rest.to_string())
        }
        "retval" => {
            let (value, text) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            doc.returns.push(String::new());
            let text = format!("{value}: {}", text.trim());
            (Part::Returns(doc.returns.len() - 1), text)
        }
        _ => match NOTES.iter().find(|(command, _)| *command == name) {
            Some((_, label)) => {
                doc.notes.push((label.to_string(), String::new()));
                (Part::Note(doc.notes.len() - 1), rest.to_string())
            }
            None if SKIPPED.contains(&name) => (Part::Details, String::new()),
            // Unknown: keep its text with the description.
            None => (Part::Details, rest.to_string()),
        },
    }
}

/// Adds `line` to `paragraph`, a space between.
fn append(paragraph: &mut String, line: &str) {
    let line = without_inline_commands(line);
    if line.is_empty() {
        return;
    }
    if !paragraph.is_empty() {
        paragraph.push(' ');
    }
    paragraph.push_str(&line);
}

/// Puts the paragraph read into `part`, and starts a new one.
fn flush(doc: &mut Doc, part: &Part, paragraph: &mut String) {
    let text = std::mem::take(paragraph);
    if text.is_empty() {
        return;
    }
    let add = |into: &mut String| {
        if !into.is_empty() {
            into.push(' ');
        }
        into.push_str(&text);
    };
    match *part {
        Part::Brief => add(&mut doc.brief),
        Part::Details => doc.details.push(text.clone()),
        Part::Param(i) => add(&mut doc.params[i].text),
        Part::Returns(i) => add(&mut doc.returns[i]),
        Part::Note(i) => add(&mut doc.notes[i].1),
    }
}

/// `line` without `@p`, `\c` and the like, the words they mark kept.
fn without_inline_commands(line: &str) -> String {
    line.split(' ')
        .filter(|word| {
            let command = word.strip_prefix('@').or_else(|| word.strip_prefix('\\'));
            !command.is_some_and(|c| INLINE.contains(&c))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_brief_params_returns_and_notes() {
        let doc = parse(
            "@brief Copies a field.\n\
             \n\
             Copies at most @p len bytes.\n\
             @param[out] dst  where to\n\
             @param[in] src what\n\
               continues here\n\
             @param len how many\n\
             @return the bytes copied, or\n\
             -1 on error\n\
             @retval 0 nothing to copy\n\
             @warning Not thread-safe.\n\
             @see parse_field",
        );
        assert_eq!(doc.brief, "Copies a field.");
        assert_eq!(doc.details, ["Copies at most len bytes."]);
        let params: Vec<(&str, Option<&str>, &str)> = doc
            .params
            .iter()
            .map(|p| (p.name.as_str(), p.direction.as_deref(), p.text.as_str()))
            .collect();
        assert_eq!(
            params,
            [
                ("dst", Some("out"), "where to"),
                ("src", Some("in"), "what continues here"),
                ("len", None, "how many"),
            ]
        );
        assert_eq!(
            doc.returns,
            ["the bytes copied, or -1 on error", "0: nothing to copy"]
        );
        assert_eq!(
            doc.notes,
            [
                ("Warning".to_string(), "Not thread-safe.".to_string()),
                ("See also".to_string(), "parse_field".to_string()),
            ]
        );
    }

    #[test]
    fn backslash_commands_and_a_plain_comment() {
        let doc = parse("\\brief Frees it.\n\\param p the buffer\n\\returns nothing");
        assert_eq!(doc.brief, "Frees it.");
        assert_eq!(doc.params[0].name, "p");
        assert_eq!(doc.returns, ["nothing"]);
        let plain = parse("Updates the window.\nIt grows.\n\nCalled after inflate.");
        assert_eq!(plain.brief, "Updates the window. It grows.");
        assert_eq!(plain.details, ["Called after inflate."]);
    }

    #[test]
    fn structural_commands_are_left_out() {
        let doc = parse("@fn int f(void)\n@ingroup io\nDoes f.\n@author someone");
        assert_eq!(doc.brief, "Does f.");
        assert!(doc.details.is_empty());
        assert!(doc.notes.is_empty());
    }

    #[test]
    fn a_tparam_and_an_email_address_in_text() {
        let doc = parse("Sends mail to x@y.org.\n@tparam T the type");
        assert_eq!(doc.brief, "Sends mail to x@y.org.");
        assert_eq!(doc.params[0].direction.as_deref(), Some("template"));
    }

    #[test]
    fn commands_after_other_text_on_a_line_start_their_part() {
        let doc = parse("@brief Parses a header. @param p text @return 0 \\n on success");
        assert_eq!(doc.brief, "Parses a header.");
        assert_eq!(doc.params[0].text, "text");
        assert_eq!(doc.returns, ["0 \\n on success"]);
    }

    #[test]
    fn empty_text_is_an_empty_doc() {
        assert_eq!(parse(""), Doc::default());
        let only_param = parse("@param x");
        assert_eq!(only_param.params[0].text, "");
        assert!(only_param.brief.is_empty());
    }
}

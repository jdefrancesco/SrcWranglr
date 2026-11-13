//! Reading C and C++ constants as the compiler would: the bytes of string
//! literals as written in the source, and the character constants in a
//! stretch of code (outside its comments and strings).

/// The longest raw-string delimiter C++ allows.
const MAX_RAW_DELIMITER: usize = 16;
/// A character constant longer than this many bytes isn't one.
const MAX_CHAR_BYTES: usize = 4;
/// An octal escape (`\101`) has at most this many digits.
const MAX_OCTAL_DIGITS: usize = 3;

/// The bytes of `text`, one or more adjacent narrow string literals as
/// written (`"a\n" "b"`, `u8"x"`, `R"(…)"`, `"x"_s`). `None` for anything
/// else: wide strings, text cut short, or an expression.
pub fn string_bytes(text: &str) -> Option<Vec<u8>> {
    let mut rest = text.trim();
    if rest.is_empty() {
        return None;
    }
    let mut bytes = Vec::new();
    while !rest.is_empty() {
        let (literal, after) = one_literal(rest)?;
        bytes.extend(literal);
        // A suffix (`"x"s`, `"x"_s`) isn't part of it; the prefix of a next
        // literal (`L"b"`) is that literal's.
        let word = after.len() - after.trim_start_matches(is_word_char).len();
        rest = if after[word..].starts_with('"') {
            after
        } else {
            after[word..].trim_start()
        };
    }
    Some(bytes)
}

/// The first literal of `text` and what follows it.
fn one_literal(text: &str) -> Option<(Vec<u8>, &str)> {
    let rest = text.strip_prefix("u8").unwrap_or(text);
    let (is_raw, rest) = match rest.strip_prefix('R') {
        Some(rest) => (true, rest),
        None => (false, rest),
    };
    let rest = rest.strip_prefix('"')?;
    if is_raw {
        let (content, after) = raw_content(rest)?;
        return Some((content.as_bytes().to_vec(), after));
    }
    let (content, after) = quoted(rest, '"').ok()?;
    Some((unescape(content)?, after))
}

/// A raw string's content after `R"`, and what follows its end.
fn raw_content(rest: &str) -> Option<(&str, &str)> {
    let open = rest.find('(').filter(|&at| at <= MAX_RAW_DELIMITER)?;
    let delimiter = &rest[..open];
    if delimiter.contains(|c: char| c.is_whitespace() || "\\)".contains(c)) {
        return None;
    }
    let closing = format!("){delimiter}\"");
    let body = &rest[open + 1..];
    let end = body.find(&closing)?;
    Some((&body[..end], &body[end + closing.len()..]))
}

/// The text up to the unescaped `close`, and what follows it. If the line
/// ends first, `Err` with what follows that line, so a scan of hostile
/// code never reads a line twice.
fn quoted(rest: &str, close: char) -> Result<(&str, &str), &str> {
    let mut chars = rest.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '\n' => return Err(&rest[at + 1..]),
            c if c == close => return Ok((&rest[..at], &rest[at + c.len_utf8()..])),
            _ => {}
        }
    }
    Err("")
}

/// `content` with its escapes (`\n`, `\x41`, `\101`, `é`) applied.
fn unescape(content: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut chars = content.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            push_char(&mut out, c);
            continue;
        }
        match chars.next()? {
            'n' => out.push(b'\n'),
            't' => out.push(b'\t'),
            'r' => out.push(b'\r'),
            'a' => out.push(0x07),
            'b' => out.push(0x08),
            'f' => out.push(0x0C),
            'v' => out.push(0x0B),
            'e' => out.push(0x1B),
            'x' => out.push(hex(&mut chars)?),
            'u' => push_char(&mut out, universal(&mut chars, 4)?),
            'U' => push_char(&mut out, universal(&mut chars, 8)?),
            digit @ '0'..='7' => out.push(octal(&mut chars, digit)?),
            // A backslash ending a line joins it to the next.
            '\n' => {}
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
            }
            // `\\`, `\"`, `\'`, `\?`, and unknown escapes: the character.
            other => push_char(&mut out, other),
        }
    }
    Some(out)
}

/// An octal escape's byte: `first` and up to two more octal digits.
fn octal(chars: &mut std::iter::Peekable<std::str::Chars>, first: char) -> Option<u8> {
    let mut value = first.to_digit(8)?;
    for _ in 0..MAX_OCTAL_DIGITS - 1 {
        let Some(digit) = chars.peek().and_then(|c| c.to_digit(8)) else {
            break;
        };
        chars.next();
        value = value * 8 + digit;
    }
    u8::try_from(value).ok()
}

/// A `\x` escape's byte: all the hex digits that follow (at least one),
/// as C reads them; `None` if they don't fit in a byte.
fn hex(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<u8> {
    let mut value: u32 = 0;
    let mut count = 0;
    while let Some(digit) = chars.peek().and_then(|c| c.to_digit(16)) {
        chars.next();
        value = value.checked_mul(16)?.checked_add(digit)?;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    u8::try_from(value).ok()
}

/// A `\u` or `\U` escape's character, from its `digits` hex digits.
fn universal(chars: &mut std::iter::Peekable<std::str::Chars>, digits: usize) -> Option<char> {
    let hex: String = (0..digits).map_while(|_| chars.next()).collect();
    if hex.len() != digits {
        return None;
    }
    char::from_u32(u32::from_str_radix(&hex, 16).ok()?)
}

fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buffer = [0; 4];
    out.extend(c.encode_utf8(&mut buffer).as_bytes());
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The narrow character constants in `code` (`':'`, `'\r'`), in order,
/// skipping comments, strings, wide constants (`L'x'`) and C++ digit
/// separators (`1'000`).
pub fn char_constants(code: &str) -> Vec<Vec<u8>> {
    let mut found = Vec::new();
    let mut rest = code;
    while let Some(at) = rest.find(['/', '"', '\'']) {
        let (before, from) = rest.split_at(at);
        rest = match from.as_bytes()[0] {
            b'/' if from.starts_with("//") => skip_past(from, "\n"),
            b'/' if from.starts_with("/*") => skip_past(&from[2..], "*/"),
            // A raw string that never ends runs to the end of the code.
            b'"' if is_raw_prefix(trailing_word(before)) => {
                raw_content(&from[1..]).map_or("", |(_, after)| after)
            }
            b'"' => match quoted(&from[1..], '"') {
                Ok((_, after)) | Err(after) => after,
            },
            b'\'' => char_constant(before, &from[1..], &mut found),
            _ => &from[1..],
        };
    }
    found
}

/// After a `'`: the constant it starts, if it is one, and what follows.
fn char_constant<'a>(before: &str, rest: &'a str, found: &mut Vec<Vec<u8>>) -> &'a str {
    match trailing_word(before) {
        // A digit separator, or the end of a name: not a constant.
        word if !word.is_empty() && !["L", "u", "U", "u8"].contains(&word) => rest,
        word => {
            let (content, after) = match quoted(rest, '\'') {
                Ok(found) => found,
                Err(after) => return after,
            };
            let bytes = unescape(content).filter(|b| (1..=MAX_CHAR_BYTES).contains(&b.len()));
            if let (Some(bytes), true) = (bytes, word.is_empty()) {
                found.push(bytes);
            }
            after
        }
    }
}

/// Whether `word`, just before a `"`, makes it a raw string.
fn is_raw_prefix(word: &str) -> bool {
    matches!(word, "R" | "LR" | "uR" | "UR" | "u8R")
}

/// The name or number `text` ends with (`L` in `x = L`).
fn trailing_word(text: &str) -> &str {
    let start = text
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word_char(*c))
        .last()
        .map_or(text.len(), |(at, _)| at);
    &text[start..]
}

/// What follows the first `end` in `text`, or nothing if there's none.
fn skip_past<'a>(text: &'a str, end: &str) -> &'a str {
    text.find(end).map_or("", |at| &text[at + end.len()..])
}

/// Whether `bytes` is a printf-style format (`%s`, `%-08.3lu`): text the
/// program writes out, not input it looks for. `%%` and a lone `%` aren't.
pub fn is_format(bytes: &[u8]) -> bool {
    let at = |i: usize| bytes.get(i).copied();
    let skip = |mut i: usize, set: &[u8]| {
        while at(i).is_some_and(|b| set.contains(&b)) {
            i += 1;
        }
        i
    };
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            i += 1;
            continue;
        }
        if at(i + 1) == Some(b'%') {
            i += 2;
            continue;
        }
        let mut j = skip(i + 1, b"-+ #0'");
        j = skip(j, b"0123456789*");
        if at(j) == Some(b'.') {
            j = skip(j + 1, b"0123456789*");
        }
        j = skip(j, b"hlLqjzt");
        if at(j).is_some_and(|b| b"diouxXeEfFgGaAcspn".contains(&b)) {
            return true;
        }
        i += 1;
    }
    false
}

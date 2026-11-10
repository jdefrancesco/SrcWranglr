//! Judging a library call by its arguments: a non-literal format string,
//! an unbounded `%s`, a copy sized by its own source, an allocation size
//! that can overflow. Also the reverse: arguments that make a call that is
//! dangerous in general safe enough here (a constant size, a bounded
//! format), which keeps the noise down.

use super::Severity;

/// What a call's arguments say about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing more to say: the function's general rule applies.
    Default,
    /// The arguments make it safe enough not to flag.
    Suppress,
    /// The arguments say more; flag it with this instead.
    Flag(Assessment),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assessment {
    /// Short name of the pattern, e.g. "non-literal format".
    pub label: &'static str,
    pub severity: Severity,
    pub cwe: u32,
    pub why: &'static str,
    pub fix: &'static str,
}

const NON_LITERAL_FORMAT: Assessment = Assessment {
    label: "non-literal format",
    severity: Severity::High,
    cwe: 134,
    why: "The format string isn't a literal. If it can come from input, conversions in it \
          (%s, %x, %n) read or write memory the call never received.",
    fix: "Pass a literal format and the text as an argument: printf(\"%s\", text).",
};
const UNBOUNDED_SPRINTF: Assessment = Assessment {
    label: "unbounded %s",
    severity: Severity::High,
    cwe: 120,
    why: "%s copies a string of any length into a buffer whose size the call doesn't know.",
    fix: "Use snprintf(dest, sizeof dest, ...), or a precision such as %.63s.",
};
const FIXED_WIDTH_SPRINTF: Assessment = Assessment {
    label: "fixed-width conversions",
    severity: Severity::Low,
    cwe: 120,
    why: "Every conversion has a bounded length, so this only overflows if the buffer is \
          smaller than the longest possible result.",
    fix: "Make sure the buffer fits the longest output, or use snprintf.",
};
const LONG_SPRINTF: Assessment = Assessment {
    label: "long conversions",
    severity: Severity::Medium,
    cwe: 120,
    why: "Floating-point (%f, %e, %g) and run-time widths (%*d) can produce hundreds of \
          characters, more than a small buffer holds.",
    fix: "Use snprintf(dest, sizeof dest, ...) and check its result.",
};
const BOUNDED_SCANF: Assessment = Assessment {
    label: "string widths",
    severity: Severity::Low,
    cwe: 120,
    why: "Every %s / %[ has a width, so this only overflows if a width is not less than its \
          buffer's size.",
    fix: "Check each width is at most the buffer size minus one.",
};
const CONSTANT_COPY: Assessment = Assessment {
    label: "constant size",
    severity: Severity::Low,
    cwe: 120,
    why: "Copies a fixed number of bytes: a bug only if the destination (or the source) is \
          smaller than that.",
    fix: "Use sizeof(dest), or check both buffers are at least that large.",
};
const CONSTANT_ALLOCA: Assessment = Assessment {
    label: "constant size",
    severity: Severity::Low,
    cwe: 770,
    why: "Allocates a fixed size on the stack: fine if small, a stack overflow if large.",
    fix: "Keep stack allocations small, or allocate on the heap.",
};
const UNBOUNDED_SCANF: Assessment = Assessment {
    label: "unbounded %s",
    severity: Severity::High,
    cwe: 120,
    why: "%s or %[ without a field width stores input of any length into the destination.",
    fix: "Give the conversion a width one less than the buffer, e.g. %63s for char buf[64].",
};
const SIZE_FROM_SOURCE: Assessment = Assessment {
    label: "size from the source",
    severity: Severity::High,
    cwe: 120,
    why: "The size comes from the source string, not the destination buffer: a long source \
          overflows the destination exactly as strcpy would.",
    fix: "Bound the size by the destination: min(strlen(src), sizeof dest - 1).",
};
const FIXED_STRING: Assessment = Assessment {
    label: "fixed string",
    severity: Severity::Low,
    cwe: 120,
    why: "Copies a string literal: only a bug if the destination is shorter than the literal.",
    fix: "Check the destination is at least sizeof(literal), or use snprintf.",
};
const CONSTANT_COMMAND: Assessment = Assessment {
    label: "constant command",
    severity: Severity::Low,
    cwe: 78,
    why: "Runs a fixed command, so nothing can be injected, but it still depends on PATH \
          and the shell.",
    fix: "Use an absolute path, or run the program with execv / posix_spawn.",
};
const VARIABLE_ALLOCA: Assessment = Assessment {
    label: "variable size",
    severity: Severity::High,
    cwe: 770,
    why: "Allocates a run-time size on the stack: a large value runs past the stack into \
          other memory, with no error to check.",
    fix: "Bound the size before the call, or allocate on the heap.",
};
const SIZE_OVERFLOW: Assessment = Assessment {
    label: "size can overflow",
    severity: Severity::Medium,
    cwe: 190,
    why: "The size is a product that can wrap around, allocating less than the code later \
          writes into it.",
    fix: "Use calloc(n, size), reallocarray, or check n <= SIZE_MAX / size first \
          (kcalloc or kmalloc_array in the Linux kernel).",
};
const CERTIFICATE_CHECK_OFF: Assessment = Assessment {
    label: "certificate check off",
    severity: Severity::High,
    cwe: 295,
    why: "Turns off checking the server's certificate, so anyone on the network path can \
          pose as the server.",
    fix: "Leave verification on; to trust a private CA, load it \
          (SSL_CTX_load_verify_locations, CURLOPT_CAINFO).",
};

/// printf-like functions and their format argument. `v` forms take the
/// format from a wrapper's caller, so a non-literal format is normal there.
const PRINTF_FORMAT_ARG: &[(&str, usize)] = &[
    ("printf", 0),
    ("wprintf", 0),
    ("fprintf", 1),
    ("fwprintf", 1),
    ("dprintf", 1),
    ("sprintf", 1),
    ("snprintf", 2),
    ("swprintf", 2),
    ("asprintf", 1),
    ("syslog", 1),
    ("err", 1),
    ("errx", 1),
    ("warn", 0),
    ("warnx", 0),
    ("vsprintf", 1),
    ("_tprintf", 0),
    ("_ftprintf", 1),
    ("_stprintf", 1),
    ("wsprintf", 1),
    ("wsprintfA", 1),
    ("wsprintfW", 1),
    ("_snprintf", 2),
    ("_snwprintf", 2),
    ("printk", 0),
    ("seq_printf", 1),
    ("scnprintf", 2),
    ("sysfs_emit", 1),
];
/// sprintf-like: write an unknown amount into a buffer of unknown size.
const UNSIZED_PRINTF: &[&str] = &[
    "sprintf",
    "vsprintf",
    "_stprintf",
    "wsprintf",
    "wsprintfA",
    "wsprintfW",
];
const SCANF_FORMAT_ARG: &[(&str, usize)] = &[
    ("scanf", 0),
    ("wscanf", 0),
    ("fscanf", 1),
    ("sscanf", 1),
    ("swscanf", 1),
    ("vscanf", 0),
    ("vfscanf", 1),
    ("vsscanf", 1),
    ("fwscanf", 1),
    ("vwscanf", 0),
    ("vfwscanf", 1),
    ("vswscanf", 1),
    ("_tscanf", 0),
    ("_ftscanf", 1),
    ("_stscanf", 1),
];
/// Byte copies taking (destination, source, size), where a constant size
/// is the common, harmless case.
const MEMORY_COPIES: &[&str] = &[
    "memcpy",
    "memmove",
    "wmemcpy",
    "wmemmove",
    "mempcpy",
    "__builtin_memcpy",
    "__builtin_memmove",
    "CopyMemory",
    "MoveMemory",
    "RtlCopyMemory",
    "RtlMoveMemory",
];
/// String copies taking (destination, source, count).
const COUNTED_STRING_COPIES: &[&str] = &[
    "strncpy", "wcsncpy", "_tcsncpy", "_mbsncpy", "stpncpy", "wcpncpy", "strncat", "wcsncat",
    "_tcsncat", "_mbsncat",
];
/// Unbounded string copies taking (destination, source).
const STRING_COPIES: &[&str] = &[
    "strcpy", "wcscpy", "stpcpy", "wcpcpy", "_tcscpy", "_mbscpy", "strcat", "wcscat", "_tcscat",
    "_mbscat", "lstrcpy", "lstrcpyA", "lstrcpyW", "lstrcat", "lstrcatA", "lstrcatW", "StrCpy",
    "StrCpyW", "StrCat", "StrCatW",
];
const SHELL_COMMANDS: &[&str] = &[
    "system", "_wsystem", "_tsystem", "popen", "_popen", "_wpopen", "_tpopen",
];
const STACK_ALLOCATIONS: &[&str] = &["alloca", "_alloca", "__builtin_alloca"];
/// Allocators and their size argument, a product of which can overflow.
const ALLOC_SIZE_ARG: &[(&str, usize)] = &[
    ("malloc", 0),
    ("valloc", 0),
    ("realloc", 1),
    ("aligned_alloc", 1),
    ("memalign", 1),
    ("HeapAlloc", 2),
    ("LocalAlloc", 1),
    ("GlobalAlloc", 1),
    ("kmalloc", 0),
    ("kzalloc", 0),
    ("kmalloc_node", 0),
    ("kzalloc_node", 0),
    ("vmalloc", 0),
    ("vzalloc", 0),
    ("kvmalloc", 0),
    ("kvzalloc", 0),
    ("krealloc", 1),
    ("devm_kmalloc", 1),
    ("devm_kzalloc", 1),
];
/// OpenSSL calls whose mode argument can turn off certificate checks.
const TLS_VERIFY_MODE: &[&str] = &["SSL_CTX_set_verify", "SSL_set_verify"];
/// libcurl options that, set to 0, turn off certificate checks.
const CURL_VERIFY_OPTIONS: &[&str] = &[
    "CURLOPT_SSL_VERIFYPEER",
    "CURLOPT_SSL_VERIFYHOST",
    "CURLOPT_PROXY_SSL_VERIFYPEER",
    "CURLOPT_PROXY_SSL_VERIFYHOST",
];

/// The `v` printf forms and their format argument. They format what a
/// wrapper's caller passed (so they aren't flagged), which makes a
/// function passing its own parameter to one a printf-like wrapper.
const VPRINTF_FORMAT_ARG: &[(&str, usize)] = &[
    ("vprintf", 0),
    ("vwprintf", 0),
    ("vfprintf", 1),
    ("vfwprintf", 1),
    ("vdprintf", 1),
    ("vsnprintf", 2),
    ("vswprintf", 2),
    ("vasprintf", 1),
    ("vsyslog", 1),
    ("verr", 1),
    ("verrx", 1),
    ("vwarn", 0),
    ("vwarnx", 0),
    ("_vsnprintf", 2),
    ("_vsnwprintf", 2),
    ("wvsprintf", 1),
    ("wvsprintfA", 1),
    ("wvsprintfW", 1),
    ("vprintk", 0),
    ("vscnprintf", 2),
];

/// Which argument of `name` is a format string, if it takes one (the `v`
/// forms and scanf-likes included).
pub fn format_argument(name: &str) -> Option<usize> {
    index_of(PRINTF_FORMAT_ARG, name)
        .or_else(|| index_of(VPRINTF_FORMAT_ARG, name))
        .or_else(|| index_of(SCANF_FORMAT_ARG, name))
}

/// Which argument of `name` is the size it copies, if it copies a counted
/// amount (memcpy's, strncpy's).
pub fn copy_size_argument(name: &str) -> Option<usize> {
    (MEMORY_COPIES.contains(&name) || COUNTED_STRING_COPIES.contains(&name)).then_some(2)
}

/// Which argument of `name` is the size it allocates, if it is an
/// allocator.
pub fn alloc_size_argument(name: &str) -> Option<usize> {
    index_of(ALLOC_SIZE_ARG, name)
}

/// Why a format that isn't a literal is risky.
pub fn non_literal_format() -> Assessment {
    NON_LITERAL_FORMAT
}

/// Why an allocation size that is a product or sum is risky.
pub fn size_overflow() -> Assessment {
    SIZE_OVERFLOW
}

/// Every name `assess` can say something about.
pub fn judged_names() -> impl Iterator<Item = &'static str> {
    let with_args = [SCANF_FORMAT_ARG, PRINTF_FORMAT_ARG, ALLOC_SIZE_ARG]
        .into_iter()
        .flatten()
        .map(|&(name, _)| name);
    let plain = [
        MEMORY_COPIES,
        COUNTED_STRING_COPIES,
        STRING_COPIES,
        SHELL_COMMANDS,
        STACK_ALLOCATIONS,
        TLS_VERIFY_MODE,
    ]
    .into_iter()
    .flatten()
    .copied();
    with_args
        .chain(plain)
        .chain(["realpath", "curl_easy_setopt"])
}

/// The verdict on a call to library function `name` with `args`.
pub fn assess(name: &str, args: &[String]) -> Verdict {
    let arg = |i: usize| args.get(i).map(|a| a.trim());
    if let Some(i) = index_of(SCANF_FORMAT_ARG, name) {
        return scanf_verdict(name, arg(i));
    }
    if let Some(i) = index_of(PRINTF_FORMAT_ARG, name) {
        return printf_verdict(name, arg(i));
    }
    if MEMORY_COPIES.contains(&name) || COUNTED_STRING_COPIES.contains(&name) {
        return copy_verdict(name, arg(1), arg(2));
    }
    if STRING_COPIES.contains(&name) {
        return flag_if(arg(1).is_some_and(is_string_literal), FIXED_STRING);
    }
    if SHELL_COMMANDS.contains(&name) {
        return flag_if(arg(0).is_some_and(is_string_literal), CONSTANT_COMMAND);
    }
    if STACK_ALLOCATIONS.contains(&name) {
        return match arg(0) {
            Some(size) if is_integer_literal(size) || size.starts_with("sizeof") => {
                Verdict::Flag(CONSTANT_ALLOCA)
            }
            Some(_) => Verdict::Flag(VARIABLE_ALLOCA),
            None => Verdict::Default,
        };
    }
    if let Some(i) = index_of(ALLOC_SIZE_ARG, name) {
        return flag_if(arg(i).is_some_and(can_overflow), SIZE_OVERFLOW);
    }
    if name == "realpath" {
        // With no buffer, realpath allocates one of the right size.
        let is_null = arg(1).is_some_and(|a| matches!(a, "NULL" | "nullptr" | "0"));
        return if is_null {
            Verdict::Suppress
        } else {
            Verdict::Default
        };
    }
    certificate_verdict(name, &arg)
}

/// Certificate checks turned off: `SSL_VERIFY_NONE`, or a libcurl verify
/// option set to 0. Other calls to these functions are fine.
fn certificate_verdict<'a>(name: &str, arg: &impl Fn(usize) -> Option<&'a str>) -> Verdict {
    let is_off = if TLS_VERIFY_MODE.contains(&name) {
        arg(1).is_some_and(|mode| mode.contains("SSL_VERIFY_NONE"))
    } else if name == "curl_easy_setopt" {
        let is_verify = arg(1).is_some_and(|option| CURL_VERIFY_OPTIONS.contains(&option));
        is_verify && arg(2).is_some_and(is_off)
    } else {
        return Verdict::Default;
    };
    match is_off {
        true => Verdict::Flag(CERTIFICATE_CHECK_OFF),
        false => Verdict::Suppress,
    }
}

fn index_of(table: &[(&str, usize)], name: &str) -> Option<usize> {
    table.iter().find(|(n, _)| *n == name).map(|(_, i)| *i)
}

fn flag_if(condition: bool, assessment: Assessment) -> Verdict {
    if condition {
        Verdict::Flag(assessment)
    } else {
        Verdict::Default
    }
}

fn is_forwarding(name: &str) -> bool {
    name.starts_with('v')
}

fn printf_verdict(name: &str, format: Option<&str>) -> Verdict {
    let Some(format) = format else {
        return Verdict::Default;
    };
    if !is_string_literal(format) {
        return flag_if(!is_forwarding(name), NON_LITERAL_FORMAT);
    }
    if !UNSIZED_PRINTF.contains(&name) {
        return Verdict::Default;
    }
    let all: Vec<Conversion> = conversions(format).collect();
    let is_unbounded = all.iter().any(|c| c.is_string() && !c.has_precision);
    // In printf, a leading `*` is a run-time width (in scanf it suppresses).
    let is_long = all
        .iter()
        .any(|c| c.has_star || c.is_suppressed || "fFeEgGaA".contains(c.kind));
    Verdict::Flag(if is_unbounded {
        UNBOUNDED_SPRINTF
    } else if is_long {
        LONG_SPRINTF
    } else {
        FIXED_WIDTH_SPRINTF
    })
}

fn scanf_verdict(name: &str, format: Option<&str>) -> Verdict {
    let Some(format) = format else {
        return Verdict::Default;
    };
    if !is_string_literal(format) {
        return flag_if(!is_forwarding(name), NON_LITERAL_FORMAT);
    }
    let strings: Vec<Conversion> = conversions(format)
        .filter(|c| (c.is_string() || c.kind == '[') && !c.is_suppressed)
        .collect();
    if strings.iter().any(|c| !c.has_width) {
        Verdict::Flag(UNBOUNDED_SCANF)
    } else if strings.is_empty() {
        Verdict::Suppress
    } else {
        Verdict::Flag(BOUNDED_SCANF)
    }
}

fn copy_verdict(name: &str, source: Option<&str>, size: Option<&str>) -> Verdict {
    let (Some(source), Some(size)) = (source, size) else {
        return Verdict::Default;
    };
    if is_size_of_source(size, source) {
        return Verdict::Flag(SIZE_FROM_SOURCE);
    }
    if MEMORY_COPIES.contains(&name) && is_integer_literal(size) {
        return Verdict::Flag(CONSTANT_COPY);
    }
    Verdict::Default
}

/// A flag turned off: `0`, `0L`, `(long)0`, `FALSE`.
fn is_off(value: &str) -> bool {
    let compact: String = value.split_whitespace().collect();
    let value = compact.strip_prefix("(long)").unwrap_or(&compact);
    let digits = value.trim_end_matches(['u', 'U', 'l', 'L']);
    matches!(digits, "0" | "0x0" | "0X0" | "FALSE" | "false")
}

/// `strlen(source)` or `strlen(source) + 1` (ignoring spaces).
fn is_size_of_source(size: &str, source: &str) -> bool {
    let compact = |text: &str| -> String { text.split_whitespace().collect() };
    let size = compact(size);
    let size = size.strip_suffix("+1").unwrap_or(&size);
    let source = compact(source);
    ["strlen", "wcslen"]
        .iter()
        .any(|len| size == format!("{len}({source})"))
}

/// A string literal: possibly prefixed (`L"…"`), wrapped in a translation
/// macro (`_("…")`), or joined to constant macros (`KERN_INFO "…"`,
/// `PACKAGE ": %s"`).
fn is_string_literal(arg: &str) -> bool {
    let mut text = arg.trim();
    for wrapper in ["_(", "N_(", "gettext(", "TEXT(", "_T("] {
        if let Some(inner) = text.strip_prefix(wrapper).and_then(|t| t.strip_suffix(')')) {
            text = inner.trim();
        }
    }
    let is_quoted = ["\"", "L\"", "u\"", "U\"", "u8\"", "R\""]
        .iter()
        .any(|prefix| text.starts_with(prefix));
    is_quoted || is_literal_with_macros(text)
}

/// Literals joined to upper-case macro names, such as `KERN_ERR PFX "…"`:
/// outside the quotes, only names in capitals and spaces, and at least one
/// literal.
fn is_literal_with_macros(text: &str) -> bool {
    let mut has_literal = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                has_literal = true;
                let mut is_escaped = false;
                let is_closed = chars.by_ref().any(|c| {
                    let is_end = c == '"' && !is_escaped;
                    is_escaped = c == '\\' && !is_escaped;
                    is_end
                });
                if !is_closed {
                    return false;
                }
            }
            c if c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' || c.is_whitespace() => {}
            _ => return false,
        }
    }
    has_literal
}

/// `16`, `0x10`, `64u`, `512UL`.
fn is_integer_literal(text: &str) -> bool {
    let text = text.trim().trim_end_matches(['u', 'U', 'l', 'L']);
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .map_or_else(
            || text.chars().all(|c| c.is_ascii_digit()),
            |hex| hex.chars().all(|c| c.is_ascii_hexdigit()),
        );
    !text.is_empty() && digits
}

/// A product (`a * b`) of at least two factors other than one, one of
/// which isn't a constant.
fn can_overflow(size: &str) -> bool {
    let factors = top_level_factors(size);
    if factors.iter().any(|f| f.is_empty()) {
        return false; // malformed
    }
    let factors: Vec<&str> = factors.into_iter().filter(|f| !is_one(f)).collect();
    let is_constant = |f: &&str| is_integer_literal(f) || f.starts_with("sizeof");
    factors.len() >= 2 && !factors.iter().all(is_constant)
}

/// `1` or the size of a one-byte type.
fn is_one(factor: &str) -> bool {
    let compact: String = factor.split_whitespace().collect();
    matches!(
        compact.as_str(),
        "1" | "sizeof(char)"
            | "sizeof(unsignedchar)"
            | "sizeof(signedchar)"
            | "sizeof(uint8_t)"
            | "sizeof(int8_t)"
    )
}

/// `size` (without redundant outer parentheses) split at multiplication
/// `*`s outside parentheses, each factor trimmed. A `*` that follows an
/// operator or starts the text is a dereference, not a multiplication.
fn top_level_factors(size: &str) -> Vec<&str> {
    let size = strip_outer_parens(size.trim());
    let mut factors = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    let mut previous: Option<char> = None;
    for (i, c) in size.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '*' if depth == 0 && previous.is_some_and(is_operand_end) => {
                factors.push(size[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
        if !c.is_whitespace() {
            previous = Some(c);
        }
    }
    factors.push(size[start..].trim());
    factors
}

/// A character an operand can end with (so a following `*` multiplies).
fn is_operand_end(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | ')' | ']')
}

/// `((a * b))` → `a * b`, but `(a) * (b)` stays as it is.
fn strip_outer_parens(text: &str) -> &str {
    let mut text = text;
    while let Some(inner) = text.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
        let mut depth = 0i32;
        let balanced = inner.chars().all(|c| {
            depth += match c {
                '(' => 1,
                ')' => -1,
                _ => 0,
            };
            depth >= 0
        });
        if !balanced {
            break;
        }
        text = inner.trim();
    }
    text
}

/// One `%…` conversion of a format string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Conversion {
    kind: char,
    has_width: bool,
    has_precision: bool,
    /// A width or precision given at run time (`%*d`, `%.*s`) in printf.
    has_star: bool,
    /// `%*s` in scanf: matched but not stored.
    is_suppressed: bool,
}

impl Conversion {
    /// `%s`, `%S` (wide), or `%ws` / `%ls` (length modifier then `s`).
    fn is_string(&self) -> bool {
        matches!(self.kind, 's' | 'S')
    }
}

/// The conversions in a format literal's text (`%%` is not one). Handles
/// positional `%1$s`, flags, `*` widths, and C99 / Microsoft length
/// modifiers (`ll`, `z`, `ws`, `I64d`).
fn conversions(format: &str) -> impl Iterator<Item = Conversion> + '_ {
    let mut chars = format.chars().peekable();
    std::iter::from_fn(move || loop {
        if chars.next()? != '%' {
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            continue;
        }
        let is_suppressed = chars.next_if_eq(&'*').is_some();
        // Digits are a width, unless a `$` makes them a position.
        let mut digits = 0;
        while chars.next_if(char::is_ascii_digit).is_some() {
            digits += 1;
        }
        let mut has_width = digits > 0;
        if digits > 0 && chars.next_if_eq(&'$').is_some() {
            has_width = false;
        }
        while chars.next_if(|c| "-+ #0'".contains(*c)).is_some() {}
        let mut has_star = false;
        while let Some(c) = chars.next_if(|c| c.is_ascii_digit() || *c == '*') {
            has_width = true;
            has_star |= c == '*';
        }
        let has_precision = chars.next_if_eq(&'.').is_some();
        while let Some(c) = chars.next_if(|c| c.is_ascii_digit() || *c == '*') {
            has_star |= c == '*';
        }
        while chars.next_if(|c| "hlLqjztwI".contains(*c)).is_some() {
            while chars.next_if(char::is_ascii_digit).is_some() {}
        }
        let kind = chars.next()?;
        return Some(Conversion {
            kind,
            has_width,
            has_precision,
            has_star: has_star && !is_suppressed,
            is_suppressed,
        });
    })
}

#[cfg(test)]
#[path = "heuristics_tests.rs"]
mod tests;

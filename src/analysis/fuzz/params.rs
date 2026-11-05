//! How a fuzz harness can fill each parameter of a function, judged from
//! the types as written: a byte buffer and its length, a string, a number,
//! or something it has to build by hand.

use crate::model::Param;

/// How a harness fills one parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fill {
    /// A pointer to bytes, filled with fuzz input. `element` is the pointee
    /// as written without qualifiers (`uint8_t`, `char`, `void`);
    /// `length` is the parameter that takes their count, if there is one.
    Bytes {
        element: String,
        is_const: bool,
        length: Option<usize>,
    },
    /// The count of bytes given to parameter `of`. `pointee` is the
    /// integer type when it is passed by address (an in/out length), which
    /// the harness starts at the buffer's size.
    Length { of: usize, pointee: Option<String> },
    /// A NUL-terminated `char` string.
    CString { is_const: bool },
    /// A `std::string` or `std::string_view`, by value or reference.
    Text,
    /// A `std::vector` of a byte type.
    ByteVector { element: String },
    /// A number or `bool`, passed by value or, for a pointer to one, by
    /// the address of a local.
    Number {
        ty: String,
        kind: NumberKind,
        by_address: bool,
    },
    /// Anything else: the harness has to build it by hand.
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberKind {
    Integer,
    Float,
    Bool,
}

impl Fill {
    /// Whether this takes fuzz input as bytes or text.
    pub fn is_input(&self) -> bool {
        matches!(
            self,
            Fill::Bytes { .. } | Fill::CString { .. } | Fill::Text | Fill::ByteVector { .. }
        )
    }
}

/// Words that qualify a type without changing what kind of value it is.
const QUALIFIERS: &[&str] = &[
    "const",
    "volatile",
    "restrict",
    "__restrict",
    "__restrict__",
    "struct",
    "union",
    "enum",
    "class",
    "typename",
    "register",
];
/// Pointees that make a pointer a byte buffer.
const BYTE_TYPES: &[&str] = &[
    "char",
    "signed char",
    "unsigned char",
    "uint8_t",
    "std::uint8_t",
    "int8_t",
    "std::int8_t",
    "u8",
    "__u8",
    "u_char",
    "uchar",
    "BYTE",
    "UCHAR",
    "CHAR",
    "guchar",
    "gchar",
    "guint8",
    "Byte",
    "Bytef",
    "std::byte",
    "void",
];
/// Pointees that make a lone pointer a NUL-terminated string.
const CHAR_TYPES: &[&str] = &["char", "CHAR", "gchar"];
const TEXT_TYPES: &[&str] = &["std::string", "string", "std::string_view", "string_view"];
/// Words that spell the built-in integer types in any combination.
const BUILTIN_INTEGER_WORDS: &[&str] = &["signed", "unsigned", "char", "short", "int", "long"];
const INTEGER_TYPES: &[&str] = &[
    "size_t",
    "ssize_t",
    "off_t",
    "off64_t",
    "ptrdiff_t",
    "intptr_t",
    "uintptr_t",
    "intmax_t",
    "uintmax_t",
    "socklen_t",
    "pid_t",
    "uid_t",
    "gid_t",
    "mode_t",
    "time_t",
    "wchar_t",
    "char16_t",
    "char32_t",
    "int8_t",
    "int16_t",
    "int32_t",
    "int64_t",
    "uint8_t",
    "uint16_t",
    "uint32_t",
    "uint64_t",
    "u8",
    "u16",
    "u32",
    "u64",
    "s8",
    "s16",
    "s32",
    "s64",
    "__u8",
    "__u16",
    "__u32",
    "__u64",
    "__s8",
    "__s16",
    "__s32",
    "__s64",
    "BYTE",
    "WORD",
    "DWORD",
    "QWORD",
    "DWORD64",
    "UINT",
    "INT",
    "ULONG",
    "LONG",
    "USHORT",
    "SHORT",
    "UCHAR",
    "CHAR",
    "SIZE_T",
    "BOOL",
    "guint",
    "gint",
    "gsize",
    "gssize",
    "guint8",
    "guint16",
    "guint32",
    "guint64",
    "gint8",
    "gint16",
    "gint32",
    "gint64",
    "gboolean",
    "uInt",
    "uIntf",
    "uLongf",
    "z_off_t",
    "z_off64_t",
    "z_crc_t",
    "uLong",
    "z_size_t",
];
const FLOAT_TYPES: &[&str] = &["float", "double", "long double", "gfloat", "gdouble"];
const BOOL_TYPES: &[&str] = &["bool", "_Bool"];
/// Name words that mark a parameter as a file name.
const PATH_WORDS: &[&str] = &["path", "file", "filename", "fname", "filepath", "pathname"];
/// Name words that mark a string as something that steers the function
/// (a format, mode or version), not data for it to work on.
const CONTROL_WORDS: &[&str] = &[
    "fmt",
    "format",
    "mode",
    "version",
    "why",
    "label",
    "title",
    "prefix",
    "suffix",
    "sep",
    "separator",
    "delim",
    "delimiter",
    "prog",
    "progname",
];
/// Name words that make a lone `void *` a buffer rather than an opaque
/// context or user-data pointer.
const BUFFER_WORDS: &[&str] = &[
    "buf", "buffer", "data", "src", "source", "in", "input", "payload", "bytes", "blob", "raw",
    "packet", "pkt", "msg", "message", "mem", "chunk",
];
/// Integer types that only ever hold sizes.
const SIZE_TYPES: &[&str] = &[
    "size_t",
    "std::size_t",
    "ssize_t",
    "socklen_t",
    "SIZE_T",
    "gsize",
    "gssize",
    "z_size_t",
    "size_type",
];

/// A type as written, taken apart.
struct Shape {
    /// The type without qualifiers, pointers or references: `uint8_t`.
    base: String,
    /// `*`s, counting an array as one.
    pointers: usize,
    is_reference: bool,
    /// `const` before the first `*` (or anywhere, if there is none).
    is_pointee_const: bool,
}

/// How to fill each of `params`, in order.
pub fn plan(params: &[Param]) -> Vec<Fill> {
    let fills: Vec<Fill> = params.iter().map(|p| classify(&p.type_text)).collect();
    let pairs = length_pairs(params, &fills);
    fills
        .into_iter()
        .enumerate()
        .map(|(i, fill)| {
            if let Some(&(buffer, _)) = pairs.iter().find(|(_, length)| *length == i) {
                let pointee = match fill {
                    Fill::Number {
                        ty,
                        by_address: true,
                        ..
                    } => Some(ty),
                    _ => None,
                };
                return Fill::Length {
                    of: buffer,
                    pointee,
                };
            }
            let length = pairs
                .iter()
                .find(|(buffer, _)| *buffer == i)
                .map(|(_, length)| *length);
            let is_buffer_name = params[i]
                .name
                .as_deref()
                .is_some_and(|n| words(n).iter().any(|w| BUFFER_WORDS.contains(&w.as_str())));
            match (fill, length) {
                (Fill::Bytes { element, .. }, None) if element == "void" && !is_buffer_name => {
                    Fill::Manual
                }
                (Fill::CString { is_const }, Some(length)) => Fill::Bytes {
                    element: "char".into(),
                    is_const,
                    length: Some(length),
                },
                (
                    Fill::Bytes {
                        element, is_const, ..
                    },
                    length,
                ) => Fill::Bytes {
                    element,
                    is_const,
                    length,
                },
                (fill, _) => fill,
            }
        })
        .collect()
}

/// Whether a parameter's name says it is a file name rather than content.
pub fn is_path_name(name: &str) -> bool {
    words(name).iter().any(|w| PATH_WORDS.contains(&w.as_str()))
}

/// Whether a parameter's name says it is a control string (a format, mode
/// or version) rather than data.
pub fn is_control_name(name: &str) -> bool {
    words(name)
        .iter()
        .any(|w| CONTROL_WORDS.contains(&w.as_str()))
}

/// A name's words, lowercased: `parseHTTPHeader` → `parse`, `http`,
/// `header`; `read_chunk` → `read`, `chunk`.
pub fn words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut found = Vec::new();
    let mut word = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            push_word(&mut found, &mut word);
            continue;
        }
        let previous = i.checked_sub(1).map(|p| chars[p]);
        let next = chars.get(i + 1);
        let starts_word = c.is_uppercase()
            && previous.is_some_and(|p| {
                p.is_lowercase()
                    || p.is_ascii_digit()
                    || (p.is_uppercase() && next.is_some_and(|n| n.is_lowercase()))
            });
        if starts_word {
            push_word(&mut found, &mut word);
        }
        word.extend(c.to_lowercase());
    }
    push_word(&mut found, &mut word);
    found
}

fn push_word(found: &mut Vec<String>, word: &mut String) {
    if !word.is_empty() {
        found.push(std::mem::take(word));
    }
}

/// How to fill one parameter on its own (lengths are paired later).
fn classify(type_text: &str) -> Fill {
    let Some(shape) = shape(type_text) else {
        return Fill::Manual;
    };
    let base = shape.base.as_str();
    match (shape.pointers, shape.is_reference) {
        (0, _) => value_fill(base),
        (1, false) if CHAR_TYPES.contains(&base) => Fill::CString {
            is_const: shape.is_pointee_const,
        },
        (1, false) if BYTE_TYPES.contains(&base) => Fill::Bytes {
            element: shape.base,
            is_const: shape.is_pointee_const,
            length: None,
        },
        (1, false) => match value_fill(base) {
            Fill::Number { ty, kind, .. } if !shape.is_pointee_const => Fill::Number {
                ty,
                kind,
                by_address: true,
            },
            _ => Fill::Manual,
        },
        _ => Fill::Manual,
    }
}

/// A parameter passed by value or reference.
fn value_fill(base: &str) -> Fill {
    let number = |kind| Fill::Number {
        ty: base.to_string(),
        kind,
        by_address: false,
    };
    let is_builtin_integer =
        !base.is_empty() && base.split(' ').all(|w| BUILTIN_INTEGER_WORDS.contains(&w));
    if TEXT_TYPES.contains(&base) {
        Fill::Text
    } else if let Some(element) = vector_element(base) {
        Fill::ByteVector { element }
    } else if BOOL_TYPES.contains(&base) {
        number(NumberKind::Bool)
    } else if FLOAT_TYPES.contains(&base) {
        number(NumberKind::Float)
    } else if is_builtin_integer || INTEGER_TYPES.contains(&base.trim_start_matches("std::")) {
        number(NumberKind::Integer)
    } else {
        Fill::Manual
    }
}

/// `std::vector<uint8_t>` → `uint8_t`, for vectors of a byte type.
fn vector_element(base: &str) -> Option<String> {
    let inner = base
        .strip_prefix("std::vector<")
        .or_else(|| base.strip_prefix("vector<"))?
        .strip_suffix('>')?
        .trim();
    (inner != "void" && BYTE_TYPES.contains(&inner)).then(|| inner.to_string())
}

/// Takes `type_text` apart, or `None` if it's empty or a function pointer.
fn shape(type_text: &str) -> Option<Shape> {
    if type_text.is_empty() || type_text.contains('(') {
        return None;
    }
    let tokens = tokens(type_text);
    let first_star = tokens.iter().position(|t| t == "*").unwrap_or(tokens.len());
    let mut in_brackets = false;
    let mut base_words = Vec::new();
    for token in &tokens {
        match token.as_str() {
            "[" => in_brackets = true,
            "]" => in_brackets = false,
            "*" | "&" => {}
            word if !in_brackets && !QUALIFIERS.contains(&word) => base_words.push(word),
            _ => {}
        }
    }

    Some(Shape {
        base: base_words.join(" "),
        // Each array dimension decays like one more pointer level.
        pointers: tokens.iter().filter(|t| *t == "*" || *t == "[").count(),
        is_reference: tokens.iter().any(|t| t == "&"),
        is_pointee_const: tokens[..first_star].iter().any(|t| t == "const"),
    })
}

/// Words and `*`, `&`, `[`, `]` outside template arguments; whitespace
/// inside them is dropped (`std::vector< char >` → `std::vector<char>`).
fn tokens(type_text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut word = String::new();
    let mut depth = 0usize;
    for c in type_text.chars() {
        match c {
            '<' => {
                depth += 1;
                word.push(c);
            }
            '>' => {
                depth = depth.saturating_sub(1);
                word.push(c);
            }
            '*' | '&' | '[' | ']' if depth == 0 => {
                push_word(&mut found, &mut word);
                found.push(c.to_string());
            }
            c if c.is_whitespace() => {
                if depth == 0 {
                    push_word(&mut found, &mut word);
                }
            }
            c => word.push(c),
        }
    }
    push_word(&mut found, &mut word);
    found
}

/// `(buffer, length)` index pairs: each buffer takes the length right
/// after it, or else one right before it. A length is an integer named
/// like one (`len`, `size`, `n`, …) or of a size type (`size_t`).
fn length_pairs(params: &[Param], fills: &[Fill]) -> Vec<(usize, usize)> {
    let name = |i: usize| params[i].name.as_deref();
    let is_buffer = |i: usize| {
        matches!(fills[i], Fill::Bytes { .. } | Fill::CString { .. })
            && !name(i).is_some_and(is_control_name)
    };
    // A length passed by reference would have to be a variable, not the
    // buffer's size.
    let is_length = |i: usize| match &fills[i] {
        _ if params[i].type_text.contains('&') => false,
        Fill::Number {
            ty,
            kind: NumberKind::Integer,
            by_address,
        } => {
            let is_named = name(i).is_some_and(|n| words(n).iter().any(|w| is_length_word(w)));
            // By address, it must say it's a length: `size_t *written` is an
            // out count, not the buffer's size.
            let is_size_type = !by_address && SIZE_TYPES.contains(&ty.as_str());
            !ty.contains("char") && (is_named || is_size_type)
        }
        _ => false,
    };
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let is_taken =
        |pairs: &[(usize, usize)], i: usize| pairs.iter().any(|&(b, l)| b == i || l == i);
    for i in (0..fills.len()).filter(|&i| is_buffer(i)) {
        let after = i + 1;
        if after < fills.len() && is_length(after) && !is_taken(&pairs, after) {
            pairs.push((i, after));
        }
    }
    for i in (1..fills.len()).filter(|&i| is_buffer(i)) {
        let before = i - 1;
        let is_free = !is_taken(&pairs, i) && !is_taken(&pairs, before);
        if is_free && is_length(before) {
            pairs.push((i, before));
        }
    }
    pairs
}

fn is_length_word(word: &str) -> bool {
    matches!(word, "n" | "num" | "cnt" | "nb")
        || ["len", "length", "size", "count", "sz", "bytes"]
            .iter()
            .any(|suffix| word.ends_with(suffix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(list: &[(&str, &str)]) -> Vec<Param> {
        list.iter()
            .map(|(ty, name)| Param {
                type_text: (*ty).into(),
                name: (!name.is_empty()).then(|| (*name).to_string()),
            })
            .collect()
    }

    fn bytes(element: &str, is_const: bool, length: Option<usize>) -> Fill {
        Fill::Bytes {
            element: element.into(),
            is_const,
            length,
        }
    }

    fn length(of: usize) -> Fill {
        Fill::Length { of, pointee: None }
    }

    fn integer(ty: &str) -> Fill {
        Fill::Number {
            ty: ty.into(),
            kind: NumberKind::Integer,
            by_address: false,
        }
    }

    #[test]
    fn pairs_a_buffer_with_the_length_after_it() {
        let fills = plan(&params(&[("const uint8_t *", "buf"), ("size_t", "len")]));
        assert_eq!(fills, [bytes("uint8_t", true, Some(1)), length(0)]);
    }

    #[test]
    fn pairs_a_length_written_before_its_buffer() {
        let fills = plan(&params(&[("int", "len"), ("char *", "buf")]));
        assert_eq!(fills, [length(1), bytes("char", false, Some(0))]);
    }

    #[test]
    fn does_not_pair_a_number_before_a_buffer_unless_named_like_a_length() {
        let fills = plan(&params(&[("int", "flags"), ("const unsigned char *", "p")]));
        assert_eq!(fills, [integer("int"), bytes("unsigned char", true, None)]);
    }

    #[test]
    fn a_char_pointer_alone_is_a_c_string() {
        assert_eq!(
            plan(&params(&[("const char *", "s")])),
            [Fill::CString { is_const: true }]
        );
        assert_eq!(
            plan(&params(&[("char *", "s")])),
            [Fill::CString { is_const: false }]
        );
    }

    #[test]
    fn a_void_pointer_and_length_is_a_buffer() {
        let fills = plan(&params(&[("const void *", "p"), ("unsigned int", "n")]));
        assert_eq!(fills, [bytes("void", true, Some(1)), length(0)]);
    }

    #[test]
    fn const_after_the_star_leaves_the_bytes_writable() {
        let fills = plan(&params(&[("char *const", "p"), ("size_t", "n")]));
        assert_eq!(fills[0], bytes("char", false, Some(1)));
    }

    #[test]
    fn reads_cpp_strings_and_byte_vectors() {
        let fills = plan(&params(&[
            ("const std::string &", "a"),
            ("std::string_view", "b"),
            ("std::vector<uint8_t>", "c"),
            ("const std::vector<char> &", "d"),
        ]));
        assert_eq!(
            fills,
            [
                Fill::Text,
                Fill::Text,
                Fill::ByteVector {
                    element: "uint8_t".into()
                },
                Fill::ByteVector {
                    element: "char".into()
                },
            ]
        );
    }

    #[test]
    fn reads_numbers_and_pointers_to_them() {
        let fills = plan(&params(&[
            ("unsigned long", "a"),
            ("const uint32_t", "b"),
            ("double", "c"),
            ("bool", "d"),
            ("size_t *", "consumed"),
        ]));
        assert_eq!(
            fills,
            [
                integer("unsigned long"),
                integer("uint32_t"),
                Fill::Number {
                    ty: "double".into(),
                    kind: NumberKind::Float,
                    by_address: false
                },
                Fill::Number {
                    ty: "bool".into(),
                    kind: NumberKind::Bool,
                    by_address: false
                },
                Fill::Number {
                    ty: "size_t".into(),
                    kind: NumberKind::Integer,
                    by_address: true
                },
            ]
        );
    }

    #[test]
    fn everything_else_is_built_by_hand() {
        let fills = plan(&params(&[
            ("struct ctx *", "ctx"),
            ("FILE *", "f"),
            ("void (*)(int)", "cb"),
            ("char **", "argv"),
            ("", "unknown"),
            ("Parser &", "p"),
        ]));
        assert!(fills.iter().all(|f| *f == Fill::Manual), "{fills:?}");
    }

    #[test]
    fn a_char_counts_as_a_number_but_never_as_a_length() {
        let fills = plan(&params(&[("const char *", "s"), ("char", "sep")]));
        assert_eq!(fills, [Fill::CString { is_const: true }, integer("char")]);
    }

    #[test]
    fn each_length_pairs_with_one_buffer() {
        let fills = plan(&params(&[
            ("const uint8_t *", "a"),
            ("size_t", "a_len"),
            ("const uint8_t *", "b"),
            ("size_t", "b_len"),
        ]));
        assert_eq!(
            fills,
            [
                bytes("uint8_t", true, Some(1)),
                length(0),
                bytes("uint8_t", true, Some(3)),
                length(2),
            ]
        );
    }

    #[test]
    fn recognizes_file_name_parameters() {
        for name in [
            "path",
            "filename",
            "file_name",
            "inPath",
            "fname",
            "src_file",
        ] {
            assert!(is_path_name(name), "{name}");
        }
        for name in ["buf", "profile", "len", "data"] {
            assert!(!is_path_name(name), "{name}");
        }
    }

    #[test]
    fn a_length_must_be_named_or_typed_like_one() {
        let fills = plan(&params(&[("const char *", "format"), ("int", "a1")]));
        assert_eq!(fills, [Fill::CString { is_const: true }, integer("int")]);
        let fills = plan(&params(&[("char *", "outname"), ("int", "test")]));
        assert_eq!(fills, [Fill::CString { is_const: false }, integer("int")]);
        let fills = plan(&params(&[("const uint8_t *", "p"), ("size_t", "x")]));
        assert_eq!(fills, [bytes("uint8_t", true, Some(1)), length(0)]);
    }

    #[test]
    fn control_strings_never_pair_with_a_length() {
        let fills = plan(&params(&[
            ("const char *", "version"),
            ("int", "stream_size"),
        ]));
        assert_eq!(fills, [Fill::CString { is_const: true }, integer("int")]);
    }

    #[test]
    fn recognizes_control_string_names() {
        for name in ["fmt", "format", "mode", "version", "open_mode", "delim"] {
            assert!(is_control_name(name), "{name}");
        }
        for name in ["msg", "buf", "formatted_text", "data"] {
            assert!(!is_control_name(name), "{name}");
        }
    }

    #[test]
    fn reads_zlib_integer_types() {
        assert_eq!(plan(&params(&[("uLongf *", "destLen")]))[0], {
            Fill::Number {
                ty: "uLongf".into(),
                kind: NumberKind::Integer,
                by_address: true,
            }
        });
    }

    #[test]
    fn multi_dimensional_arrays_are_built_by_hand() {
        let fills = plan(&params(&[("int [3][3]", "m"), ("char [4][8]", "names")]));
        assert_eq!(fills, [Fill::Manual, Fill::Manual]);
    }

    #[test]
    fn a_length_passed_by_reference_is_not_paired() {
        let fills = plan(&params(&[("const uint8_t *", "p"), ("size_t &", "n")]));
        assert_eq!(fills, [bytes("uint8_t", true, None), integer("size_t")]);
    }

    #[test]
    fn a_lone_void_pointer_is_a_buffer_only_if_named_like_one() {
        assert_eq!(plan(&params(&[("void *", "ctx")])), [Fill::Manual]);
        assert_eq!(
            plan(&params(&[("const void *", "buf")])),
            [bytes("void", true, None)]
        );
    }

    #[test]
    fn a_length_passed_by_address_is_paired_when_named_like_one() {
        let fills = plan(&params(&[
            ("Bytef *", "dest"),
            ("uLongf *", "destLen"),
            ("const Bytef *", "source"),
            ("uLong", "sourceLen"),
        ]));
        assert_eq!(
            fills,
            [
                bytes("Bytef", false, Some(1)),
                Fill::Length {
                    of: 0,
                    pointee: Some("uLongf".into())
                },
                bytes("Bytef", true, Some(3)),
                length(2),
            ]
        );
        let fills = plan(&params(&[("char *", "out"), ("size_t *", "written")]));
        assert_eq!(
            fills[1],
            Fill::Number {
                ty: "size_t".into(),
                kind: NumberKind::Integer,
                by_address: true
            }
        );
    }

    #[test]
    fn input_fills_are_the_buffers_and_strings() {
        assert!(bytes("char", true, None).is_input());
        assert!(Fill::Text.is_input());
        assert!(!integer("int").is_input());
        assert!(!Fill::Manual.is_input());
        assert!(!length(0).is_input());
    }
}

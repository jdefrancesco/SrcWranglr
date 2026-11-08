//! Finding the comment that documents a declaration.

use super::*;

/// The comment above the first line containing `needle`.
fn above(source: &str, needle: &str) -> Option<DocComment> {
    let index = source
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no {needle}"));
    comment_above(source, u32::try_from(index + 1).expect("line"))
}

#[test]
fn reads_a_doxygen_block_above_a_function() {
    let source = "#include <x.h>\n\
\n\
/**\n\
 * Copies a field.\n\
 *\n\
 * @param dst where to\n\
 */\n\
int copy_field(char *dst);\n";
    let doc = above(source, "copy_field").expect("a doc");
    assert!(doc.is_doxygen);
    assert_eq!(doc.text, "Copies a field.\n\n@param dst where to");
    assert_eq!(doc.line, 3);
}

#[test]
fn skips_the_return_type_and_attributes_on_lines_above_the_name() {
    let source = "/*! Parses one header. */\n\
static int\n\
__attribute__((nonnull))\n\
parse_header(const char *p)\n\
{\n";
    let doc = above(source, "parse_header").expect("a doc");
    assert_eq!(doc.text, "Parses one header.");
    assert!(doc.is_doxygen);
}

#[test]
fn reads_a_run_of_line_comments_and_allows_one_blank_line() {
    let source = "int a;\n\
/// Frees the buffer.\n\
/// Safe to call twice.\n\
\n\
void release(void *p);\n";
    let doc = above(source, "release").expect("a doc");
    assert_eq!(doc.text, "Frees the buffer.\nSafe to call twice.");
    assert!(doc.is_doxygen);
    let plain = "// helper for the parser\nint helper(void);\n";
    let doc = above(plain, "int helper").expect("a doc");
    assert_eq!(doc.text, "helper for the parser");
    assert!(!doc.is_doxygen);
}

#[test]
fn a_plain_block_comment_counts_but_not_a_license_header() {
    let plain = "/* Update the window with the last output. */\nlocal int updatewindow(void) {\n";
    let doc = above(plain, "updatewindow").expect("a doc");
    assert_eq!(doc.text, "Update the window with the last output.");
    assert!(!doc.is_doxygen);
    let license = "/* Copyright (C) 1995 Jean-loup Gailly\n * see zlib.h */\n\nint first(void);\n";
    assert_eq!(above(license, "first"), None);
}

#[test]
fn looks_past_a_separator_line_to_the_comment_above_it() {
    let source = "/* Inflate the stream. */\n\
/* ========================================================================= */\n\
int inflate(void) {\n";
    let doc = above(source, "int inflate").expect("a doc");
    assert_eq!(doc.text, "Inflate the stream.");
    let bare = "/* ===================================================== */\nint f(void);\n";
    assert_eq!(above(bare, "int f"), None);
}

#[test]
fn nothing_when_code_or_a_trailing_comment_is_above() {
    assert_eq!(above("int x = 1;\nint f(void);\n", "int f"), None);
    let code_with_comment = "/* about x */\nint x; // counter\nint f(void);\n";
    assert_eq!(above(code_with_comment, "int f"), None);
    assert_eq!(above("int x; /* about x */\nint f(void);\n", "int f"), None);
    assert_eq!(above("}\n\n\nint f(void);\n", "int f"), None);
    assert_eq!(above("int f(void);\n", "int f"), None);
    assert_eq!(comment_above("int f(void);\n", 0), None);
    assert_eq!(comment_above("int f(void);\n", 99), None);
}

#[test]
fn a_huge_comment_is_cut_short() {
    let body = "x".repeat(40_000);
    let source = format!("/** {body} */\nint f(void);\n");
    let doc = above(&source, "int f").expect("a doc");
    assert!(doc.text.len() <= MAX_TEXT_BYTES + '…'.len_utf8());
    assert!(doc.text.ends_with('…'));
}

#[test]
fn star_decorated_lines_lose_their_stars() {
    let source = "/**\n\
 ** Weird stars.\n\
 *  Indented text.\n\
 */\n\
void f(void);\n";
    let doc = above(source, "void f").expect("a doc");
    assert_eq!(doc.text, "* Weird stars.\n Indented text.");
}

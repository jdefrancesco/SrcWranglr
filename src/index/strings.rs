//! String literals written in the source: collected from each file's
//! syntax tree, then placed in the function they are in.
//!
//! An `#include` path and the `"C"` of `extern "C"` aren't strings the code
//! uses, so they are left out. Adjacent literals (`"a" "b"`) are one string.
//! Strings in a macro's body count too (`#define VERSION "1.2"`). A
//! user-defined literal (`"abc"_s`) is listed without its suffix.

use std::collections::HashMap;
use std::ops::Range;

use tree_sitter::Node;

use super::macro_strings;
use super::resolve::SourceFile;
use crate::model::{FunctionMetrics, Span, StringLiteral};

/// Strings kept per file: a generated file of literals (a table, an
/// embedded blob) shouldn't take over the index.
pub const MAX_STRINGS_PER_FILE: usize = 5_000;
/// Strings kept in all, so many files of them can't either.
pub const MAX_STRINGS_TOTAL: usize = 200_000;
/// A string longer than this, in characters, is cut short (with `…`).
pub const MAX_STRING_CHARS: usize = 512;

/// One string literal in a file, before it is placed in a function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringFact {
    /// As written: prefix, quotes and escapes included. Adjacent literals
    /// are joined by a space.
    pub text: String,
    /// Where it starts; to the end of that line if it goes on.
    pub span: Span,
}

/// A file's strings, and whether there were more than were kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileStrings {
    pub strings: Vec<StringFact>,
    pub is_cut_short: bool,
}

/// The strings written in the tree at `root` (of `src`), in file order;
/// at most `limit` of them.
pub fn collect(root: Node, src: &str, limit: usize) -> FileStrings {
    let mut strings = Vec::new();
    // A cursor walk: no recursion to overflow, no list per node.
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        let (found, should_descend) = match node.kind() {
            "string_literal" | "raw_string_literal" | "concatenated_string" => {
                (literal_fact(node, src).into_iter().collect(), false)
            }
            "preproc_arg" => (macro_facts(node, src), false),
            _ => (Vec::new(), true),
        };
        for fact in found {
            if strings.len() >= limit {
                return FileStrings {
                    strings,
                    is_cut_short: true,
                };
            }
            strings.push(fact);
        }
        if should_descend && cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return FileStrings {
                    strings,
                    is_cut_short: false,
                };
            }
        }
    }
}

/// The literal at `node`, unless it isn't a string the code uses.
fn literal_fact(node: Node, src: &str) -> Option<StringFact> {
    if !is_used_as_a_string(node) {
        return None;
    }
    let text = if node.kind() == "concatenated_string" {
        let mut cursor = node.walk();
        let parts: Vec<&str> = node
            .named_children(&mut cursor)
            .map(|part| node_text(part, src))
            .collect();
        joined_cut_short(&parts)
    } else {
        joined_cut_short(&[node_text(node, src)])
    };
    let start = node.start_position();
    let first_line = node_text(node, src).lines().next().unwrap_or("");
    fact_at(text, start.row, start.column, first_line.len())
}

/// Whether the literal at `node` is a string the code uses: not an
/// `#include` path, nor the language of `extern "C"`.
fn is_used_as_a_string(node: Node) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    let field = match parent.kind() {
        "preproc_include" => "path",
        "linkage_specification" => "value",
        _ => return true,
    };
    parent.child_by_field_name(field) != Some(node)
}

/// The strings in a macro's body (its `preproc_arg`, plain text).
fn macro_facts(node: Node, src: &str) -> Vec<StringFact> {
    let body = node_text(node, src);
    let origin = node.start_position();
    macro_strings::literals(body)
        .into_iter()
        .filter_map(|group| {
            let first = group.first()?.clone();
            let parts: Vec<&str> = group.iter().filter_map(|r| body.get(r.clone())).collect();
            let (row, column) = position_in(body, first.start, origin.row, origin.column);
            let first_line = body.get(first)?.lines().next().unwrap_or("");
            fact_at(joined_cut_short(&parts), row, column, first_line.len())
        })
        .collect()
}

/// The row and column of byte `offset` of `body`, which starts at
/// `row`, `column`.
fn position_in(body: &str, offset: usize, row: usize, column: usize) -> (usize, usize) {
    let before = body.get(..offset).unwrap_or("");
    match before.rfind('\n') {
        None => (row, column + offset),
        Some(newline) => (row + before.matches('\n').count(), offset - newline - 1),
    }
}

/// A string `text` starting at `row`, `column` (0-based), whose first line
/// is `width` bytes.
fn fact_at(text: String, row: usize, column: usize, width: usize) -> Option<StringFact> {
    let start = u32::try_from(column).ok()?;
    let span = Span {
        line: u32::try_from(row.checked_add(1)?).ok()?,
        start,
        end: start.checked_add(u32::try_from(width).ok()?)?,
    };
    Some(StringFact { text, span })
}

/// `parts` joined by spaces, cut short (with `…`) past `MAX_STRING_CHARS`
/// without building the rest.
fn joined_cut_short(parts: &[&str]) -> String {
    let mut chars = parts
        .iter()
        .enumerate()
        .flat_map(|(i, part)| (i > 0).then_some(' ').into_iter().chain(part.chars()));
    let kept: String = chars.by_ref().take(MAX_STRING_CHARS).collect();
    match chars.next() {
        Some(_) => kept + "…",
        None => kept,
    }
}

fn node_text<'s>(node: Node, src: &'s str) -> &'s str {
    src.get(node.byte_range()).unwrap_or("")
}

/// The graph's strings, moved out of `files`: each file's (that has any),
/// placed in the function they are in; and the files that had more than
/// were kept.
pub fn by_file(
    files: &mut [SourceFile],
    functions: &[FunctionMetrics],
) -> (HashMap<String, Vec<StringLiteral>>, Vec<String>) {
    let mut functions_in: HashMap<&str, Vec<&FunctionMetrics>> = HashMap::new();
    for function in functions {
        functions_in
            .entry(function.file.as_str())
            .or_default()
            .push(function);
    }
    let cut_short = files
        .iter()
        .filter(|file| file.facts.strings.is_cut_short)
        .map(|file| file.path.clone())
        .collect();
    let strings = files
        .iter_mut()
        .filter_map(|file| {
            let facts = std::mem::take(&mut file.facts.strings.strings);
            if facts.is_empty() {
                return None;
            }
            let in_file = functions_in
                .get(file.path.as_str())
                .map_or(&[][..], Vec::as_slice);
            Some((file.path.clone(), placed(facts, in_file)))
        })
        .collect();
    (strings, cut_short)
}

/// `facts` placed in the innermost of `functions` (one file's) holding
/// each: the one starting last among those whose lines hold it. One sweep
/// down the file, so many functions don't cost time per string.
fn placed(facts: Vec<StringFact>, functions: &[&FunctionMetrics]) -> Vec<StringLiteral> {
    let mut by_start: Vec<&FunctionMetrics> = functions.to_vec();
    by_start.sort_by_key(|f| f.line);
    let mut order: Vec<usize> = (0..facts.len()).collect();
    order.sort_by_key(|&i| facts[i].span.line);
    let lines = |f: &FunctionMetrics| -> Range<u32> { f.line..f.line.saturating_add(f.lines) };
    let mut function_of = vec![None; facts.len()];
    // The functions started so far that may still hold the line, innermost
    // last.
    let mut open: Vec<&FunctionMetrics> = Vec::new();
    let mut next = 0;
    for i in order {
        let line = facts[i].span.line;
        while let Some(function) = by_start.get(next).filter(|f| f.line <= line) {
            open.push(function);
            next += 1;
        }
        while open.last().is_some_and(|f| !lines(f).contains(&line)) {
            open.pop();
        }
        function_of[i] = open.last().map(|f| f.symbol);
    }
    facts
        .into_iter()
        .zip(function_of)
        .map(|(fact, function)| StringLiteral {
            text: fact.text,
            span: fact.span,
            function,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::extract::Extractor;
    use crate::index::graph_of_files;

    fn strings_in(source: &str) -> FileStrings {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .expect("grammar");
        let tree = parser.parse(source, None).expect("parse");
        collect(tree.root_node(), source, MAX_STRINGS_PER_FILE)
    }

    fn texts(source: &str) -> Vec<String> {
        strings_in(source)
            .strings
            .into_iter()
            .map(|s| s.text)
            .collect()
    }

    #[test]
    fn finds_strings_in_functions_and_at_file_scope() {
        let source = "static const char *usage = \"usage: tool [-v]\";\n\
                      int main() { puts(\"hello\"); return 0; }\n";
        assert_eq!(texts(source), ["\"usage: tool [-v]\"", "\"hello\""]);
    }

    #[test]
    fn says_where_each_string_is() {
        let source = "int f() {\n  return g(\"abc\");\n}\n";
        let found = strings_in(source).strings;
        assert_eq!(
            found[0].span,
            Span {
                line: 2,
                start: 11,
                end: 16
            }
        );
    }

    #[test]
    fn leaves_out_include_paths_extern_c_and_characters() {
        let source = "#include \"local.h\"\n#include <stdio.h>\n\
                      extern \"C\" { void f(void) { putchar('x'); puts(\"kept\"); } }\n";
        assert_eq!(texts(source), ["\"kept\""]);
    }

    #[test]
    fn adjacent_literals_are_one_string() {
        let source = "const char *s = \"one \"\n    \"two\";\n";
        assert_eq!(texts(source), ["\"one \" \"two\""]);
    }

    #[test]
    fn keeps_prefixes_raw_strings_and_quoted_macros() {
        let source = "#define VERSION \"1.2.3\"\n#define SIZE 64\n\
                      const wchar_t *w = L\"wide\";\nconst char *r = R\"(a\\b)\";\n";
        assert_eq!(texts(source), ["\"1.2.3\"", "L\"wide\"", "R\"(a\\b)\""]);
    }

    #[test]
    fn finds_each_string_in_a_macro_body() {
        let source = "#define A \"a\" /* \"not this\" */\n\
                      #define B \"b\" // \"nor this\"\n\
                      #define LOG(x) printf(\"hi %s\", x)\n\
                      #define TWO \"one\" SEP \"two\"\n\
                      #define ESC \"say \\\"hi\\\"\"\n\
                      #define QUOTE '\"'\n\
                      #define JOIN \"x\" \\\n  \"y\"\n";
        assert_eq!(
            texts(source),
            [
                "\"a\"",
                "\"b\"",
                "\"hi %s\"",
                "\"one\"",
                "\"two\"",
                "\"say \\\"hi\\\"\"",
                "\"x\" \"y\""
            ]
        );
    }

    #[test]
    fn says_where_a_macro_string_is() {
        let found = strings_in("#define LOG(x) printf(\"hi %s\", x)\n").strings;
        let at = found[0].span;
        assert_eq!((at.line, at.start, at.end), (1, 22, 29));
    }

    #[test]
    fn the_project_keeps_at_most_its_budget_of_strings() {
        let mut extractor = Extractor::with_string_budget(4).expect("grammar");
        let three = "const char *t[] = { \"a\", \"b\", \"c\" };\n";
        let first = extractor.extract(three).expect("parse").strings;
        let second = extractor.extract(three).expect("parse").strings;
        let third = extractor.extract(three).expect("parse").strings;
        assert_eq!((first.strings.len(), first.is_cut_short), (3, false));
        assert_eq!((second.strings.len(), second.is_cut_short), (1, true));
        assert_eq!((third.strings.len(), third.is_cut_short), (0, true));
    }

    #[test]
    fn strings_are_placed_in_the_innermost_function_holding_them() {
        let graph = graph_of_files(&[(
            "a.cpp",
            "struct S {\n  int m() {\n    return puts(\"in m\");\n  }\n};\n\
             const char *between = \"between\";\n\
             int f() { return puts(\"in f\"); } int g() { return puts(\"in g\"); }\n\
             const char *after = \"after\";\n",
        )]);
        let name = |s: &StringLiteral| {
            s.function
                .and_then(|id| graph.symbols.get(&id))
                .map(|f| f.qualified_name.as_str())
        };
        let placed: Vec<(&str, Option<&str>)> = graph.strings["a.cpp"]
            .iter()
            .map(|s| (s.text.as_str(), name(s)))
            .collect();
        assert_eq!(placed[0], ("\"in m\"", Some("S::m")));
        assert_eq!(placed[1], ("\"between\"", None));
        // On one line, the later function is the innermost holding it.
        assert_eq!(placed[2].1, placed[3].1);
        assert_eq!(placed[4], ("\"after\"", None));
    }

    #[test]
    fn a_string_over_lines_is_placed_at_its_first_line() {
        let source = "const char *r = R\"(first\nsecond)\";\n";
        let found = strings_in(source).strings;
        assert_eq!(found[0].span.line, 1);
        assert_eq!(found[0].span.start, 16);
        assert_eq!(
            found[0].span.end as usize,
            "const char *r = R\"(first".len()
        );
        assert!(found[0].text.contains("second"), "{:?}", found[0].text);
    }

    #[test]
    fn a_long_string_is_cut_short() {
        let long = "x".repeat(MAX_STRING_CHARS * 2);
        let source = format!("const char *s = \"{long}\";\n");
        let found = strings_in(&source).strings;
        assert_eq!(found[0].text.chars().count(), MAX_STRING_CHARS + 1);
        assert!(found[0].text.ends_with('…'));
    }

    #[test]
    fn a_file_of_too_many_strings_is_cut_short_and_says_so() {
        let many = "\"s\",".repeat(MAX_STRINGS_PER_FILE + 10);
        let source = format!("const char *t[] = {{{many}}};\n");
        let found = strings_in(&source);
        assert_eq!(found.strings.len(), MAX_STRINGS_PER_FILE);
        assert!(found.is_cut_short);
        assert!(!strings_in("const char *s = \"a\";").is_cut_short);
    }

    #[test]
    fn exactly_the_most_strings_kept_is_not_cut_short() {
        let all = "\"s\",".repeat(MAX_STRINGS_PER_FILE);
        let source = format!("const char *t[] = {{{all}}};\nint after(void) {{ return 1; }}\n");
        let found = strings_in(&source);
        assert_eq!(found.strings.len(), MAX_STRINGS_PER_FILE);
        assert!(!found.is_cut_short);
    }

    #[test]
    fn each_string_is_placed_in_the_function_it_is_in() {
        let graph = graph_of_files(&[(
            "a.c",
            "const char *g = \"global\";\n\
             int helper(void) {\n  return puts(\"in helper\");\n}\n\
             int main(void) {\n  return puts(\"in main\");\n}\n",
        )]);
        let name = |s: &StringLiteral| {
            s.function
                .and_then(|id| graph.symbols.get(&id))
                .map(|f| f.qualified_name.as_str())
        };
        let placed: Vec<(&str, Option<&str>)> = graph.strings["a.c"]
            .iter()
            .map(|s| (s.text.as_str(), name(s)))
            .collect();
        assert_eq!(
            placed,
            [
                ("\"global\"", None),
                ("\"in helper\"", Some("helper")),
                ("\"in main\"", Some("main")),
            ]
        );
    }

    #[test]
    fn files_without_strings_have_no_entry() {
        let graph = graph_of_files(&[("a.c", "int f(void) { return 1; }\n")]);
        assert!(graph.strings.is_empty());
        assert!(graph.strings_cut_short.is_empty());
    }
}

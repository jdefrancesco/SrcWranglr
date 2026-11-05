//! A function definition's prototype and parameters, as written: what a
//! fuzz harness needs to know to call it.

use std::ops::Range;

use tree_sitter::Node;

use super::syntax::{declarator_name, named_children, text};
use crate::model::{Param, Signature, MAX_TYPE_CHARS};

/// Prototypes longer than this are left out (a harness can't use them).
pub(super) const MAX_PROTOTYPE_CHARS: usize = 512;
/// Parameter names longer than this are dropped.
pub(super) const MAX_NAME_CHARS: usize = 128;
/// Source longer than this many bytes per kept character isn't read at
/// all (comments and whitespace aside, it can't fit).
const MAX_BYTES_PER_CHAR: usize = 4;
/// Parameters read per function; real ones rarely have more than a dozen.
pub(super) const MAX_PARAMS: usize = 32;

/// A piece of source to leave out of written text, and what replaces it.
type Cut = (Range<usize>, &'static str);

/// Reads the signature of `definition` (a `function_definition`) whose
/// `function_declarator` is `declarator`.
pub(super) fn of(definition: Node, declarator: Node, src: &str) -> Signature {
    let list = declarator
        .child_by_field_name("parameters")
        .map(all_children)
        .unwrap_or_default();
    Signature {
        prototype: prototype(definition, src),
        params: list
            .iter()
            .filter(|p| {
                matches!(
                    p.kind(),
                    "parameter_declaration" | "optional_parameter_declaration"
                )
            })
            .filter_map(|p| param(*p, src))
            .take(MAX_PARAMS)
            .collect(),
        is_static: named_children(definition)
            .iter()
            .any(|c| c.kind() == "storage_class_specifier" && text(*c, src) == "static"),
        // C's `...` is an anonymous token; `Args... args` a declaration.
        is_variadic: list.iter().any(|p| {
            matches!(
                p.kind(),
                "..." | "variadic_parameter" | "variadic_parameter_declaration"
            )
        }),
        is_template: definition
            .parent()
            .is_some_and(|p| p.kind() == "template_declaration"),
    }
}

/// Named and anonymous children alike.
fn all_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

/// From the start of the definition to the end of its declarator: return
/// type, name, parameters and trailing qualifiers, but not a constructor's
/// initializer list or the body.
fn prototype(definition: Node, src: &str) -> String {
    let end = definition
        .child_by_field_name("declarator")
        .map_or(definition.end_byte(), |d| d.end_byte());
    let range = definition.start_byte()..end;
    if range.len() > MAX_PROTOTYPE_CHARS * MAX_BYTES_PER_CHAR {
        return String::new();
    }
    let written = written(src, range.clone(), comments_in(definition, &range));
    if written.chars().count() > MAX_PROTOTYPE_CHARS {
        String::new()
    } else {
        written
    }
}

/// One parameter, or `None` for the `void` of `f(void)`.
fn param(node: Node, src: &str) -> Option<Param> {
    let declared_type = node.child_by_field_name("type")?;
    let declarator = node.child_by_field_name("declarator");
    if declarator.is_none() && text(declared_type, src) == "void" {
        return None;
    }
    let name = declarator
        .and_then(declarator_name)
        .filter(|n| n.kind() == "identifier");
    // Up to the declarator's end, which leaves out any `= default`.
    let end = declarator.map_or(declared_type.end_byte(), |d| d.end_byte());
    let range = node.start_byte()..end;
    let type_text = if range.len() > MAX_TYPE_CHARS * MAX_BYTES_PER_CHAR {
        String::new()
    } else {
        let cuts: Vec<Cut> = comments_in(node, &range)
            .into_iter()
            .chain(name.map(|n| (n.byte_range(), "")))
            .collect();
        written(src, range, cuts)
    };
    Some(Param {
        type_text: if type_text.chars().count() > MAX_TYPE_CHARS {
            String::new()
        } else {
            type_text
        },
        name: name
            .filter(|n| n.byte_range().len() <= MAX_NAME_CHARS)
            .map(|n| text(n, src).to_string()),
    })
}

/// Comments inside `node` that start within `range`, each to be replaced
/// by a space (so the words either side stay apart).
fn comments_in(node: Node, range: &Range<usize>) -> Vec<Cut> {
    let mut found = Vec::new();
    let mut cursor = node.walk();
    loop {
        let current = cursor.node();
        let is_inside = current.start_byte() < range.end;
        if is_inside && current.kind() == "comment" {
            found.push((current.byte_range(), " "));
        }
        if is_inside && cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return found;
            }
        }
    }
}

/// The source in `range` with `cuts` replaced, whitespace collapsed.
fn written(src: &str, range: Range<usize>, mut cuts: Vec<Cut>) -> String {
    cuts.sort_by_key(|(cut, _)| cut.start);
    let mut out = String::new();
    let mut at = range.start;
    for (cut, replacement) in cuts {
        if cut.start < at || cut.end > range.end {
            continue;
        }
        out.push_str(src.get(at..cut.start).unwrap_or(""));
        out.push_str(replacement);
        at = cut.end;
    }
    out.push_str(src.get(at..range.end).unwrap_or(""));
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use tree_sitter::Parser;

    use super::*;
    use crate::index::syntax::find_function_declarator;

    /// The signature of the first function defined in `source`.
    fn signature_of(source: &str) -> Signature {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .expect("grammar");
        let tree = parser.parse(source, None).expect("parse");
        let root = tree.root_node();
        let mut cursor = root.walk();
        let definition = root
            .children(&mut cursor)
            .find(|n| n.kind() == "function_definition")
            .expect("a function");
        let declarator = definition
            .child_by_field_name("declarator")
            .and_then(find_function_declarator)
            .expect("a function declarator");
        of(definition, declarator, source)
    }

    fn param(type_text: &str, name: Option<&str>) -> Param {
        Param {
            type_text: type_text.into(),
            name: name.map(str::to_string),
        }
    }

    #[test]
    fn reads_types_and_names_of_a_buffer_and_length() {
        let signature = signature_of("int parse(const uint8_t *buf, size_t len) { return 0; }");
        assert_eq!(
            signature.prototype,
            "int parse(const uint8_t *buf, size_t len)"
        );
        assert_eq!(
            signature.params,
            [
                param("const uint8_t *", Some("buf")),
                param("size_t", Some("len"))
            ]
        );
        assert!(!signature.is_static && !signature.is_variadic);
    }

    #[test]
    fn void_parameter_list_means_no_parameters() {
        assert!(signature_of("int f(void) { return 0; }").params.is_empty());
        assert!(signature_of("int f() { return 0; }").params.is_empty());
    }

    #[test]
    fn notices_static_and_variadic_functions() {
        let signature = signature_of("static void log_it(const char *fmt, ...) {}");
        assert!(signature.is_static);
        assert!(signature.is_variadic);
        assert_eq!(signature.params, [param("const char *", Some("fmt"))]);
    }

    #[test]
    fn keeps_references_arrays_and_unnamed_parameters() {
        let signature = signature_of("void f(const std::string &s, char out[16], int) {}");
        assert_eq!(
            signature.params,
            [
                param("const std::string &", Some("s")),
                param("char [16]", Some("out")),
                param("int", None)
            ]
        );
    }

    #[test]
    fn drops_default_values_and_comments() {
        let signature = signature_of(
            "int f(int flags /* bits */ = 0, // the input\n const char *in) { return 0; }",
        );
        assert_eq!(
            signature.params,
            [
                param("int", Some("flags")),
                param("const char *", Some("in"))
            ]
        );
        assert_eq!(signature.prototype, "int f(int flags = 0, const char *in)");
    }

    #[test]
    fn names_function_pointer_parameters() {
        let signature = signature_of("void each(void (*cb)(int), int n) {}");
        assert_eq!(
            signature.params,
            [param("void (*)(int)", Some("cb")), param("int", Some("n"))]
        );
    }

    #[test]
    fn keeps_the_qualified_name_of_an_out_of_line_method() {
        let signature = signature_of("bool Parser::feed(const char *data, int n) { return true; }");
        assert_eq!(
            signature.prototype,
            "bool Parser::feed(const char *data, int n)"
        );
    }

    #[test]
    fn leaves_out_overlong_prototypes_but_keeps_parameters() {
        let long_type = "a".repeat(MAX_PROTOTYPE_CHARS);
        let signature = signature_of(&format!("void f({long_type} x) {{}}"));
        assert!(signature.prototype.is_empty());
        assert_eq!(signature.params.len(), 1);
    }

    #[test]
    fn notices_templates() {
        let source = "template <typename T> int parse(const T *p) { return 0; }";
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .expect("grammar");
        let tree = parser.parse(source, None).expect("parse");
        let template = tree.root_node().named_child(0).expect("template");
        let definition = template
            .named_children(&mut template.walk())
            .find(|n| n.kind() == "function_definition")
            .expect("a function");
        let declarator = definition
            .child_by_field_name("declarator")
            .and_then(find_function_declarator)
            .expect("a declarator");
        assert!(of(definition, declarator, source).is_template);
        assert!(!signature_of("int f(int a) { return a; }").is_template);
    }

    #[test]
    fn drops_overlong_names_and_types() {
        let name = "n".repeat(MAX_NAME_CHARS + 1);
        let long_type = "t".repeat(MAX_TYPE_CHARS * MAX_BYTES_PER_CHAR + 1);
        let signature = signature_of(&format!("void f(int {name}, {long_type} x) {{}}"));
        assert_eq!(signature.params[0], param("int", None));
        assert_eq!(signature.params[1].type_text, "");
    }

    #[test]
    fn stops_reading_parameters_at_the_limit() {
        let params: Vec<String> = (0..MAX_PARAMS + 5).map(|i| format!("int p{i}")).collect();
        let signature = signature_of(&format!("void f({}) {{}}", params.join(", ")));
        assert_eq!(signature.params.len(), MAX_PARAMS);
    }
}

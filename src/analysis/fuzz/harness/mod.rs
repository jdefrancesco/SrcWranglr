//! Drafts a libFuzzer harness for one function: `LLVMFuzzerTestOneInput`
//! taking each parameter from a `FuzzedDataProvider`, buffers last.
//!
//! Names, types and paths come from a source tree that may be hostile, so
//! each is checked before it goes into the code; anything that doesn't
//! pass becomes a TODO for the user instead.

mod body;

use super::params::plan;
use super::{class_names, split_name, Placement};
use crate::model::{
    CodeGraph, FunctionMetrics, OccurrenceRole, SymbolId, SymbolKind, MAX_TYPE_CHARS,
};

const HEADER_EXTENSIONS: &[&str] = &["h", "hh", "hpp", "hxx", "h++", "inl"];
/// File names keep at most this much of the function's name.
const MAX_FILE_STEM: usize = 64;
/// What marks a place that needs the user's attention.
const TODO: &str = "TODO:";

/// A drafted harness, ready to show and save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Harness {
    /// A suggested file name: `fuzz_<function>.cc`.
    pub file_name: String,
    pub code: String,
}

/// How many places in harness `code` need the user's attention.
pub fn todo_count(code: &str) -> usize {
    code.matches(TODO).count()
}

/// A harness for `function`, or `None` if its symbol isn't in `graph`.
pub fn draft(graph: &CodeGraph, function: &FunctionMetrics) -> Option<Harness> {
    let symbol = graph.symbols.get(&function.symbol)?;
    let name = symbol.qualified_name.as_str();
    let placement = Placement::of(&class_names(graph), symbol, function.signature.is_static);
    let fills = plan(&function.signature.params);
    let file_name = file_name_for(name);
    let body = body::body(name, function, &placement, &fills);
    let mut system = vec!["<cstddef>", "<cstdint>"];
    system.extend(body.includes.iter().copied());
    let mut includes: Vec<String> = system.iter().map(|h| format!("#include {h}")).collect();
    if body.uses_provider {
        includes.extend([
            String::new(),
            "#include <fuzzer/FuzzedDataProvider.h>".into(),
        ]);
    }
    let sections = [
        intro(name, function, &file_name),
        includes.join("\n"),
        project_declaration(graph, function, &placement, name),
        body.code,
    ];
    Some(Harness {
        file_name,
        code: sections.join("\n\n") + "\n",
    })
}

/// `fuzz_<leaf name>.cc`, keeping only identifier characters.
pub fn file_name_for(qualified: &str) -> String {
    let stem: String = split_name(qualified)
        .1
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_FILE_STEM)
        .collect();
    let stem = if stem.is_empty() {
        "target".into()
    } else {
        stem
    };
    format!("fuzz_{stem}.cc")
}

/// What the harness is for, and how to build and run it.
fn intro(name: &str, function: &FunctionMetrics, file_name: &str) -> String {
    let stem = file_name.trim_end_matches(".cc");
    let prototype = &function.signature.prototype;
    let declaration = if is_safe_prototype(prototype) {
        format!("the setup below is guessed from the declaration\n//   {prototype}\n// and")
    } else {
        "the setup below is guessed from its declaration, and".into()
    };
    [
        format!(
            "// libFuzzer harness for {} ({}:{}), drafted by SrcWranglr.",
            comment_safe(name),
            comment_safe(&function.file),
            function.line
        ),
        format!("// Check it before use: {declaration} look at each TODO."),
        "//".into(),
        "// Build with clang, adding the project's sources (or library) and include paths:"
            .into(),
        format!(
            "//   clang++ -g -O1 -fsanitize=fuzzer,address,undefined -I. {file_name} <sources> -o {stem}"
        ),
        "// Run it on a folder of sample inputs:".into(),
        format!("//   ./{stem} corpus/"),
        "// AFL++ builds the same file with afl-clang-fast++ -fsanitize=fuzzer.".into(),
    ]
    .join("\n")
}

/// How the harness learns about the function: its header, its own file
/// (if static), a copy of its prototype, or a TODO.
fn project_declaration(
    graph: &CodeGraph,
    function: &FunctionMetrics,
    placement: &Placement,
    name: &str,
) -> String {
    let (notes, declared) = if function.signature.is_static && placement.class.is_none() {
        static_include(function, name)
    } else {
        header_or_copy(graph, function, placement, name)
    };
    let declared = declared.map(|d| {
        if function.file.ends_with(".c") {
            format!("extern \"C\" {{\n{d}\n}}")
        } else {
            d
        }
    });
    notes
        .into_iter()
        .chain(declared)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A static function's own file, compiled into the harness.
fn static_include(function: &FunctionMetrics, name: &str) -> (Vec<String>, Option<String>) {
    let shown = comment_safe(name);
    let file = comment_safe(&function.file);
    let Some(line) = include_line(&function.file) else {
        return (
            vec![format!(
                "// {TODO} #include the file that defines {shown} (its path can't be written here safely)."
            )],
            None,
        );
    };
    let mut notes = vec![format!(
        "// {shown} is static: its file is compiled into the harness, so leave {file} out of the build."
    )];
    if function.file.ends_with(".c") {
        notes.push(format!(
            "// {TODO} if {file} doesn't compile as C++, make {shown} non-static or call it through a small C wrapper."
        ));
    }
    (notes, Some(line))
}

/// The header declaring the function, or else a copy of its prototype.
fn header_or_copy(
    graph: &CodeGraph,
    function: &FunctionMetrics,
    placement: &Placement,
    name: &str,
) -> (Vec<String>, Option<String>) {
    let header = header_of(graph, function.symbol, placement);
    if let Some(line) = header.as_deref().and_then(include_line) {
        return (Vec::new(), Some(line));
    }
    let why = if header.is_some() {
        " (its path can't be written here safely)"
    } else {
        ""
    };
    let todo = format!(
        "// {TODO} #include the header that declares {}{why}.",
        comment_safe(name)
    );
    match copied_prototype(function, placement, name) {
        Some((note, declaration)) if header.is_none() => (vec![note], Some(declaration)),
        Some((note, declaration)) => (vec![todo, note], Some(declaration)),
        None => (vec![todo], None),
    }
}

/// The function's own prototype, declared again inside its namespaces,
/// with a note saying where it came from. Never for members (their class
/// has to be included) or out-of-line `ns::f` definitions.
fn copied_prototype(
    function: &FunctionMetrics,
    placement: &Placement,
    name: &str,
) -> Option<(String, String)> {
    let prototype = function.signature.prototype.as_str();
    if placement.class.is_some() || !is_safe_prototype(prototype) {
        return None;
    }
    let before_parameters = &prototype[..prototype.find('(').unwrap_or(prototype.len())];
    if before_parameters
        .rsplit(' ')
        .next()
        .is_some_and(|n| n.contains("::"))
    {
        return None;
    }
    let declaration = format!(
        "{};",
        prototype.strip_prefix("inline ").unwrap_or(prototype)
    );
    let declaration = match split_name(name).0 {
        None => declaration,
        Some(scope) if is_qualified_identifier(scope) => {
            let opening: Vec<String> = scope
                .split("::")
                .map(|n| format!("namespace {n} {{"))
                .collect();
            let closing = vec!["}"; opening.len()].join("\n");
            format!("{}\n{declaration}\n{closing}", opening.join("\n"))
        }
        Some(_) => return None,
    };
    let note = format!(
        "// No header declares {}; this declaration is copied from {}:{}.",
        comment_safe(name),
        comment_safe(&function.file),
        function.line
    );
    Some((note, declaration))
}

/// The header that declares `symbol`, or else the one defining its class.
fn header_of(graph: &CodeGraph, symbol: SymbolId, placement: &Placement) -> Option<String> {
    let declared_in = |id: SymbolId| {
        graph
            .locations_of(id)
            .into_iter()
            .filter(|l| l.occurrence.role == OccurrenceRole::Declaration && is_header(&l.file))
            .map(|l| l.file)
            .next()
    };
    declared_in(symbol).or_else(|| {
        let class = placement.class.as_deref()?;
        let class_id = graph
            .symbols
            .values()
            .filter(|s| {
                s.qualified_name == class
                    && matches!(s.kind, SymbolKind::Class | SymbolKind::Struct)
            })
            .map(|s| s.id)
            .min()?;
        declared_in(class_id)
    })
}

fn is_header(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, extension)| {
        HEADER_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
    })
}

/// `#include "path"`, if `path` can't break out of the quotes or reach
/// outside the project.
fn include_line(path: &str) -> Option<String> {
    let is_safe = !path.is_empty()
        && !path.starts_with('/')
        && path.split('/').all(|segment| segment != "..")
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+@".contains(c));
    is_safe.then(|| format!("#include \"{path}\""))
}

/// Text for a `//` comment: printable ASCII only, so no line breaks, no
/// `\\` to continue it, and no bidi or invisible characters to mislead.
fn comment_safe(text: &str) -> String {
    text.chars()
        .map(|c| {
            // `?` too: `??/` is a trigraph for a backslash.
            if c == ' ' || (c.is_ascii_graphic() && !"\\?".contains(c)) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A prototype safe to copy as code: one declaration, no comments,
/// strings, preprocessor lines, line continuations, or the digraphs and
/// trigraphs that spell braces and `#` another way.
fn is_safe_prototype(prototype: &str) -> bool {
    !prototype.is_empty()
        && !prototype.contains("<:")
        && !prototype.contains(":>")
        && prototype
            .chars()
            .all(|c| c == ' ' || (c.is_ascii_graphic() && !";{}#\\\"'/%?".contains(c)))
}

/// A type safe to write as code.
fn is_safe_type(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_TYPE_CHARS
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " _:*&<>,[]()".contains(c))
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_qualified_identifier(name: &str) -> bool {
    name.split("::").all(is_identifier)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

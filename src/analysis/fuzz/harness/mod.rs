//! Drafts a fuzz harness for one function, taking each parameter from a
//! `FuzzedDataProvider`, buffers last: for libFuzzer, or for AFL++ in
//! persistent mode.
//!
//! Names, types and paths come from a source tree that may be hostile, so
//! each is checked before it goes into the code; anything that doesn't
//! pass becomes a TODO for the user instead.

mod body;
mod entry;

use std::collections::BTreeSet;

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
/// What `main` is renamed to in a project file the harness compiles in.
pub const RENAMED_MAIN: &str = "project_main";
/// ASan settings for fuzzing: what afl-fuzz needs (abort on the first
/// error, no symbolizing), no leak reports at exit, and more bugs caught:
/// stack use after return, and reads of freshly allocated memory (filled
/// with garbage for allocations up to 1 GiB, not just 4 KiB).
pub const ASAN_OPTIONS: &str = "abort_on_error=1:symbolize=0:detect_leaks=0:\
    detect_stack_use_after_return=1:max_malloc_fill_size=1073741824";

/// Which fuzzer a harness is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// `LLVMFuzzerTestOneInput`, for libFuzzer (AFL++ can link it too,
    /// through its libAFLDriver).
    LibFuzzer,
    /// A `main` running inputs in AFL++'s persistent mode.
    Afl,
}

impl Engine {
    pub const ALL: [Engine; 2] = [Engine::LibFuzzer, Engine::Afl];

    pub fn label(self) -> &'static str {
        match self {
            Engine::LibFuzzer => "libFuzzer",
            Engine::Afl => "AFL++",
        }
    }
}

/// A drafted harness, ready to show and save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Harness {
    /// A suggested file name: `fuzz_<function>.cc`, or
    /// `fuzz_<function>_afl.cc` for AFL++.
    pub file_name: String,
    pub code: String,
}

/// How many places in harness `code` need the user's attention.
pub fn todo_count(code: &str) -> usize {
    code.matches(TODO).count()
}

/// A harness for `function` for `engine`, or `None` if its symbol isn't
/// in `graph`.
pub fn draft(graph: &CodeGraph, function: &FunctionMetrics, engine: Engine) -> Option<Harness> {
    let symbol = graph.symbols.get(&function.symbol)?;
    let name = symbol.qualified_name.as_str();
    let placement = Placement::of(&class_names(graph), symbol, function.signature.is_static);
    let fills = plan(&function.signature.params);
    let file_name = file_name_for(name, engine);
    let body = body::body(name, function, &placement, &fills);
    let others: BTreeSet<&str> = body
        .includes
        .iter()
        .chain(entry::includes(engine))
        .copied()
        .collect();
    let system = ["<cstddef>", "<cstdint>"].into_iter().chain(others);
    let mut includes: Vec<String> = system.map(|h| format!("#include {h}")).collect();
    if body.uses_provider {
        includes.extend([
            String::new(),
            "#include <fuzzer/FuzzedDataProvider.h>".into(),
        ]);
    }
    let sections = [
        intro(name, function, engine),
        includes.join("\n"),
        project_declaration(graph, function, &placement, name),
        entry::wrapped(&body.statements, engine),
    ];
    Some(Harness {
        file_name,
        code: sections.join("\n\n") + "\n",
    })
}

/// The harness's file name for `engine`.
pub fn file_name_for(qualified: &str, engine: Engine) -> String {
    let stem = stem_for(qualified);
    match engine {
        Engine::LibFuzzer => format!("{stem}.cc"),
        Engine::Afl => format!("{stem}_afl.cc"),
    }
}

/// `fuzz_<leaf name>`, keeping only identifier characters: the name of
/// the fuzzer built from either harness, and of its dictionary.
pub fn stem_for(qualified: &str) -> String {
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
    format!("fuzz_{stem}")
}

/// What the harness is for, and how to build and run it.
fn intro(name: &str, function: &FunctionMetrics, engine: Engine) -> String {
    let prototype = &function.signature.prototype;
    let declaration = if is_safe_prototype(prototype) {
        format!("the setup below is guessed from the declaration\n//   {prototype}\n// and")
    } else {
        "the setup below is guessed from its declaration, and".into()
    };
    let kind = match engine {
        Engine::LibFuzzer => "libFuzzer harness",
        Engine::Afl => "AFL++ persistent-mode harness",
    };
    let about = [
        format!(
            "// {kind} for {} ({}:{}), drafted by SrcWranglr.",
            comment_safe(name),
            comment_safe(&function.file),
            function.line
        ),
        format!("// Check it before use: {declaration} look at each TODO."),
        "//".into(),
    ];
    let how = match engine {
        Engine::LibFuzzer => libfuzzer_how(name),
        Engine::Afl => afl_how(name),
    };
    about.into_iter().chain(how).collect::<Vec<_>>().join("\n")
}

/// How to build and run a libFuzzer harness.
fn libfuzzer_how(name: &str) -> Vec<String> {
    let (file, stem) = (file_name_for(name, Engine::LibFuzzer), stem_for(name));
    vec![
        "// Build with clang, adding the project's sources (or library) and include paths,".into(),
        "// but not the file with the project's main(): libFuzzer brings its own.".into(),
        format!(
            "//   clang++ -g -O1 -fsanitize=fuzzer,address,undefined -I. {file} <sources> -o {stem}"
        ),
        "// Run it on a folder of sample inputs:".into(),
        format!("//   ./{stem} corpus/"),
        format!("// Add -dict={stem}.dict if you saved the dictionary SrcWranglr drafted."),
        "// AFL++ builds the same file with afl-clang-fast++ -fsanitize=fuzzer.".into(),
    ]
}

/// How to build and run an AFL++ harness.
fn afl_how(name: &str) -> Vec<String> {
    let (file, stem) = (file_name_for(name, Engine::Afl), stem_for(name));
    vec![
        "// Build the project's code with AFL++'s compilers too (C files with afl-clang-fast),"
            .into(),
        "// then this file, adding include paths. Leave out the project's main(): this file has one."
            .into(),
        format!(
            "//   AFL_USE_ASAN=1 AFL_USE_UBSAN=1 afl-clang-fast++ -g -O1 -I. {file} <objects> -o {stem}"
        ),
        "// A second build with CMPLOG helps afl-fuzz get past magic values and checksums:".into(),
        format!(
            "//   AFL_LLVM_CMPLOG=1 afl-clang-fast++ -g -O1 -I. {file} <objects> -o {stem}.cmplog"
        ),
        "// Run it on a folder of sample inputs, with ASan set to catch more (afl-fuzz needs".into(),
        "// abort_on_error=1 and symbolize=0 in any ASAN_OPTIONS you set):".into(),
        format!("//   export ASAN_OPTIONS={ASAN_OPTIONS}"),
        format!("//   afl-fuzz -i seeds -o out -c ./{stem}.cmplog -- ./{stem}"),
        format!("// Add -x {stem}.dict if you saved the dictionary SrcWranglr drafted."),
        format!(
            "// One process runs up to {} inputs, so the code must not keep state between them.",
            entry::AFL_LOOP_COUNT
        ),
        "// Built with plain clang++ instead, it runs one input from stdin, to replay a crash:"
            .into(),
        format!("//   ./{stem} < out/default/crashes/<file>"),
    ]
}

/// Whether the harness `#include`s `function`'s own file: a static
/// function can only be called from there.
pub fn includes_its_file(graph: &CodeGraph, function: &FunctionMetrics) -> bool {
    graph.symbols.get(&function.symbol).is_some_and(|symbol| {
        let placement = Placement::of(&class_names(graph), symbol, function.signature.is_static);
        is_compiled_in(function, &placement)
    })
}

fn is_compiled_in(function: &FunctionMetrics, placement: &Placement) -> bool {
    function.signature.is_static && placement.class.is_none()
}

/// How the harness learns about the function: its header, its own file
/// (if static), a copy of its prototype, or a TODO.
fn project_declaration(
    graph: &CodeGraph,
    function: &FunctionMetrics,
    placement: &Placement,
    name: &str,
) -> String {
    let (notes, declared) = if is_compiled_in(function, placement) {
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
    // A file compiled in may have a main() of its own.
    let declared = declared.map(|d| match is_compiled_in(function, placement) {
        true => format!(
            "#define main {RENAMED_MAIN}  // its main() would clash with the harness's\n{d}\n#undef main"
        ),
        false => d,
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
pub(super) fn comment_safe(text: &str) -> String {
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

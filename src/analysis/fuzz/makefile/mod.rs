//! Drafts the Makefile that builds a function's fuzzers: the AFL++ one
//! and its CMPLOG copy, the libFuzzer one, a plain build to replay
//! crashes, and a target running afl-fuzz. It lists the project files the
//! function calls into (see `sources`), each built once per fuzzer with
//! that fuzzer's compiler, C files as C.
//!
//! Paths come from a source tree that may be hostile and make reads `$`,
//! `%`, `:` and the like as syntax, so a path goes in only if it is plain;
//! any other becomes a TODO in a comment.

mod sources;

use std::collections::BTreeSet;
use std::path::Path;

use super::harness::{comment_safe, includes_its_file, stem_for, ASAN_OPTIONS};
use crate::model::{CodeGraph, FunctionMetrics};
use sources::{include_folders, is_make_safe, needed, Folders, Needed, MAX_SOURCES};

/// Clashes listed at most.
const MAX_CLASHES: usize = 10;

/// A drafted Makefile, ready to show and save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Makefile {
    /// `fuzz_<function>.mk`, run with `make -f`.
    pub file_name: String,
    pub text: String,
}

/// One way each source is built, for one fuzzer.
struct Build {
    name: &'static str,
    /// The C and C++ compile commands, flags included.
    c: &'static str,
    cpp: &'static str,
}

const BUILDS: [Build; 4] = [
    Build {
        name: "afl",
        c: "$(AFL_SANITIZERS) $(AFL_CC) $(CFLAGS)",
        cpp: "$(AFL_SANITIZERS) $(AFL_CXX) $(CXXFLAGS)",
    },
    Build {
        name: "cmplog",
        c: "AFL_LLVM_CMPLOG=1 $(AFL_CC) $(CFLAGS)",
        cpp: "AFL_LLVM_CMPLOG=1 $(AFL_CXX) $(CXXFLAGS)",
    },
    Build {
        name: "libfuzzer",
        c: "$(CLANG) $(CFLAGS) -fsanitize=fuzzer-no-link,address,undefined",
        cpp: "$(CLANGXX) $(CXXFLAGS) -fsanitize=fuzzer-no-link,address,undefined",
    },
    Build {
        name: "replay",
        c: "$(CLANG) $(CFLAGS) -fsanitize=address,undefined",
        cpp: "$(CLANGXX) $(CXXFLAGS) -fsanitize=address,undefined",
    },
];

/// The Makefile for `function`'s fuzzers (`has_dictionary`: whether its
/// dictionary was drafted, for `make fuzz`), or `None` if its symbol isn't
/// in `graph`.
pub fn draft(
    graph: &CodeGraph,
    function: &FunctionMetrics,
    has_dictionary: bool,
) -> Option<Makefile> {
    let name = graph.symbols.get(&function.symbol)?.qualified_name.as_str();
    let stem = stem_for(name);
    let file_name = format!("{stem}.mk");
    let is_own_file_included = includes_its_file(graph, function);
    let needed = needed(graph, function, is_own_file_included);
    let mut scanned: Vec<&str> = needed.sources.iter().map(String::as_str).collect();
    scanned.push(&function.file);
    let folders = include_folders(graph, &scanned);
    let sections = [
        intro(name, function, &file_name),
        variables(&stem, &needed.sources, &folders.usable),
        todos(&needed, &folders).join("\n"),
        rules(has_dictionary),
        object_rules(&needed.sources),
    ];
    let text = sections
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(Makefile {
        file_name,
        text: text + "\n",
    })
}

/// What it is for, and how to use it.
fn intro(name: &str, function: &FunctionMetrics, file_name: &str) -> String {
    let make = format!("make -f {file_name}");
    [
        format!(
            "# Builds the fuzzers for {} ({}:{}), drafted by SrcWranglr.",
            comment_safe(name),
            comment_safe(&function.file),
            function.line
        ),
        "# Check it before use. SOURCES are the files defining the functions it calls,".into(),
        "# as far as the index can tell: if the link says a symbol is undefined (a global,".into(),
        "# say), add the file defining it; files the project's configure step generates".into(),
        "# (version.c, config.h) need it run first. Add the flags the project's own build".into(),
        "# uses (defines, include folders) to CFLAGS. Save it next to the harnesses (as".into(),
        "# Save All does). Run make in the folder you save it in:".into(),
        "#".into(),
        format!("#   {make}             the AFL++ fuzzer, and its CMPLOG copy"),
        format!("#   {make} fuzz        run afl-fuzz on the inputs in seeds/"),
        format!("#   {make} libfuzzer   the libFuzzer fuzzer (needs a clang with libFuzzer)"),
        format!("#   {make} replay      a plain build: ./<name>_replay < crash-file"),
        format!("#   {make} clean"),
    ]
    .join("\n")
}

/// The settings, each of which `make VAR=…` can change.
fn variables(stem: &str, sources: &[String], folders: &[String]) -> String {
    let listed: String = sources.iter().map(|s| format!(" \\\n  {s}")).collect();
    let includes: String = folders.iter().map(|f| format!(" -I$(ROOT)/{f}")).collect();
    [
        "# The project's top folder, from this file's (saving fills it in). Change any".into(),
        "# setting here or on the command line: make -f <file> ROOT=../project".into(),
        "ROOT = .".into(),
        String::new(),
        format!("NAME := {stem}"),
        format!("SOURCES :={listed}"),
        format!("INCLUDES := -I$(ROOT){includes}"),
        "CFLAGS = -g -O1".into(),
        "CXXFLAGS = -g -O1".into(),
        "# Libraries the project links with (-lz, -lssl, ...).".into(),
        "LIBS =".into(),
        "SEEDS = seeds".into(),
        "OUT = out".into(),
        "# ASan for afl-fuzz runs: it needs abort_on_error=1 and symbolize=0.".into(),
        format!("ASAN_OPTIONS = {ASAN_OPTIONS}"),
        "# AFL++'s compilers, and a clang with libFuzzer (on macOS, Homebrew's llvm).".into(),
        "AFL_CC = afl-clang-fast".into(),
        "AFL_CXX = afl-clang-fast++".into(),
        "CLANG = clang".into(),
        "CLANGXX = clang++".into(),
        "# Sanitizers for the AFL++ build (empty for none).".into(),
        "AFL_SANITIZERS = AFL_USE_ASAN=1 AFL_USE_UBSAN=1".into(),
        String::new(),
        "BUILD := build/$(NAME)".into(),
        "objects = $(SOURCES:%=$(BUILD)/$(1)/%.o)".into(),
        "# Each project file's main() (if any) gets a name of its own, so none".into(),
        "# clashes with the harness's: src/ircd.c's is main_src_ircd.".into(),
        "RENAME_MAIN = -Dmain=main_$(subst /,_,$(subst .,_,$(subst -,_,$(subst +,_,$*))))".into(),
    ]
    .join("\n")
}

/// What it couldn't do for you, and what it left out.
fn todos(needed: &Needed, folders: &Folders) -> Vec<String> {
    let cannot = |what: &str, path: &str| {
        format!(
            "# TODO: add {what} {} (its path can't be written here as is).",
            comment_safe(path)
        )
    };
    let left_out = |what: &str| format!("# Left out, for another platform: {what}");
    let mut lines: Vec<String> = needed
        .unwritable
        .iter()
        .map(|f| cannot("to SOURCES:", f))
        .chain(
            folders
                .unwritable
                .iter()
                .map(|f| cannot("to INCLUDES: -I", f)),
        )
        .chain(
            needed
                .other_platform
                .iter()
                .map(|f| left_out(&comment_safe(f))),
        )
        .chain(
            folders
                .other_platform
                .iter()
                .map(|f| left_out(&format!("-I {}", comment_safe(f)))),
        )
        .collect();
    lines.extend(needed.clashes.iter().take(MAX_CLASHES).map(|(name, a, b)| {
        format!(
            "# TODO: {} is defined in both {} and {}; the link will clash, so leave one out.",
            comment_safe(name),
            comment_safe(a),
            comment_safe(b)
        )
    }));
    if needed.clashes.len() > MAX_CLASHES {
        let more = needed.clashes.len() - MAX_CLASHES;
        lines.push(format!(
            "# TODO: {more} more functions are defined in two of the files."
        ));
    }
    if needed.is_cut {
        lines.push(format!(
            "# TODO: it calls into more than {MAX_SOURCES} files; only the nearest are listed."
        ));
    }
    lines
}

/// The fuzzers, and running one.
fn rules(has_dictionary: bool) -> String {
    let dictionary = if has_dictionary {
        " -x $(NAME).dict"
    } else {
        ""
    };
    let link = |target: &str, harness: &str, build: &str, compile: &str| {
        format!(
            "{target}: {harness} $(call objects,{build})\n\t{compile} $(INCLUDES) $^ $(LIBS) -o $@"
        )
    };
    [
        ".PHONY: afl libfuzzer replay fuzz clean".to_string(),
        "afl: $(NAME) $(NAME).cmplog".into(),
        "libfuzzer: $(NAME)_libfuzzer".into(),
        "replay: $(NAME)_replay".into(),
        String::new(),
        link("$(NAME)", "$(NAME)_afl.cc", "afl", BUILDS[0].cpp),
        link("$(NAME).cmplog", "$(NAME)_afl.cc", "cmplog", BUILDS[1].cpp),
        link(
            "$(NAME)_libfuzzer",
            "$(NAME).cc",
            "libfuzzer",
            "$(CLANGXX) $(CXXFLAGS) -fsanitize=fuzzer,address,undefined",
        ),
        link("$(NAME)_replay", "$(NAME)_afl.cc", "replay", BUILDS[3].cpp),
        String::new(),
        "fuzz: $(NAME) $(NAME).cmplog".into(),
        format!("\tASAN_OPTIONS=$(ASAN_OPTIONS) afl-fuzz -i $(SEEDS) -o $(OUT){dictionary} -c ./$(NAME).cmplog -- ./$(NAME)"),
        String::new(),
        "clean:".into(),
        "\trm -rf $(BUILD) $(NAME) $(NAME).cmplog $(NAME)_libfuzzer $(NAME)_replay".into(),
    ]
    .join("\n")
}

/// How each kind of source among `sources` is built for each fuzzer: a
/// rule per extension as written (`.c`, `.cc`, `.C`), C only for `.c`.
fn object_rules(sources: &[String]) -> String {
    let extensions: BTreeSet<&str> = sources
        .iter()
        .filter_map(|s| s.rsplit_once('.').map(|(_, extension)| extension))
        .collect();
    BUILDS
        .iter()
        .flat_map(|build| {
            extensions.iter().map(move |extension| {
                let compile = if *extension == "c" {
                    build.c
                } else {
                    build.cpp
                };
                format!(
                    "$(BUILD)/{}/%.{extension}.o: $(ROOT)/%.{extension}\n\t@mkdir -p $(@D)\n\t\
                     {compile} $(INCLUDES) $(RENAME_MAIN) -c $< -o $@",
                    build.name
                )
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `ROOT` for a Makefile saved in `folder`: the way up to `project` if it
/// is inside it, else `project`'s full path; `None` if that path can't be
/// written into a Makefile (a space in it, say).
pub fn root_for(folder: &Path, project: &Path) -> Option<String> {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (folder, project) = (real(folder), real(project));
    if let Ok(inside) = folder.strip_prefix(&project) {
        let depth = inside.components().count();
        return Some(match depth {
            0 => ".".into(),
            depth => vec![".."; depth].join("/"),
        });
    }
    let full = project.to_str()?;
    let is_safe = full
        .strip_prefix('/')
        .is_some_and(|rest| rest.is_empty() || is_make_safe(rest));
    is_safe.then(|| full.to_string())
}

/// `text` with its `ROOT = …` line set to `root`.
pub fn with_root(text: &str, root: &str) -> String {
    let mut is_set = false;
    let lines: Vec<String> = text
        .lines()
        .map(|line| match line.starts_with("ROOT = ") && !is_set {
            true => {
                is_set = true;
                format!("ROOT = {root}")
            }
            false => line.to_string(),
        })
        .collect();
    lines.join("\n") + if text.ends_with('\n') { "\n" } else { "" }
}

#[cfg(test)]
mod tests;

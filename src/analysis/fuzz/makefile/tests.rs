//! Tests for drafting fuzzing Makefiles.

use std::path::Path;

use super::*;
use crate::index::graph_of_files;

const TLV_H: (&str, &str) = (
    "include/tlv.h",
    "int tlv_parse(const unsigned char *buf, unsigned long len);\n",
);
const TLV_C: (&str, &str) = (
    "src/tlv.c",
    "#include <tlv.h>\n#include \"util.h\"\n\
     int tlv_parse(const unsigned char *buf, unsigned long len) { return checksum(buf, len); }\n",
);
const UTIL_H: (&str, &str) = (
    "src/util.h",
    "int checksum(const unsigned char *p, unsigned long n);\n",
);
const UTIL_C: (&str, &str) = (
    "lib/util.c",
    "#include \"../src/util.h\"\n\
     int checksum(const unsigned char *p, unsigned long n) { return helper(p) + (int)n; }\n\
     int main(void) { return 0; }\n",
);
const HELPER_C: (&str, &str) = (
    "lib/helper.c",
    "int helper(const unsigned char *p) { return p[0]; }\n",
);
const TOOL_C: (&str, &str) = (
    "tools/tool.c",
    "int main(void) { return tlv_parse(0, 0); }\n",
);
const OTHER_C: (&str, &str) = ("src/other.c", "int unrelated(void) { return 1; }\n");

fn makefile_for(sources: &[(&str, &str)], name: &str, has_dictionary: bool) -> Makefile {
    let graph = graph_of_files(sources);
    let function = graph
        .functions
        .iter()
        .find(|f| graph.symbols[&f.symbol].qualified_name == name)
        .unwrap_or_else(|| panic!("no definition of {name}"));
    draft(&graph, function, has_dictionary).expect("a makefile")
}

fn tlv() -> Makefile {
    makefile_for(
        &[TLV_H, TLV_C, UTIL_H, UTIL_C, HELPER_C, TOOL_C, OTHER_C],
        "tlv_parse",
        true,
    )
}

/// The lines make reads (not comments).
fn code_lines(makefile: &Makefile) -> Vec<&str> {
    makefile
        .text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect()
}

fn assert_has(makefile: &Makefile, needle: &str) {
    assert!(
        makefile.text.contains(needle),
        "missing {needle:?} in:\n{}",
        makefile.text
    );
}

#[test]
fn sources_are_the_files_its_calls_reach() {
    let makefile = tlv();
    assert_has(
        &makefile,
        "SOURCES := \\\n  lib/helper.c \\\n  lib/util.c \\\n  src/tlv.c\n",
    );
    assert!(!makefile.text.contains("tools/tool.c"));
    assert!(!makefile.text.contains("src/other.c"));
}

#[test]
fn include_folders_come_from_the_include_lines() {
    // `<tlv.h>` lives in include/; "util.h" and "../src/util.h" are found
    // beside the file that includes them.
    assert_has(&tlv(), "INCLUDES := -I$(ROOT) -I$(ROOT)/include\n");
}

#[test]
fn main_is_renamed_in_every_project_file() {
    // lib/util.c defines main; any file added to SOURCES by hand might too.
    let makefile = tlv();
    assert_has(
        &makefile,
        "RENAME_MAIN = -Dmain=main_$(subst /,_,$(subst .,_,$(subst -,_,$(subst +,_,$*))))\n",
    );
    assert_has(&makefile, "$(INCLUDES) $(RENAME_MAIN) -c $< -o $@");
    assert!(!makefile.text.contains("-Dmain=project_main"));
}

#[test]
fn it_names_itself_and_the_harnesses() {
    let makefile = tlv();
    assert_eq!(makefile.file_name, "fuzz_tlv_parse.mk");
    assert_has(&makefile, "NAME := fuzz_tlv_parse\n");
    assert_has(&makefile, "make -f fuzz_tlv_parse.mk");
    assert_has(&makefile, "$(NAME): $(NAME)_afl.cc $(call objects,afl)\n");
    assert_has(
        &makefile,
        "$(NAME)_libfuzzer: $(NAME).cc $(call objects,libfuzzer)\n",
    );
    assert_has(&makefile, "AFL_LLVM_CMPLOG=1 $(AFL_CC)");
}

#[test]
fn the_dictionary_is_used_when_there_is_one() {
    assert_has(&tlv(), "-x $(NAME).dict");
    let without = makefile_for(
        &[TLV_H, TLV_C, UTIL_H, UTIL_C, HELPER_C],
        "tlv_parse",
        false,
    );
    assert!(!without.text.contains(".dict"));
}

#[test]
fn a_static_functions_own_file_is_left_to_the_harness() {
    let makefile = makefile_for(
        &[
            (
                "src/s.c",
                "static int parse(const char *s) { return g(s); }\n",
            ),
            ("src/g.c", "int g(const char *s) { return s[0]; }\n"),
        ],
        "parse",
        false,
    );
    assert_has(&makefile, "SOURCES := \\\n  src/g.c\n");
}

#[test]
fn cpp_sources_are_built_as_cpp() {
    let makefile = makefile_for(
        &[("src/a.cc", "int parse(const char *s) { return s[0]; }\n")],
        "parse",
        false,
    );
    assert_has(&makefile, "$(BUILD)/afl/%.cc.o: $(ROOT)/%.cc\n");
    assert_has(&makefile, "$(AFL_SANITIZERS) $(AFL_CXX) $(CXXFLAGS)");
    assert!(!makefile.text.contains("%.c.o:"));
}

#[test]
fn paths_that_could_inject_make_syntax_are_left_out() {
    let makefile = makefile_for(
        &[
            ("src/a.c", "int parse(const char *s) { return evil(s); }\n"),
            (
                "src/e $(shell id).c",
                "int evil(const char *s) { return 1; }\n",
            ),
        ],
        "parse",
        false,
    );
    assert!(code_lines(&makefile).iter().all(|l| !l.contains("shell")));
    assert_has(&makefile, "# TODO: add to SOURCES: src/e $(shell id).c");
}

#[test]
fn root_is_found_from_where_it_is_saved() {
    let project = Path::new("/work/proj");
    assert_eq!(
        root_for(Path::new("/work/proj"), project).as_deref(),
        Some(".")
    );
    assert_eq!(
        root_for(Path::new("/work/proj/fuzz/x"), project).as_deref(),
        Some("../..")
    );
    assert_eq!(
        root_for(Path::new("/elsewhere"), project).as_deref(),
        Some("/work/proj")
    );
    assert_eq!(
        root_for(Path::new("/elsewhere"), Path::new("/my proj")),
        None
    );
    let text = "NAME := x\nROOT = .\nSEEDS = seeds\n";
    assert_eq!(
        with_root(text, ".."),
        "NAME := x\nROOT = ..\nSEEDS = seeds\n"
    );
}

#[test]
fn every_function_in_a_needed_file_is_followed_too() {
    // b64 needs support.c, whose other function calls into log.c: the link
    // needs log.c even though b64 never calls it.
    let makefile = makefile_for(
        &[
            (
                "src/support.c",
                "int b64(const char *s) { return s[0]; }\nint other(void) { return log_it(); }\n",
            ),
            ("src/log.c", "int log_it(void) { return 0; }\n"),
        ],
        "b64",
        false,
    );
    assert_has(
        &makefile,
        "SOURCES := \\\n  src/log.c \\\n  src/support.c\n",
    );
}

#[test]
fn other_platforms_files_and_folders_are_left_out() {
    let makefile = makefile_for(
        &[
            (
                "src/a.c",
                "#include \"win.h\"\nint parse(const char *s) { return gui(s); }\n",
            ),
            (
                "src/windows/gui.c",
                "int gui(const char *s) { return 0; }\n",
            ),
            ("include/windows/win.h", "int gui(const char *s);\n"),
        ],
        "parse",
        false,
    );
    assert_has(&makefile, "SOURCES := \\\n  src/a.c\n");
    assert_has(&makefile, "INCLUDES := -I$(ROOT)\n");
    assert_has(
        &makefile,
        "# Left out, for another platform: src/windows/gui.c",
    );
    assert_has(
        &makefile,
        "# Left out, for another platform: -I include/windows",
    );
}

#[test]
fn a_function_defined_in_two_listed_files_is_flagged() {
    let makefile = makefile_for(
        &[
            (
                "src/a.c",
                "int parse(const char *s) { return init() + b(); }\nint init(void) { return 1; }\n",
            ),
            (
                "src/b.c",
                "int b(void) { return 2; }\nint init(void) { return 3; }\n",
            ),
        ],
        "parse",
        false,
    );
    assert_has(
        &makefile,
        "# TODO: init is defined in both src/a.c and src/b.c",
    );
}

#[test]
fn a_macro_written_like_a_function_ties_nothing_together() {
    // Each module "defines" CMD_FUNC (a macro making a command handler),
    // whose body calls the module's own helper. Read as one function, it
    // would pull every module in, and look like a clash.
    let module = |n: u32| {
        (
            format!("src/modules/m{n}.c"),
            format!(
                "int helper{n}(void) {{ return {n}; }}\nCMD_FUNC(cmd_{n}) {{ helper{n}(); }}\n"
            ),
        )
    };
    let modules: Vec<(String, String)> = (0..4).map(module).collect();
    let mut sources: Vec<(&str, &str)> = modules
        .iter()
        .map(|(f, c)| (f.as_str(), c.as_str()))
        .collect();
    sources.push((
        "src/core.c",
        "int parse(const char *s) { return other(s); }\nCMD_FUNC(cmd_core) { return; }\n",
    ));
    sources.push((
        "src/other.c",
        "int other(const char *s) { return s[0]; }\nCMD_FUNC(cmd_other) { return; }\n",
    ));
    let makefile = makefile_for(&sources, "parse", false);
    assert_has(&makefile, "SOURCES := \\\n  src/core.c \\\n  src/other.c\n");
    assert!(!makefile.text.contains("is defined in both"));
}

#[test]
fn versions_of_a_function_under_if_are_still_one_function() {
    let dispatch =
        "#if USE_POLL\nvoid fd_select(int d) { }\n#elif USE_KQUEUE\nvoid fd_select(int d) { }\n\
                    #else\nvoid fd_select(int d) { }\n#endif\n";
    let makefile = makefile_for(
        &[
            (
                "src/a.c",
                "int parse(const char *s) { fd_select(s[0]); return 0; }\n",
            ),
            ("src/dispatch.c", dispatch),
        ],
        "parse",
        false,
    );
    assert_has(&makefile, "SOURCES := \\\n  src/a.c \\\n  src/dispatch.c\n");
}

#[test]
fn files_of_only_globals_beside_the_sources_are_built_too() {
    let makefile = makefile_for(
        &[
            (
                "src/a.c",
                "extern int loop;\nint parse(const char *s) { return loop + s[0]; }\n",
            ),
            ("src/vars.c", "int loop;\nconst char *version = \"1.0\";\n"),
            ("tests/data.c", "int fixture = 1;\n"),
        ],
        "parse",
        false,
    );
    assert_has(&makefile, "SOURCES := \\\n  src/a.c \\\n  src/vars.c\n");
}

#[test]
fn settings_ignore_the_environment() {
    // `?=` would take ROOT, OUT or CFLAGS from whatever the shell exports.
    let makefile = tlv();
    assert!(code_lines(&makefile).iter().all(|l| !l.contains("?=")));
    assert_has(&makefile, "\nROOT = .\n");
    assert_has(&makefile, "\nCFLAGS = -g -O1\n");
}

#[test]
fn an_upper_case_extension_gets_its_own_rule() {
    let makefile = makefile_for(
        &[("src/A.C", "int parse(const char *s) { return s[0]; }\n")],
        "parse",
        false,
    );
    assert_has(&makefile, "$(BUILD)/afl/%.C.o: $(ROOT)/%.C\n");
    assert_has(&makefile, "$(AFL_SANITIZERS) $(AFL_CXX) $(CXXFLAGS)");
}

#[test]
fn only_files_defining_data_count_as_globals_and_not_included_ones() {
    let makefile = makefile_for(
        &[
            (
                "src/a.c",
                "#include \"ops.c\"\nint parse(const char *s) { return s[0]; }\n",
            ),
            ("src/vars.c", "int loop = 0;\n"),
            ("src/ops.c", "OP(read)\nOP(write)\n"),
            ("src/broken.c", "#include \"x.h\"\n"),
            ("src/table.c", "const int table[] = { 1, 2 };\n"),
        ],
        "parse",
        false,
    );
    assert_has(
        &makefile,
        "SOURCES := \\\n  src/a.c \\\n  src/table.c \\\n  src/vars.c\n",
    );
}

#[test]
fn cpp_overloads_are_not_clashes() {
    let makefile = makefile_for(
        &[
            (
                "src/a.cc",
                "int parse(const char *s) { return f(1) + g(); }\nint f(int x) { return x; }\n",
            ),
            (
                "src/b.cc",
                "int g() { return 0; }\nint f(double x) { return 1; }\n",
            ),
        ],
        "parse",
        false,
    );
    assert!(!makefile.text.contains("is defined in both"));
}

#[test]
fn it_says_to_run_make_where_it_is_saved() {
    assert_has(&tlv(), "Run make in the folder you save it in");
}

#[test]
fn fuzzing_runs_with_asan_set_to_catch_more() {
    let makefile = tlv();
    assert_has(
        &makefile,
        "ASAN_OPTIONS = abort_on_error=1:symbolize=0:detect_leaks=0:\
         detect_stack_use_after_return=1:max_malloc_fill_size=1073741824\n",
    );
    assert_has(
        &makefile,
        "\tASAN_OPTIONS=$(ASAN_OPTIONS) afl-fuzz -i $(SEEDS)",
    );
}

#[test]
fn a_main_in_the_file_the_harness_includes_needs_no_todo() {
    // The harness renames it around its #include now.
    let makefile = makefile_for(
        &[(
            "src/tool.c",
            "static int parse(const char *s) { return s[0]; }\nint main(void) { return 0; }\n",
        )],
        "parse",
        false,
    );
    assert!(!makefile.text.contains("defines main() too"));
}

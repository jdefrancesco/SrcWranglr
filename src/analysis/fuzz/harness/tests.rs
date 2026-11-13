//! Tests for drafting fuzz harnesses.

use super::body::variable_names;
use super::*;
use crate::index::graph_of_files;
use crate::model::Param;

/// The libFuzzer harness for the definition of `name` among `sources`.
fn harness_for(sources: &[(&str, &str)], name: &str) -> Harness {
    drafted_for(sources, name, Engine::LibFuzzer)
}

/// The harness for `engine` for the definition of `name` among `sources`.
fn drafted_for(sources: &[(&str, &str)], name: &str, engine: Engine) -> Harness {
    let graph = graph_of_files(sources);
    let function = graph
        .functions
        .iter()
        .find(|f| graph.symbols[&f.symbol].qualified_name == name)
        .unwrap_or_else(|| panic!("no definition of {name}"));
    draft(&graph, function, engine).expect("a harness")
}

fn assert_has(harness: &Harness, needle: &str) {
    assert!(
        harness.code.contains(needle),
        "missing {needle:?} in:\n{}",
        harness.code
    );
}

const PNG_H: (&str, &str) = (
    "include/png.h",
    "int parse_chunk(const uint8_t *buf, size_t len);",
);
const PNG_C: (&str, &str) = (
    "src/png.c",
    "int parse_chunk(const uint8_t *buf, size_t len) { return buf[0]; }",
);

#[test]
fn a_buffer_and_length_get_the_input_and_its_size() {
    let harness = harness_for(&[PNG_H, PNG_C], "parse_chunk");
    assert_eq!(harness.file_name, "fuzz_parse_chunk.cc");
    assert_has(
        &harness,
        "extern \"C\" int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {",
    );
    assert_has(&harness, "FuzzedDataProvider provider(data, size);");
    assert_has(
        &harness,
        "std::vector<uint8_t> buf = provider.ConsumeRemainingBytes<uint8_t>();",
    );
    assert_has(&harness, "parse_chunk(buf.data(), buf.size());");
    assert_has(&harness, "return 0;");
}

#[test]
fn c_headers_are_included_with_c_linkage() {
    let harness = harness_for(&[PNG_H, PNG_C], "parse_chunk");
    assert_has(&harness, "extern \"C\" {\n#include \"include/png.h\"\n}");
    assert_has(&harness, "#include <fuzzer/FuzzedDataProvider.h>");
    assert_has(&harness, "-fsanitize=fuzzer,address,undefined");
    assert_has(
        &harness,
        "//   int parse_chunk(const uint8_t *buf, size_t len)",
    );
}

#[test]
fn char_buffers_are_cast_and_numbers_come_first() {
    let harness = harness_for(
        &[(
            "x.cc",
            "int decode(int flags, const char *in, size_t n) { return 0; }",
        )],
        "decode",
    );
    assert_has(&harness, "int flags = provider.ConsumeIntegral<int>();");
    assert_has(
        &harness,
        "decode(flags, reinterpret_cast<const char *>(in.data()), in.size());",
    );
    let flags_at = harness.code.find("ConsumeIntegral").expect("flags");
    let input_at = harness.code.find("ConsumeRemainingBytes").expect("input");
    assert!(flags_at < input_at);
    assert!(!harness.code.contains("extern \"C\" {"));
}

#[test]
fn only_the_last_buffer_takes_the_rest_of_the_input() {
    let harness = harness_for(
        &[(
            "two.cc",
            "int diff(const uint8_t *a, size_t an, const char *b) { return 0; }",
        )],
        "diff",
    );
    assert_has(
        &harness,
        "std::vector<uint8_t> a = provider.ConsumeBytes<uint8_t>(\
         provider.ConsumeIntegralInRange<size_t>(0, provider.remaining_bytes()));",
    );
    assert_has(
        &harness,
        "std::string b = provider.ConsumeRemainingBytesAsString();",
    );
    assert_has(&harness, "diff(a.data(), a.size(), b.c_str());");
}

#[test]
fn methods_get_an_object_to_be_called_on() {
    let harness = harness_for(
        &[(
            "dec.h",
            "class Decoder { public: int feed(const std::string &s) { return 0; } };",
        )],
        "Decoder::feed",
    );
    assert_eq!(harness.file_name, "fuzz_feed.cc");
    assert_has(&harness, "#include \"dec.h\"");
    assert_has(
        &harness,
        "Decoder object{};  // TODO: construct a real Decoder",
    );
    assert_has(&harness, "object.feed(s);");
}

#[test]
fn static_functions_include_their_own_file() {
    let harness = harness_for(
        &[(
            "src/frame.c",
            "static int read_frame(const char *p) { return 0; }",
        )],
        "read_frame",
    );
    assert_has(&harness, "#include \"src/frame.c\"");
    assert_has(&harness, "leave src/frame.c out of the build");
    assert_has(&harness, "read_frame(p.c_str());");
}

#[test]
fn an_included_files_main_is_renamed_around_the_include() {
    // The file the harness compiles in may have a main() of its own, which
    // would clash with the harness's (or libFuzzer's).
    let tool = "static int parse(const char *s) { return s[0]; }\nint main(void) { return parse(\"x\"); }\n";
    for engine in Engine::ALL {
        let harness = drafted_for(&[("src/tool.c", tool)], "parse", engine);
        assert_has(
            &harness,
            "#define main project_main  // its main() would clash with the harness's\n\
             extern \"C\" {\n#include \"src/tool.c\"\n}\n#undef main\n",
        );
    }
}

#[test]
fn the_afl_harness_suggests_asan_options() {
    let harness = drafted_for(&[PNG_H, PNG_C], "parse_chunk", Engine::Afl);
    assert_has(
        &harness,
        "ASAN_OPTIONS=abort_on_error=1:symbolize=0:detect_leaks=0:\
         detect_stack_use_after_return=1:max_malloc_fill_size=1073741824",
    );
}

#[test]
fn without_a_header_the_prototype_is_declared() {
    let harness = harness_for(
        &[(
            "lib.c",
            "int checksum(const void *p, unsigned n) { return 0; }",
        )],
        "checksum",
    );
    assert_has(
        &harness,
        "extern \"C\" {\nint checksum(const void *p, unsigned n);\n}",
    );
    assert_has(&harness, "checksum(p.data(), p.size());");
}

#[test]
fn values_it_cannot_make_become_todos() {
    let harness = harness_for(
        &[(
            "c.c",
            "int load_with(struct ctx *c, const uint8_t *p, size_t n, size_t *used) { return 0; }",
        )],
        "load_with",
    );
    assert_has(&harness, "using c_type = struct ctx *;");
    assert_has(&harness, "c_type c{};  // TODO: set up c (struct ctx *)");
    assert_has(
        &harness,
        "size_t used = provider.ConsumeIntegral<size_t>();",
    );
    assert_has(&harness, "load_with(c, p.data(), p.size(), &used);");
    assert!(todo_count(&harness.code) >= 1);
}

#[test]
fn a_buffer_without_a_length_says_so() {
    let harness = harness_for(
        &[(
            "h.c",
            "int parse_hdr(const unsigned char *p) { return p[3]; }",
        )],
        "parse_hdr",
    );
    assert_has(&harness, "TODO: p has no length parameter");
    assert_has(
        &harness,
        "parse_hdr(reinterpret_cast<const unsigned char *>(p.data()));",
    );
}

#[test]
fn file_name_parameters_are_flagged() {
    let harness = harness_for(
        &[("f.c", "int parse_file(const char *path) { return 0; }")],
        "parse_file",
    );
    assert_has(&harness, "TODO: path is a file name");
}

#[test]
fn clashing_or_missing_names_are_replaced() {
    let harness = harness_for(
        &[(
            "n.c",
            "int scan(const char *data, int size, int, int provider) { return 0; }",
        )],
        "scan",
    );
    assert_has(&harness, "int arg3 = provider.ConsumeIntegral<int>();");
    assert_has(&harness, "int arg4 = provider.ConsumeIntegral<int>();");
    assert_has(
        &harness,
        "scan(reinterpret_cast<const char *>(arg1.data()), arg1.size(), arg3, arg4);",
    );
}

#[test]
fn keywords_repeats_and_clashing_names_are_not_used() {
    let params: Vec<Param> = ["new", "class", "ok", "ok", "bad$", "parse", "x_type"]
        .iter()
        .map(|name| Param {
            type_text: "int".into(),
            name: Some((*name).to_string()),
        })
        .collect();
    assert_eq!(
        variable_names(&params, &["parse"]),
        ["arg1", "arg2", "ok", "arg4", "arg5", "arg6", "arg7"]
    );
}

#[test]
fn a_header_path_that_could_break_out_is_not_written() {
    let harness = harness_for(
        &[
            ("inc/a\"b.h", "int parse(const char *s);"),
            ("bad\nname.c", "int parse(const char *s) { return 0; }"),
        ],
        "parse",
    );
    assert!(!harness.code.contains("a\"b.h"), "{}", harness.code);
    assert!(!harness.code.contains("bad\nname"), "{}", harness.code);
    assert_has(&harness, "TODO: #include the header that declares parse");
}

#[test]
fn a_type_that_could_break_out_is_not_written() {
    let graph = graph_of_files(&[("t.c", "int parse(struct s *p) { return 0; }")]);
    let mut function = graph.functions[0].clone();
    function.signature.params[0].type_text = "int; system(\"x\")".into();
    function.signature.prototype = "int parse(int x) \\".into();
    let harness = draft(&graph, &function, Engine::LibFuzzer).expect("a harness");
    assert!(!harness.code.contains("system"), "{}", harness.code);
    assert!(!harness.code.contains('\\'), "{}", harness.code);
    assert_has(&harness, "TODO: parameter 1's type couldn't be read");
}

#[test]
fn functions_without_parameters_need_no_provider() {
    let harness = harness_for(&[("z.c", "int parse_env(void) { return 0; }")], "parse_env");
    assert!(!harness.code.contains("FuzzedDataProvider provider"));
    assert_has(&harness, "parse_env();");
    assert_has(&harness, "TODO: parse_env takes no parameters");
}

#[test]
fn file_names_keep_only_identifier_characters() {
    assert_eq!(
        file_name_for("ns::Parse", Engine::LibFuzzer),
        "fuzz_Parse.cc"
    );
    assert_eq!(
        file_name_for("weird$name", Engine::LibFuzzer),
        "fuzz_weird_name.cc"
    );
    assert_eq!(file_name_for("", Engine::LibFuzzer), "fuzz_target.cc");
    assert_eq!(
        file_name_for(&"x".repeat(200), Engine::LibFuzzer).len(),
        "fuzz_.cc".len() + 64
    );
}

#[test]
fn a_copied_prototype_keeps_its_namespace() {
    let harness = harness_for(
        &[(
            "u.cc",
            "namespace util { int parse(const uint8_t *buf, size_t len) { return 0; } }",
        )],
        "util::parse",
    );
    assert_has(
        &harness,
        "namespace util {\nint parse(const uint8_t *buf, size_t len);\n}",
    );
    assert_has(&harness, "util::parse(buf.data(), buf.size());");
}

#[test]
fn an_out_of_line_definition_is_not_copied() {
    let harness = harness_for(
        &[(
            "o.cc",
            "namespace util { int parse(const char *s); }\nint util::parse(const char *s) { return 0; }",
        )],
        "util::parse",
    );
    assert!(!harness.code.contains("int util::parse(const char *s);"));
    assert_has(
        &harness,
        "TODO: #include the header that declares util::parse",
    );
}

#[test]
fn rvalue_references_are_moved_into_the_call() {
    let harness = harness_for(
        &[("m.cc", "int take(std::string &&text) { return 0; }")],
        "take",
    );
    assert_has(&harness, "take(std::move(text));");
    assert_has(&harness, "#include <utility>");
}

#[test]
fn an_included_c_file_may_not_compile_as_cpp() {
    let harness = harness_for(
        &[(
            "src/frame.c",
            "static int read_frame(const char *p) { return 0; }",
        )],
        "read_frame",
    );
    assert_has(&harness, "TODO: if src/frame.c doesn't compile as C++");
}

#[test]
fn templates_need_their_arguments_picked() {
    let harness = harness_for(
        &[(
            "t.cc",
            "template <typename T> int parse_as(const char *s) { return 0; }",
        )],
        "parse_as",
    );
    assert_has(&harness, "TODO: parse_as is a template");
}

#[test]
fn parameters_named_like_the_function_are_renamed() {
    let harness = harness_for(
        &[("p.c", "int parse(const char *parse) { return 0; }")],
        "parse",
    );
    assert_has(&harness, "parse(arg1.c_str());");
}

#[test]
fn comments_keep_only_printable_ascii() {
    assert_eq!(comment_safe("a\u{202E}b\\c\nd"), "a_b_c_d");
}

#[test]
fn prototypes_with_digraphs_or_trigraphs_are_not_copied() {
    assert!(is_safe_prototype("int f(const char *s, int n)"));
    assert!(!is_safe_prototype("int f(int a = 1 <% 2 %> 3)"));
    assert!(!is_safe_prototype("int f(int a??<)"));
    assert!(!is_safe_prototype("int f(int a<:2:>)"));
}

#[test]
fn lengths_passed_by_address_start_at_the_buffer_size() {
    let harness = harness_for(
        &[(
            "z.c",
            "int uncompress(unsigned char *dest, unsigned long *destLen, \
             const unsigned char *source, unsigned long sourceLen) { return 0; }",
        )],
        "uncompress",
    );
    assert_has(&harness, "unsigned long destLen = dest.size();");
    assert_has(
        &harness,
        "uncompress(reinterpret_cast<unsigned char *>(dest.data()), &destLen, \
         reinterpret_cast<const unsigned char *>(source.data()), source.size());",
    );
    let buffer_at = harness
        .code
        .find("std::vector<uint8_t> dest")
        .expect("dest");
    let length_at = harness.code.find("destLen = dest.size()").expect("destLen");
    assert!(buffer_at < length_at);
}

#[test]
fn an_afl_harness_runs_the_same_setup_in_persistent_mode() {
    let harness = drafted_for(&[PNG_H, PNG_C], "parse_chunk", Engine::Afl);
    let libfuzzer = harness_for(&[PNG_H, PNG_C], "parse_chunk");
    assert_eq!(harness.file_name, "fuzz_parse_chunk_afl.cc");
    assert!(!harness.code.contains("LLVMFuzzerTestOneInput"));
    assert_has(
        &harness,
        "static int fuzz_one_input(const uint8_t *data, size_t size) {",
    );
    let setup = |code: &str| {
        let start = code.find("  FuzzedDataProvider").expect("setup");
        code[start..start + code[start..].find("\n}").expect("end")].to_string()
    };
    assert_eq!(setup(&harness.code), setup(&libfuzzer.code));
    assert_has(&harness, "__AFL_FUZZ_INIT();");
    assert_has(&harness, "unsigned char *buf = __AFL_FUZZ_TESTCASE_BUF;");
    assert_has(&harness, "while (__AFL_LOOP(10000)) {");
    assert_has(&harness, "fuzz_one_input(buf, __AFL_FUZZ_TESTCASE_LEN);");
}

#[test]
fn an_afl_harness_says_how_to_build_and_run_it() {
    let harness = drafted_for(&[PNG_H, PNG_C], "parse_chunk", Engine::Afl);
    assert_has(
        &harness,
        "// AFL++ persistent-mode harness for parse_chunk (src/png.c:1)",
    );
    assert_has(
        &harness,
        "afl-clang-fast++ -g -O1 -I. fuzz_parse_chunk_afl.cc",
    );
    assert_has(&harness, "AFL_LLVM_CMPLOG=1");
    assert_has(
        &harness,
        "afl-fuzz -i seeds -o out -c ./fuzz_parse_chunk.cmplog -- ./fuzz_parse_chunk",
    );
    assert_has(&harness, "-x fuzz_parse_chunk.dict");
}

#[test]
fn built_without_afl_it_replays_one_input_from_stdin() {
    let harness = drafted_for(&[PNG_H, PNG_C], "parse_chunk", Engine::Afl);
    assert_has(
        &harness,
        "#ifdef __AFL_FUZZ_TESTCASE_LEN\n#include <unistd.h>",
    );
    assert_has(&harness, "#else");
    assert_has(&harness, "std::fread(chunk, 1, sizeof chunk, stdin)");
    assert_has(
        &harness,
        "return fuzz_one_input(input.data(), input.size());",
    );
    assert_has(&harness, "#include <cstdio>");
    assert_eq!(harness.code.matches("#include <vector>").count(), 1);
}

#[test]
fn both_engines_name_the_same_binary() {
    assert_eq!(stem_for("png::parse_chunk"), "fuzz_parse_chunk");
    assert_eq!(
        file_name_for("png::parse_chunk", Engine::LibFuzzer),
        "fuzz_parse_chunk.cc"
    );
    assert_eq!(
        file_name_for("png::parse_chunk", Engine::Afl),
        "fuzz_parse_chunk_afl.cc"
    );
}

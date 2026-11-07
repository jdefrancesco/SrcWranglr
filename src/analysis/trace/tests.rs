use super::*;
use crate::analysis::reach::COMMAND_LINE;
use crate::analysis::{Finding, Severity};
use crate::index::graph_of_files;

/// A finding on the first `needle` in `file`, as the risky-call check
/// reports one: at the call's name.
fn finding_at(graph: &CodeGraph, file: &str, needle: &str) -> Finding {
    let (line, column) = position(graph, file, needle);
    Finding {
        tool: "test".into(),
        rule: needle.into(),
        severity: Severity::High,
        message: String::new(),
        fix: None,
        cwe: None,
        file: file.into(),
        line,
        columns: Some((column, column + 1)),
        symbol: None,
    }
}

/// 1-based line and byte column of the first `needle` in `file`.
pub(super) fn position(graph: &CodeGraph, file: &str, needle: &str) -> (u32, u32) {
    let source = &graph.file_contents[file];
    let at = source.find(needle).unwrap_or_else(|| panic!("no {needle}"));
    let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = source[..at].matches('\n').count() + 1;
    (line as u32, (at - line_start) as u32)
}

fn lines(path: &TracePath) -> Vec<u32> {
    path.steps.iter().map(|s| s.line).collect()
}

fn notes(path: &TracePath) -> Vec<&str> {
    path.steps.iter().map(|s| s.note.as_str()).collect()
}

fn trace_of(sources: &[(&str, &str)], file: &str, sink: &str) -> Trace {
    let graph = graph_of_files(sources);
    trace_finding(&graph, &finding_at(&graph, file, sink)).expect("a trace")
}

fn stopped(trace: &Trace) -> Vec<&str> {
    trace
        .paths
        .iter()
        .filter_map(|p| match &p.endpoint {
            Endpoint::Stopped(why) => Some(why.as_str()),
            Endpoint::Input(_) | Endpoint::Sink(_) => None,
        })
        .collect()
}

const USER: &str = "char *trim(char *s) {\n\
    while (*s == ' ') s++;\n\
    return s;\n\
}\n\
void load_user(const char *raw) {\n\
    char buf[16];\n\
    char *name = trim(raw);\n\
    strcpy(buf, name);\n\
}\n\
int main(int argc, char **argv) {\n\
    load_user(argv[1]);\n\
    return 0;\n\
}\n";

#[test]
fn follows_argv_through_a_call_and_a_return_into_strcpy() {
    let trace = trace_of(&[("user.c", USER)], "user.c", "strcpy");
    assert_eq!(trace.subject, "strcpy(buf, name)");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    let path = &trace.paths[0];
    assert_eq!(path.endpoint, Endpoint::Input(COMMAND_LINE.into()));
    assert_eq!(lines(path), [10, 11, 7, 3, 7, 8]);
    assert_eq!(
        notes(path),
        [
            "`argv` is the command line",
            "passes `argv[1]` as `raw`",
            "passes `raw` as `s`",
            "`trim()` returns `s`",
            "`name` set from `trim(raw)`",
            "the risky call to `strcpy()`",
        ]
    );
    assert!(path.steps.iter().all(|s| s.symbol.is_some()));
    assert_eq!(path.steps[4].code, "char *name = trim(raw);");
}

#[test]
fn a_step_marks_the_traced_name_on_its_line() {
    let graph = graph_of_files(&[("user.c", USER)]);
    let trace = trace_finding(&graph, &finding_at(&graph, "user.c", "strcpy")).expect("a trace");
    let step = &trace.paths[0].steps[4];
    let span = step.span.expect("one line");
    let line = USER.lines().nth(span.line as usize - 1).expect("line");
    assert_eq!(&line[span.start as usize..span.end as usize], "name");
}

#[test]
fn a_buffer_fgets_filled_is_input() {
    let source = "void greet(char *dst) {\n\
        char line[64];\n\
        fgets(line, sizeof line, stdin);\n\
        strcpy(dst, line);\n\
    }\n";
    let trace = trace_of(&[("g.c", source)], "g.c", "strcpy");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    assert_eq!(trace.paths[0].endpoint, Endpoint::Input("fgets()".into()));
    assert_eq!(lines(&trace.paths[0]), [3, 4]);
    assert_eq!(
        trace.paths[0].steps[0].note,
        "`fgets()` reads outside input into `line`"
    );
}

#[test]
fn what_getenv_returns_is_input_and_format_strings_are_skipped() {
    let source = "void home(char *out) {\n\
        const char *h = getenv(\"HOME\");\n\
        sprintf(out, \"%s/.cfg\", h);\n\
    }\n";
    let trace = trace_of(&[("h.c", source)], "h.c", "sprintf");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    assert_eq!(trace.paths[0].endpoint, Endpoint::Input("getenv()".into()));
    assert_eq!(lines(&trace.paths[0]), [2, 3]);
}

#[test]
fn says_where_it_stops() {
    let source = "void copy(char *d, const char *s) {\n\
        int n = 16;\n\
        memcpy(d, s, n);\n\
    }\n";
    let trace = trace_of(&[("c.c", source)], "c.c", "memcpy");
    let mut why = stopped(&trace);
    why.sort_unstable();
    assert_eq!(
        why,
        [
            "`s` is a parameter of `copy`, and nothing indexed calls it",
            "set to a constant",
        ]
    );
}

#[test]
fn a_later_assignment_hides_an_earlier_one() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        char *p = argv[1];\n\
        p = \"fixed\";\n\
        strcpy(d, p);\n\
    }\n";
    let trace = trace_of(&[("m.c", source)], "m.c", "strcpy");
    assert_eq!(stopped(&trace), ["set to a constant"], "{trace:#?}");
    assert_eq!(trace.paths.len(), 1);
}

#[test]
fn an_assignment_in_a_branch_keeps_the_earlier_one() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        char *p = \"fixed\";\n\
        if (argc > 1) p = argv[1];\n\
        strcpy(d, p);\n\
    }\n";
    let trace = trace_of(&[("m.c", source)], "m.c", "strcpy");
    assert_eq!(trace.paths.len(), 2, "{trace:#?}");
    assert_eq!(
        trace.paths[0].endpoint,
        Endpoint::Input(COMMAND_LINE.into())
    );
    assert_eq!(lines(&trace.paths[0]), [1, 4, 5]);
    assert_eq!(stopped(&trace), ["set to a constant"]);
}

#[test]
fn follows_a_buffer_into_the_function_that_fills_it() {
    let source = "void read_name(char *out) {\n\
        fgets(out, 64, stdin);\n\
    }\n\
    void hello(char *g) {\n\
        char name[64];\n\
        read_name(name);\n\
        strcpy(g, name);\n\
    }\n";
    let trace = trace_of(&[("r.c", source)], "r.c", "strcpy(g");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    let path = &trace.paths[0];
    assert_eq!(path.endpoint, Endpoint::Input("fgets()".into()));
    assert_eq!(lines(path), [2, 6, 7]);
    assert_eq!(
        path.steps[1].note,
        "`name` passed to `read_name()`, which may write into it"
    );
}

#[test]
fn follows_a_struct_its_field_is_filled_through() {
    let source = "struct req { char name[32]; };\n\
    void parse(struct req *r) { fgets(r->name, 32, stdin); }\n\
    void handle(char *d) {\n\
        struct req q;\n\
        parse(&q);\n\
        strcpy(d, q.name);\n\
    }\n";
    let trace = trace_of(&[("s.c", source)], "s.c", "strcpy");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    assert_eq!(trace.paths[0].endpoint, Endpoint::Input("fgets()".into()));
    assert_eq!(lines(&trace.paths[0]), [2, 5, 6]);
}

#[test]
fn a_global_stops_the_trace() {
    let source = "char g_name[32];\nvoid f(char *d) { strcpy(d, g_name); }\n";
    let trace = trace_of(&[("g.c", source)], "g.c", "strcpy");
    assert_eq!(
        stopped(&trace),
        ["`g_name` isn't set in `f`: a global, a member or a macro, set elsewhere (not followed)"]
    );
}

#[test]
fn what_a_library_call_returns_carries_its_arguments() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        int n = atoi(argv[1]);\n\
        memcpy(d, argv[2], n);\n\
        return 0;\n\
    }\n";
    let trace = trace_of(&[("a.c", source)], "a.c", "memcpy");
    assert_eq!(trace.paths.len(), 2, "{trace:#?}");
    assert!(trace.paths.iter().all(|p| p.endpoint.is_input()));
    assert_eq!(lines(&trace.paths[1]), [1, 3, 4]);
}

#[test]
fn a_method_result_comes_from_its_object() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        std::string s = argv[1];\n\
        strcpy(d, s.c_str());\n\
        return 0;\n\
    }\n";
    let trace = trace_of(&[("s.cpp", source)], "s.cpp", "strcpy");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    assert!(trace.paths[0].endpoint.is_input());
    assert_eq!(lines(&trace.paths[0]), [1, 3, 4]);
}

#[test]
fn a_read_in_an_if_condition_always_happens() {
    let source = "void serve(int fd, char *d) {\n\
        char buf[64];\n\
        int n;\n\
        if ((n = read(fd, buf, sizeof buf)) > 0)\n\
            strcpy(d, buf);\n\
    }\n";
    let trace = trace_of(&[("n.c", source)], "n.c", "strcpy");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    assert_eq!(trace.paths[0].endpoint, Endpoint::Input("read()".into()));
    assert_eq!(lines(&trace.paths[0]), [4, 5]);
}

#[test]
fn std_cin_is_input() {
    let source = "void ask(char *d) {\n\
        std::string answer;\n\
        std::cin >> answer;\n\
        strcpy(d, answer.c_str());\n\
    }\n";
    let trace = trace_of(&[("q.cpp", source)], "q.cpp", "strcpy");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    assert_eq!(trace.paths[0].endpoint, Endpoint::Input("std::cin".into()));
}

#[test]
fn a_return_goes_back_to_the_call_it_came_from() {
    let source = "char *id(char *s) { return s; }\n\
    void f(char **argv, char *d) {\n\
        char *a = id(\"const\");\n\
        char *b = id(argv[1]);\n\
        strcpy(d, a);\n\
    }\n";
    let trace = trace_of(&[("i.c", source)], "i.c", "strcpy");
    assert_eq!(stopped(&trace), ["set to a constant"], "{trace:#?}");
    assert_eq!(trace.paths.len(), 1);
}

#[test]
fn recursion_ends() {
    let source = "void walk(char *d, char *p) {\n\
        strcpy(d, p);\n\
        walk(d, p);\n\
    }\n";
    let trace = trace_of(&[("w.c", source)], "w.c", "strcpy");
    assert!(trace.paths.iter().all(|p| !p.endpoint.is_input()));
}

#[test]
fn follows_calls_across_files() {
    let lib = "void store(const char *v) {\n    char b[8];\n    strcpy(b, v);\n}\n";
    let app = "int main(int argc, char **argv) {\n    store(argv[1]);\n}\n";
    let trace = trace_of(&[("lib.c", lib), ("app.c", app)], "lib.c", "strcpy");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    let files: Vec<&str> = trace.paths[0]
        .steps
        .iter()
        .map(|s| s.file.as_str())
        .collect();
    assert_eq!(files, ["app.c", "app.c", "lib.c"]);
}

#[test]
fn traces_a_variable_picked_in_the_code() {
    let graph = graph_of_files(&[("user.c", USER)]);
    let (line, column) = position(&graph, "user.c", "name);");
    let trace = trace_variable(&graph, "user.c", line, column).expect("a trace");
    assert_eq!(trace.subject, "`name`");
    assert_eq!(lines(&trace.paths[0]), [10, 11, 7, 3, 7, 8]);
    assert_eq!(trace.paths[0].steps[5].note, "`name` here");
}

#[test]
fn tracing_a_name_where_it_is_set_starts_at_that_line() {
    let graph = graph_of_files(&[("user.c", USER)]);
    let (line, column) = position(&graph, "user.c", "name =");
    let trace = trace_variable(&graph, "user.c", line, column).expect("a trace");
    assert_eq!(lines(&trace.paths[0]), [10, 11, 7, 3, 7]);
}

#[test]
fn nothing_to_trace_outside_a_variable() {
    let graph = graph_of_files(&[("user.c", USER)]);
    let (line, column) = position(&graph, "user.c", "return 0");
    assert!(trace_variable(&graph, "user.c", line, column).is_none());
}

#[test]
fn input_paths_come_first() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        int n = 4;\n\
        memcpy(d, argv[1], n);\n\
    }\n";
    let trace = trace_of(&[("o.c", source)], "o.c", "memcpy");
    assert!(trace.paths[0].endpoint.is_input(), "{trace:#?}");
    assert!(!trace.paths[1].endpoint.is_input());
}

#[test]
fn a_call_belongs_to_the_definition_in_its_own_file() {
    let a = "static void makedir(const char *d) {\n    char b[8];\n    strcpy(b, d);\n}\n\
        void fa(void) { makedir(\"fixed\"); }\n";
    let b = "void makedir(char *d) { }\nint main(int argc, char **argv) { makedir(argv[1]); }\n";
    let trace = trace_of(&[("a.c", a), ("b.c", b)], "a.c", "strcpy");
    assert_eq!(stopped(&trace), ["set to a constant"], "{trace:#?}");
    assert_eq!(trace.paths.len(), 1);
}

#[test]
fn a_result_comes_from_the_definition_the_call_can_reach() {
    let a = "static char *get(char **v) { return \"fixed\"; }\n\
        int main(int argc, char **argv) { char d[8]; strcpy(d, get(argv)); }\n";
    let b = "char *get(char **v) { return v[1]; }\n";
    let trace = trace_of(&[("a.c", a), ("b.c", b)], "a.c", "strcpy");
    assert_eq!(stopped(&trace), ["set to a constant"], "{trace:#?}");
    assert_eq!(trace.paths.len(), 1);
}

#[test]
fn deeply_nested_code_is_cut_short_instead_of_overflowing() {
    let depth = 1_000;
    let calls = format!("{}a{}", "g(".repeat(depth), ")".repeat(depth));
    let chain = format!("s{}", ".c_str()".repeat(depth));
    for value in [calls, chain] {
        let source = format!("void f(char *d, char *a) {{\n    strcpy(d, {value});\n}}\n");
        let trace = trace_of(&[("deep.c", &source)], "deep.c", "strcpy");
        assert!(trace.is_cut_short);
    }
}

#[test]
fn a_default_set_in_a_branch_keeps_the_callers_value() {
    let source = "void use_name(char *p) {\n\
        char b[8];\n\
        if (!p) p = \"guest\";\n\
        strcpy(b, p);\n\
    }\n\
    int main(int argc, char **argv) { use_name(argv[1]); return 0; }\n";
    let trace = trace_of(&[("d.c", source)], "d.c", "strcpy");
    assert_eq!(trace.paths.len(), 2, "{trace:#?}");
    assert_eq!(
        trace.paths[0].endpoint,
        Endpoint::Input(COMMAND_LINE.into())
    );
    assert_eq!(stopped(&trace), ["set to a constant"]);
}

#[test]
fn the_other_arm_of_an_if_does_not_reach() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        char *x = \"k\";\n\
        if (argc > 2) { x = argv[1]; } else { strcpy(d, x); }\n\
        return 0;\n\
    }\n";
    let trace = trace_of(&[("e.c", source)], "e.c", "strcpy");
    assert_eq!(stopped(&trace), ["set to a constant"], "{trace:#?}");
    assert_eq!(trace.paths.len(), 1);
}

#[test]
fn the_other_arm_does_reach_on_the_next_pass_of_a_loop() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        char *x = \"k\";\n\
        for (int i = 1; i < argc; i++) {\n\
            if (i % 2) { x = argv[i]; } else { strcpy(d, x); }\n\
        }\n\
    }\n";
    let trace = trace_of(&[("l.c", source)], "l.c", "strcpy");
    assert!(
        trace.paths.iter().any(|p| p.endpoint.is_input()),
        "{trace:#?}"
    );
}

#[test]
fn a_variable_in_a_closed_block_is_another_variable() {
    let source = "void f(char *p) {\n\
        char d[8];\n\
        { char *p = \"inner\"; p = \"inner2\"; }\n\
        strcpy(d, p);\n\
    }\n\
    int main(int argc, char **argv) { f(argv[1]); }\n";
    let trace = trace_of(&[("b.c", source)], "b.c", "strcpy");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    assert_eq!(
        trace.paths[0].endpoint,
        Endpoint::Input(COMMAND_LINE.into())
    );
}

#[test]
fn a_later_write_in_a_loop_reaches_an_earlier_use() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        char *p = \"k\";\n\
        while (argc-- > 1) {\n\
            strcpy(d, p);\n\
            p = argv[argc];\n\
        }\n\
    }\n";
    let trace = trace_of(&[("w.c", source)], "w.c", "strcpy");
    assert_eq!(trace.paths.len(), 2, "{trace:#?}");
    assert!(trace.paths[0].endpoint.is_input());
    assert_eq!(stopped(&trace), ["set to a constant"]);
}

#[test]
fn a_for_update_runs_only_after_the_body() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        char *p;\n\
        for (p = \"start\"; *p; p = argv[1]) strcpy(d, p);\n\
    }\n";
    let trace = trace_of(&[("f.c", source)], "f.c", "strcpy");
    assert_eq!(trace.paths.len(), 2, "{trace:#?}");
    assert_eq!(stopped(&trace), ["set to a constant"]);
}

#[test]
fn too_many_writes_are_cut_short() {
    let writes: String = (0..10)
        .map(|i| format!("    if (c) p = \"{i}\";\n"))
        .collect();
    let source =
        format!("void f(int c, char *d) {{\n    char *p = \"a\";\n{writes}    strcpy(d, p);\n}}\n");
    let trace = trace_of(&[("m.c", &source)], "m.c", "strcpy");
    assert!(trace.is_cut_short, "{trace:#?}");
}

#[test]
fn a_left_out_default_argument_says_so() {
    let source = "void put(char *d, const char *s = \"dflt\") { strcpy(d, s); }\n\
        void g(char *d) { put(d); }\n";
    let trace = trace_of(&[("p.cpp", source)], "p.cpp", "strcpy");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    assert!(stopped(&trace)[0].contains("default"), "{trace:#?}");
}

#[test]
fn a_buffer_handed_to_a_function_that_never_writes_it_still_has_no_value() {
    let source = "void look(char *b) { }\n\
        void f(char *d) {\n\
            char b[8];\n\
            look(b);\n\
            strcpy(d, b);\n\
        }\n";
    let trace = trace_of(&[("n.c", source)], "n.c", "strcpy");
    assert_eq!(
        stopped(&trace),
        ["`b` has no value before this"],
        "{trace:#?}"
    );
}

#[test]
fn a_write_to_another_field_is_not_the_traced_field() {
    let source = "struct s { char *name; int len; };\n\
    int main(int argc, char **argv) {\n\
        char d[8];\n\
        struct s v;\n\
        v.len = atoi(argv[2]);\n\
        if (argc > 3) v.name = \"fixed\";\n\
        strcpy(d, v.name);\n\
    }\n";
    let trace = trace_of(&[("v.c", source)], "v.c", "strcpy");
    assert!(
        trace.paths.iter().all(|p| !p.endpoint.is_input()),
        "{trace:#?}"
    );
    let mut why = stopped(&trace);
    why.sort_unstable();
    assert_eq!(why, ["`v` has no value before this", "set to a constant"]);
}

#[test]
fn a_field_set_by_the_caller_is_found_through_the_struct() {
    let source = "struct s { char *name; int len; };\n\
    void use(struct s *q, int c, char *d) {\n\
        if (c) q->name = \"x\";\n\
        strcpy(d, q->name);\n\
    }\n\
    int main(int argc, char **argv) {\n\
        struct s v;\n\
        char d[8];\n\
        v.len = 4;\n\
        v.name = argv[1];\n\
        use(&v, argc, d);\n\
    }\n";
    let trace = trace_of(&[("u.c", source)], "u.c", "strcpy");
    assert_eq!(trace.paths.len(), 2, "{trace:#?}");
    assert_eq!(
        trace.paths[0].endpoint,
        Endpoint::Input(COMMAND_LINE.into())
    );
    assert!(trace.paths[0]
        .steps
        .iter()
        .all(|s| !s.code.contains("v.len")));
    assert_eq!(stopped(&trace), ["set to a constant"]);
}

fn lines_of(trace: &Trace) -> Vec<Vec<u32>> {
    trace.paths.iter().map(lines).collect()
}

#[test]
fn a_break_can_skip_an_assignment() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        char *p = argv[1];\n\
        do { if (argc) break; p = \"k\"; } while (0);\n\
        strcpy(d, p);\n\
    }\n";
    let trace = trace_of(&[("j.c", source)], "j.c", "strcpy");
    assert_eq!(trace.paths.len(), 2, "{trace:#?}");
    assert!(trace.paths[0].endpoint.is_input());
}

#[test]
fn a_goto_can_skip_an_assignment() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        char *p = argv[1];\n\
        if (argc > 1) goto out;\n\
        p = \"k\";\n\
    out:\n\
        strcpy(d, p);\n\
    }\n";
    let trace = trace_of(&[("g.c", source)], "g.c", "strcpy");
    assert_eq!(trace.paths.len(), 2, "{trace:#?}");
    assert!(trace.paths[0].endpoint.is_input());
}

#[test]
fn a_whole_struct_write_replaces_its_fields() {
    let source = "struct S { char *x; };\n\
    int main(int argc, char **argv) {\n\
        char d[8];\n\
        struct S s;\n\
        s.x = \"k\";\n\
        s = parse(argv[1]);\n\
        strcpy(d, s.x);\n\
    }\n";
    let trace = trace_of(&[("s.c", source)], "s.c", "strcpy");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    assert!(trace.paths[0].endpoint.is_input());
}

#[test]
fn clearing_a_struct_replaces_its_fields() {
    let source = "struct S { char *x; };\n\
    void f(struct S *p, char **argv, char *d) {\n\
        p->x = argv[1];\n\
        memset(p, 0, sizeof *p);\n\
        strcpy(d, p->x);\n\
    }\n";
    let trace = trace_of(&[("c.c", source)], "c.c", "strcpy");
    assert_eq!(stopped(&trace), ["`p` is cleared here"], "{trace:#?}");
}

#[test]
fn a_variable_declared_in_a_loop_is_new_on_each_pass() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        for (int i = 0; i < 2; i++) { char *p = \"k\"; strcpy(d, p); p = argv[1]; }\n\
    }\n";
    let trace = trace_of(&[("r.c", source)], "r.c", "strcpy");
    assert_eq!(stopped(&trace), ["set to a constant"], "{trace:#?}");
    assert_eq!(trace.paths.len(), 1);
}

#[test]
fn an_outer_loop_carries_a_later_write_too() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        char *p = \"k\";\n\
        while (argc) { while (argc > 1) { strcpy(d, p); argc--; } p = argv[1]; argc--; }\n\
    }\n";
    let trace = trace_of(&[("o.c", source)], "o.c", "strcpy");
    assert_eq!(trace.paths.len(), 2, "{trace:#?}");
    assert!(trace.paths[0].endpoint.is_input());
}

#[test]
fn tracing_a_write_itself_ignores_the_loops_later_writes() {
    let source = "int main(int argc, char **argv) {\n\
        char *p, *q;\n\
        while (argc--) { p = \"a\"; q = p; p = argv[1]; }\n\
    }\n";
    let graph = graph_of_files(&[("t.c", source)]);
    let (line, column) = position(&graph, "t.c", "p = \"a\"");
    let trace = trace_variable(&graph, "t.c", line, column).expect("a trace");
    assert_eq!(stopped(&trace), ["set to a constant"], "{trace:#?}");
    assert_eq!(trace.paths.len(), 1);
}

#[test]
fn a_declaration_in_a_condition_has_its_value() {
    let source = "void f(char *d) {\n\
        if (char *p = getenv(\"X\")) strcpy(d, p);\n\
    }\n";
    let trace = trace_of(&[("i.cpp", source)], "i.cpp", "strcpy");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    assert_eq!(trace.paths[0].endpoint, Endpoint::Input("getenv()".into()));
}

#[test]
fn a_field_written_in_between_replaces_the_callers() {
    let source = "typedef struct T { char *name; } T;\n\
    void g(T *t, char *d) { strcpy(d, t->name); }\n\
    void h(T *q, char *d, char **argv) { q->name = argv[1]; g(q, d); }\n\
    int main(int argc, char **v) { T t; char dd[8]; t.name = \"zz\"; h(&t, dd, v); }\n";
    let trace = trace_of(&[("h.c", source)], "h.c", "strcpy");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    assert!(trace.paths[0].endpoint.is_input());
}

#[test]
fn a_lambdas_parameter_is_another_variable() {
    let source = "void f(char *d, char *p) {\n\
        auto g = [](char *p) { p = \"a\"; };\n\
        strcpy(d, p);\n\
    }\n\
    int main(int argc, char **argv) { char d[8]; f(d, argv[1]); }\n";
    let trace = trace_of(&[("l.cpp", source)], "l.cpp", "strcpy");
    assert_eq!(trace.paths.len(), 1, "{trace:#?}");
    assert!(trace.paths[0].endpoint.is_input());
}

#[test]
fn each_write_into_a_buffer_is_its_own_path() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        char b[8];\n\
        b[0] = argv[1][0];\n\
        b[1] = 'x';\n\
        b[2] = 'x';\n\
        b[3] = 'x';\n\
        strcpy(d, b);\n\
    }\n";
    let trace = trace_of(&[("x.c", source)], "x.c", "strcpy");
    assert!(!trace.is_cut_short, "{:?}", lines_of(&trace));
    // From input, and the three constants, each once.
    assert_eq!(trace.paths.len(), 4, "{:?}", lines_of(&trace));
    assert!(trace.paths[0].endpoint.is_input());
}

#[test]
fn a_loops_start_runs_once() {
    let source = "int main(int argc, char **argv) {\n\
        char d[8];\n\
        char *p = \"k\";\n\
        for (char *q = p; *q; q++) { strcpy(d, q); p = argv[1]; }\n\
    }\n";
    let trace = trace_of(&[("q.c", source)], "q.c", "strcpy");
    assert_eq!(stopped(&trace), ["set to a constant"], "{trace:#?}");
    assert_eq!(trace.paths.len(), 1);
}

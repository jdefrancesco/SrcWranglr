//! Tracing a value forward, to the risky calls it reaches.

use super::tests::position;
use super::*;
use crate::analysis::dangerous::DangerousList;
use crate::analysis::risky::risky_calls;
use crate::index::graph_of_files;

/// Traces forward from the first `needle` in `file`, to the risky calls
/// the built-in check finds.
fn forward_of(sources: &[(&str, &str)], file: &str, needle: &str) -> Trace {
    try_forward(sources, file, needle).expect("a trace")
}

fn try_forward(sources: &[(&str, &str)], file: &str, needle: &str) -> Option<Trace> {
    let graph = graph_of_files(sources);
    let sinks: Vec<Sink> = risky_calls(&graph, &DangerousList::default())
        .iter()
        .map(Sink::of)
        .collect();
    let (line, column) = position(&graph, file, needle);
    trace_forward(&graph, &sinks, file, line, column)
}

/// The paths that reach a risky call.
fn to_sinks(trace: &Trace) -> Vec<&TracePath> {
    trace
        .paths
        .iter()
        .filter(|p| p.endpoint.is_sink())
        .collect()
}

fn lines(path: &TracePath) -> Vec<u32> {
    path.steps.iter().map(|s| s.line).collect()
}

/// The line each risky call reached is on.
fn sink_lines(trace: &Trace) -> Vec<u32> {
    let mut found: Vec<u32> = to_sinks(trace)
        .iter()
        .filter_map(|p| p.steps.last().map(|s| s.line))
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

const LOAD: &str = "void load_user(const char *raw) {\n\
    char buf[16];\n\
    strcpy(buf, raw);\n\
}\n\
int main(int argc, char **argv) {\n\
    load_user(argv[1]);\n\
}\n";

#[test]
fn follows_argv_through_a_call_into_strcpy() {
    let trace = forward_of(&[("user.c", LOAD)], "user.c", "argv[1]");
    assert_eq!(trace.direction, Direction::Forward);
    assert_eq!(trace.subject, "`argv`");
    let found = to_sinks(&trace);
    assert_eq!(found.len(), 1, "{trace:#?}");
    assert_eq!(lines(found[0]), [6, 6, 3]);
    assert_eq!(
        found[0].endpoint,
        Endpoint::Sink("`strcpy()` (high)".into())
    );
    let notes: Vec<&str> = found[0].steps.iter().map(|s| s.note.as_str()).collect();
    assert!(notes[1].contains("`load_user()` as `raw`"), "{notes:?}");
    assert!(notes[2].contains("`strcpy()`"), "{notes:?}");
}

#[test]
fn a_step_marks_the_name_the_value_is_in() {
    let trace = forward_of(&[("user.c", LOAD)], "user.c", "argv[1]");
    let last = to_sinks(&trace)[0].steps.last().cloned().expect("a step");
    let span = last.span.expect("a span");
    assert_eq!((span.line, span.start, span.end), (3, 12, 15), "`raw`");
}

#[test]
fn picking_a_parameter_follows_what_comes_in() {
    let trace = forward_of(&[("user.c", LOAD)], "user.c", "argv)");
    let found = to_sinks(&trace);
    assert_eq!(found.len(), 1, "{trace:#?}");
    assert_eq!(lines(found[0]), [5, 6, 3]);
    assert!(found[0].steps[0].note.contains("command line"));
}

#[test]
fn a_copy_carries_it_on_to_a_later_risky_call() {
    let code = "int main(int argc, char **argv) {\n\
        char name[64];\n\
        char out[16];\n\
        strncpy(name, argv[1], 63);\n\
        strcpy(out, name);\n\
    }\n";
    let trace = forward_of(&[("main.c", code)], "main.c", "argv[1]");
    assert!(sink_lines(&trace).contains(&5), "{trace:#?}");
    let later = to_sinks(&trace)
        .into_iter()
        .find(|p| p.steps.last().is_some_and(|s| s.line == 5))
        .expect("a path to line 5");
    assert_eq!(lines(later), [4, 4, 5]);
    assert!(
        later.steps[1].note.contains("`strncpy()` copies"),
        "{later:#?}"
    );
}

#[test]
fn a_length_worked_out_from_it_reaches_a_copy_too() {
    let code = "void f(const char *s) {\n\
        char buf[8];\n\
        size_t n = strlen(s);\n\
        memcpy(buf, s, n);\n\
    }\n";
    let trace = forward_of(&[("f.c", code)], "f.c", "s) {");
    let found = to_sinks(&trace);
    assert_eq!(found.len(), 2, "{trace:#?}");
    assert!(found
        .iter()
        .all(|p| p.steps.last().is_some_and(|s| s.line == 4)));
    let through_n = found
        .iter()
        .find(|p| p.steps.iter().any(|s| s.line == 3))
        .expect("a path through `n`");
    assert!(through_n.steps[1].note.contains("`n`"), "{through_n:#?}");
}

#[test]
fn a_later_write_stops_it() {
    let code = "void f(char *s) {\n\
        char buf[8];\n\
        char *p = s;\n\
        p = \"safe\";\n\
        strcpy(buf, p);\n\
    }\n";
    let trace = forward_of(&[("f.c", code)], "f.c", "s;");
    assert!(to_sinks(&trace).is_empty(), "{trace:#?}");
}

#[test]
fn a_write_in_a_branch_keeps_it() {
    let code = "void f(char *s, int c) {\n\
        char buf[8];\n\
        char *p = s;\n\
        if (c) p = \"safe\";\n\
        strcpy(buf, p);\n\
    }\n";
    let trace = forward_of(&[("f.c", code)], "f.c", "s;");
    assert_eq!(sink_lines(&trace), [5], "{trace:#?}");
}

#[test]
fn a_return_goes_back_to_its_caller() {
    let code = "char *dup(const char *s) { return strdup(s); }\n\
        int main(int argc, char **argv) {\n\
        char buf[8];\n\
        char *copy = dup(argv[1]);\n\
        strcpy(buf, copy);\n\
    }\n";
    let trace = forward_of(&[("main.c", code)], "main.c", "argv[1]");
    let found = to_sinks(&trace);
    assert_eq!(found.len(), 1, "{trace:#?}");
    assert_eq!(lines(found[0]), [4, 4, 1, 4, 5]);
}

#[test]
fn a_return_goes_back_only_to_the_call_it_came_from() {
    let code = "char *same(char *s) { return s; }\n\
        void other(void) { char b[4]; strcpy(b, same(\"const\")); }\n\
        int main(int argc, char **argv) {\n\
        char *v = same(argv[1]);\n\
        puts(v);\n\
    }\n";
    let trace = forward_of(&[("main.c", code)], "main.c", "argv[1]");
    assert!(to_sinks(&trace).is_empty(), "{trace:#?}");
}

#[test]
fn starting_in_a_function_its_result_goes_to_every_caller() {
    let code = "char *same(char *s) {\n\
        return s;\n\
    }\n\
    void one(char *x) { char b[4]; strcpy(b, same(x)); }\n\
    void two(char *y) { char b[4]; sprintf(b, \"%s\", same(y)); }\n";
    let trace = forward_of(&[("main.c", code)], "main.c", "s;");
    assert_eq!(sink_lines(&trace), [4, 5], "{trace:#?}");
}

#[test]
fn a_function_writing_into_its_out_parameter_taints_the_callers_buffer() {
    let code = "void copy_in(char *dst, const char *src) { strcpy(dst, src); }\n\
        int main(int argc, char **argv) {\n\
        char a[64];\n\
        char b[8];\n\
        copy_in(a, argv[1]);\n\
        strcpy(b, a);\n\
    }\n";
    let trace = forward_of(&[("main.c", code)], "main.c", "argv[1]");
    assert_eq!(sink_lines(&trace), [1, 6], "{trace:#?}");
}

#[test]
fn picking_a_buffer_where_input_is_read_follows_it() {
    let code = "int main(void) {\n\
        char line[256];\n\
        char out[16];\n\
        fgets(line, sizeof line, stdin);\n\
        strcpy(out, line);\n\
    }\n";
    let trace = forward_of(&[("main.c", code)], "main.c", "line, sizeof");
    assert_eq!(sink_lines(&trace), [5], "{trace:#?}");
}

#[test]
fn a_field_set_from_it_is_followed_through_the_struct() {
    let code = "struct req { char *name; char *cmd; };\n\
        void handle(struct req *r) { char b[8]; strcpy(b, r->name); system(r->cmd); }\n\
        int main(int argc, char **argv) {\n\
        struct req q;\n\
        q.name = argv[1];\n\
        q.cmd = \"ls\";\n\
        handle(&q);\n\
    }\n";
    let trace = forward_of(&[("main.c", code)], "main.c", "argv[1]");
    let found = to_sinks(&trace);
    assert_eq!(found.len(), 1, "{trace:#?}");
    assert!(found[0]
        .steps
        .last()
        .is_some_and(|s| s.code.contains("strcpy")));
}

#[test]
fn storing_it_in_a_global_says_where_it_stops() {
    let code = "char *g_name;\n\
        void keep(char *s) {\n\
        g_name = s;\n\
    }\n";
    let trace = forward_of(&[("g.c", code)], "g.c", "s) {");
    let stops: Vec<String> = trace
        .paths
        .iter()
        .filter(|p| !p.endpoint.is_sink())
        .map(|p| p.endpoint.label())
        .collect();
    assert!(stops.iter().any(|s| s.contains("`g_name`")), "{trace:#?}");
}

#[test]
fn a_condition_or_comparison_carries_nothing() {
    let code = "void f(const char *s) {\n\
        int big = strlen(s) > 4;\n\
        char *cmd = big ? \"a\" : \"b\";\n\
        if (s[0] == 'x') system(\"ls\");\n\
        system(cmd);\n\
    }\n";
    let trace = forward_of(&[("f.c", code)], "f.c", "s) {");
    assert!(to_sinks(&trace).is_empty(), "{trace:#?}");
}

#[test]
fn recursion_ends() {
    let code = "void walk(char *s, int n) {\n\
        char b[4];\n\
        if (n) walk(s, n - 1);\n\
        strcpy(b, s);\n\
    }\n";
    let trace = forward_of(&[("w.c", code)], "w.c", "s, int");
    assert_eq!(sink_lines(&trace), [4], "{trace:#?}");
}

#[test]
fn nothing_to_follow_outside_a_variable() {
    assert!(try_forward(&[("user.c", LOAD)], "user.c", "void").is_none());
}

#[test]
fn the_same_stop_is_listed_once() {
    let code = "char *g;\n\
        void keep(char *s) { g = s; }\n\
        int main(int argc, char **argv) {\n\
        keep(argv[1]);\n\
        keep(argv[1]);\n\
    }\n";
    let trace = forward_of(&[("g.c", code)], "g.c", "argv)");
    let stops = trace.paths.iter().filter(|p| !p.endpoint.is_sink()).count();
    assert_eq!(stops, 1, "{trace:#?}");
}

#[test]
fn a_write_through_a_moving_pointer_reaches_the_callers_buffer() {
    let code = "void copy(char *out, const char *in) {\n\
        while (*in) *out++ = *in++;\n\
        *out = 0;\n\
    }\n\
    int main(int argc, char **argv) {\n\
        char cmd[64];\n\
        copy(cmd, argv[1]);\n\
        system(cmd);\n\
    }\n";
    let trace = forward_of(&[("c.c", code)], "c.c", "argv[1]");
    assert_eq!(sink_lines(&trace), [8], "{trace:#?}");
}

#[test]
fn a_value_returned_through_recursion_ends() {
    let code = "int fact(int n) {\n\
        int r = n;\n\
        if (n <= 1) return 1;\n\
        return fact(n - 1) * r;\n\
    }\n";
    let trace = forward_of(&[("f.c", code)], "f.c", "r = n");
    assert!(to_sinks(&trace).is_empty(), "{trace:#?}");
}

#[test]
fn a_function_called_twice_on_one_path_passes_each_value_on() {
    let id = "char *id(char *x) { return x; }\n";
    let nested = format!(
        "{id}void f(char *a, char *dst) {{ char *p = id(id(a)); strcpy(dst, p); }}\n\
         int main(int argc, char **argv) {{ f(argv[1], 0); }}\n"
    );
    let trace = forward_of(&[("n.c", &nested)], "n.c", "argv[1]");
    assert_eq!(sink_lines(&trace), [2], "nested: {trace:#?}");
    let sequential = format!(
        "{id}void f(char *a, char *dst) {{ char *p = id(a); char *q = id(p); strcpy(dst, q); }}\n\
         int main(int argc, char **argv) {{ f(argv[1], 0); }}\n"
    );
    let trace = forward_of(&[("s.c", &sequential)], "s.c", "argv[1]");
    assert_eq!(sink_lines(&trace), [2], "sequential: {trace:#?}");
}

#[test]
fn the_same_name_in_other_functions_is_followed_in_each() {
    let code = "void a(char *s) { strcpy(0, s); }\n\
        void b(char *s) { strcpy(0, s); }\n\
        void c(char *s) { strcpy(0, s); }\n\
        void d(char *s) { strcpy(0, s); }\n\
        int main(int argc, char **argv) { a(argv[1]); b(argv[1]); c(argv[1]); d(argv[1]); }\n";
    let trace = forward_of(&[("m.c", code)], "m.c", "argv[1]");
    assert_eq!(sink_lines(&trace), [1, 2, 3, 4], "{trace:#?}");
}

#[test]
fn what_a_copy_returns_is_its_destination() {
    let code = "void f(const char *s) {\n\
        char buf[64];\n\
        char *p = strncpy(buf, s, 63);\n\
        system(p);\n\
    }\n";
    let trace = forward_of(&[("f.c", code)], "f.c", "s, 63");
    assert!(sink_lines(&trace).contains(&4), "{trace:#?}");
}

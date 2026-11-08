//! Tracing a value from a source to a sink, both picked.

use super::tests::position;
use super::*;
use crate::index::graph_of_files;

/// Traces from the first `source` needle in `file` to the call to `call`
/// on the line of the first `sink` needle.
fn between(sources: &[(&str, &str)], file: &str, source: &str, sink: &str, call: &str) -> Trace {
    let graph = graph_of_files(sources);
    let (line, column) = position(&graph, file, source);
    let (sink_line, _) = position(&graph, file, sink);
    let sink = Sink::picked(file, sink_line, call);
    trace_between(&graph, file, line, column, &sink).expect("a trace")
}

fn reached(trace: &Trace) -> Vec<&TracePath> {
    trace
        .paths
        .iter()
        .filter(|p| p.endpoint.is_sink())
        .collect()
}

fn lines(path: &TracePath) -> Vec<u32> {
    path.steps.iter().map(|s| s.line).collect()
}

const HANDLER: &str = "void store(char *dst, const char *src, int len) { }\n\
void log_it(int v) { }\n\
void handle(int fd) {\n\
    char buf[64], out[8];\n\
    int n = read(fd, buf, sizeof buf);\n\
    log_it(n); store(out, buf, 0);\n\
    store(out, buf, n);\n\
}\n";

#[test]
fn reaches_a_picked_call_that_is_not_risky() {
    let trace = between(
        &[("h.c", HANDLER)],
        "h.c",
        "n = read",
        "store(out, buf, n)",
        "store",
    );
    assert_eq!(trace.direction, Direction::Between);
    assert_eq!(trace.subject, "`n` to `store()`");
    let found = reached(&trace);
    assert_eq!(found.len(), 1, "{trace:#?}");
    assert_eq!(lines(found[0]), [5, 7]);
    assert_eq!(
        found[0].endpoint,
        Endpoint::Sink("`store()` (the sink)".into())
    );
    let last = found[0].steps.last().expect("a step");
    assert!(last.note.contains("argument 3"), "{}", last.note);
}

#[test]
fn only_paths_to_the_sink_are_kept_when_it_is_reached() {
    let trace = between(
        &[("h.c", HANDLER)],
        "h.c",
        "n = read",
        "store(out, buf, n)",
        "store",
    );
    assert!(
        trace.paths.iter().all(|p| p.endpoint.is_sink()),
        "{trace:#?}"
    );
}

#[test]
fn only_the_named_call_on_the_sink_line_counts() {
    let to_store = between(&[("h.c", HANDLER)], "h.c", "n = read", "log_it(n)", "store");
    assert!(reached(&to_store).is_empty(), "{to_store:#?}");
    let to_log = between(
        &[("h.c", HANDLER)],
        "h.c",
        "n = read",
        "log_it(n)",
        "log_it",
    );
    assert_eq!(reached(&to_log).len(), 1, "{to_log:#?}");
}

#[test]
fn a_sink_the_value_never_reaches_keeps_where_it_stopped() {
    const UNUSED: &str = "void store(int len) { }\n\
void handle(void) {\n\
    int n = 4;\n\
    int m = n * 2;\n\
    store(7);\n\
}\n";
    let trace = between(&[("u.c", UNUSED)], "u.c", "n = 4", "store(7)", "store");
    assert!(reached(&trace).is_empty(), "{trace:#?}");
    assert_eq!(trace.direction, Direction::Between);
}

#[test]
fn follows_the_value_through_calls_to_a_sink_in_another_function() {
    const NESTED: &str = "void sink_fn(int len) { }\n\
void middle(int k) {\n\
    sink_fn(k);\n\
}\n\
void top(void) {\n\
    int n = atoi(getenv(\"N\"));\n\
    middle(n);\n\
}\n";
    let trace = between(
        &[("n.c", NESTED)],
        "n.c",
        "n = atoi",
        "sink_fn(k)",
        "sink_fn",
    );
    let found = reached(&trace);
    assert_eq!(found.len(), 1, "{trace:#?}");
    assert_eq!(lines(found[0]), [6, 7, 3]);
}

#[test]
fn a_qualified_sink_name_matches_its_call() {
    const METHOD: &str = "namespace io { void put(int v) { } }\n\
void handle(void) {\n\
    int n = 3;\n\
    io::put(n);\n\
}\n";
    let trace = between(
        &[("m.cpp", METHOD)],
        "m.cpp",
        "n = 3",
        "io::put(n)",
        "io::put",
    );
    assert_eq!(reached(&trace).len(), 1, "{trace:#?}");
}

#[test]
fn last_names_of_qualified_member_and_template_calls() {
    for (called, name) in [
        ("parse", "parse"),
        ("ns::parse", "parse"),
        ("obj.parse", "parse"),
        ("p->parse", "parse"),
        ("make<int>", "make"),
        ("std::make<std::string>", "make"),
        ("operator>", "operator>"),
    ] {
        assert_eq!(last_name(called), name, "{called}");
    }
}

#[test]
fn no_variable_at_the_source_is_no_trace() {
    let graph = graph_of_files(&[("h.c", HANDLER)]);
    let sink = Sink::picked("h.c", 7, "store");
    assert!(trace_between(&graph, "h.c", 1, 0, &sink).is_none());
}

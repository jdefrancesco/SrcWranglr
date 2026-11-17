use super::findings;
use crate::analysis::Severity;
use crate::index::graph_of_files;

/// (rule, line) of each finding the walk makes for `source`.
fn rules(source: &str) -> Vec<(String, u32)> {
    findings(&graph_of_files(&[("a.c", source)]))
        .into_iter()
        .map(|f| (f.rule, f.line))
        .collect()
}

#[test]
fn freeing_then_dereferencing_is_use_after_free() {
    let found = rules(
        "void f(char *p) {\n\
         \x20   free(p);\n\
         \x20   *p = 0;\n\
         }\n",
    );
    assert_eq!(found, [("use-after-free".to_string(), 3)]);
}

#[test]
fn freeing_twice_is_double_free() {
    let found = rules(
        "void f(char *p) {\n\
         \x20   free(p);\n\
         \x20   free(p);\n\
         }\n",
    );
    assert_eq!(found, [("double-free".to_string(), 3)]);
}

#[test]
fn passing_a_freed_pointer_to_a_call_is_use_after_free() {
    let found = rules(
        "void g(char *q);\n\
         void f(char *p) {\n\
         \x20   free(p);\n\
         \x20   g(p);\n\
         }\n",
    );
    assert_eq!(found, [("use-after-free".to_string(), 4)]);
}

#[test]
fn reallocating_before_the_use_is_fine() {
    let found = rules(
        "void f(char *p, int n) {\n\
         \x20   free(p);\n\
         \x20   p = malloc(n);\n\
         \x20   *p = 0;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn nulling_after_free_is_not_a_use_after_free() {
    let found = rules(
        "void f(char *p) {\n\
         \x20   free(p);\n\
         \x20   p = 0;\n\
         \x20   g(p);\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn using_before_the_free_is_fine() {
    let found = rules(
        "void f(char *p) {\n\
         \x20   *p = 0;\n\
         \x20   free(p);\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_function_that_never_frees_has_no_findings() {
    let found = rules(
        "void f(char *p) {\n\
         \x20   *p = 0;\n\
         \x20   p[1] = 1;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_straight_line_use_after_free_is_high_severity() {
    let graph = graph_of_files(&[("a.c", "void f(char *p) {\n   free(p);\n   *p = 0;\n}\n")]);
    let found = findings(&graph);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].severity, Severity::High);
    assert_eq!(found[0].cwe, Some(416));
}

#[test]
fn freeing_in_one_branch_then_using_is_a_lower_confidence_lead() {
    // Freed only when c is set, then used unconditionally: a real
    // use-after-free on that path, flagged at medium (maybe-freed).
    let graph = graph_of_files(&[(
        "a.c",
        "void f(char *p, int c) {\n   if (c) { free(p); }\n   *p = 0;\n}\n",
    )]);
    let found = findings(&graph);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].rule, "use-after-free");
    assert_eq!(found[0].severity, Severity::Medium);
}

#[test]
fn the_classic_safe_list_free_loop_is_not_flagged() {
    // Each node is read into a temp, freed, then the cursor advances — a
    // correct free loop that a naive checker would flag.
    let found = rules(
        "void f(struct n *head) {\n\
         \x20   struct n *cur = head;\n\
         \x20   while (cur) {\n\
         \x20       struct n *next = cur->next;\n\
         \x20       free(cur);\n\
         \x20       cur = next;\n\
         \x20   }\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn freeing_one_pointer_and_using_another_is_not_flagged() {
    let found = rules(
        "void f(char *a, char *b) {\n\
         \x20   free(a);\n\
         \x20   *b = 0;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn freeing_inside_a_branch_with_no_later_use_is_not_flagged() {
    let found = rules(
        "void f(char *p, int c) {\n\
         \x20   if (c) {\n\
         \x20       free(p);\n\
         \x20   }\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn lifetimes_lists_each_pointers_free_and_use() {
    use super::{lifetimes, PointKind};
    let graph = graph_of_files(&[(
        "a.c",
        "void f(char *p, char *q) {\n   free(p);\n   *p = 0;\n   free(q);\n}\n",
    )]);
    let matrix = lifetimes(&graph, "a.c", 2).expect("lifetimes");
    assert_eq!((matrix.first_line, matrix.last_line), (1, 5));
    assert_eq!(matrix.rows.len(), 2);
    let p = &matrix.rows[0];
    assert_eq!(p.variable, "p");
    let points: Vec<_> = p.points.iter().map(|pt| (pt.line, pt.kind)).collect();
    assert_eq!(points, [(2, PointKind::Freed), (3, PointKind::Used)]);
    // q is freed but never misused: still shown, just the free.
    let q = &matrix.rows[1];
    assert_eq!(q.variable, "q");
    assert_eq!(
        q.points.iter().map(|pt| pt.kind).collect::<Vec<_>>(),
        [PointKind::Freed]
    );
}

#[test]
fn a_free_in_a_returning_branch_is_not_flagged_as_a_later_use() {
    // The classic cleanup pattern: the free path returns, so the later use
    // runs only when the free did not happen.
    let found = rules(
        "char *f(int err, char *p) {\n\
         \x20   if (err) { free(p); return 0; }\n\
         \x20   *p = 1;\n\
         \x20   return p;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_for_loop_initializer_reassignment_clears_the_freed_pointer() {
    let found = rules(
        "void f(char *p, char *list) {\n\
         \x20   free(p);\n\
         \x20   for (p = list; p; p = list) { *p = 0; }\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_trailing_comment_after_return_still_terminates_the_branch() {
    let found = rules(
        "void f(int err, char *p) {\n\
         \x20   if (err) { free(p); return; /* done */ }\n\
         \x20   free(p);\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_returning_branch_inside_a_switch_case_is_handled() {
    let found = rules(
        "void f(int k, char *p) {\n\
         \x20   switch (k) {\n\
         \x20   case 1:\n\
         \x20       if (!p) { free(p); return; }\n\
         \x20       *p = 0;\n\
         \x20       break;\n\
         \x20   }\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_function_that_did_not_parse_cleanly_is_skipped() {
    // An incomplete expression leaves the body with a parse error; emit
    // nothing rather than flatten the control flow into false positives.
    let found = rules(
        "void f(char *p) {\n\
         \x20   free(p);\n\
         \x20   *p = ;\n\
         \x20   return;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

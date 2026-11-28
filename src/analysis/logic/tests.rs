use super::{findings, TOOL};
use crate::index::graph_of_files;

/// (rule, line) of each logic finding the walk makes for `source`.
fn rules(source: &str) -> Vec<(String, u32)> {
    findings(&graph_of_files(&[("a.c", source)]))
        .into_iter()
        .map(|f| (f.rule, f.line))
        .collect()
}

#[test]
fn an_assignment_in_a_condition_is_flagged() {
    let found = rules(
        "int f(int x, int y) {\n\
         \x20   if (x = y) {\n\
         \x20       return 1;\n\
         \x20   }\n\
         \x20   return 0;\n\
         }\n",
    );
    assert_eq!(found, [("assignment-in-condition".to_string(), 2)]);
}

#[test]
fn the_double_paren_idiom_is_left_alone() {
    let found = rules(
        "int f(int x, int y) {\n\
         \x20   if ((x = y)) {\n\
         \x20       return 1;\n\
         \x20   }\n\
         \x20   return 0;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_equality_check_in_a_condition_is_fine() {
    let found = rules(
        "int f(int x, int y) {\n\
         \x20   if (x == y) {\n\
         \x20       return 1;\n\
         \x20   }\n\
         \x20   return 0;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_bitwise_and_compared_for_equality_is_a_precedence_bug() {
    let found = rules(
        "int f(int flags, int mask, int v) {\n\
         \x20   return flags & mask == v;\n\
         }\n",
    );
    assert_eq!(found, [("operator-precedence".to_string(), 2)]);
}

#[test]
fn a_parenthesized_mask_test_is_fine() {
    let found = rules(
        "int f(int flags, int mask, int v) {\n\
         \x20   return (flags & mask) == v;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn comparing_a_value_with_itself_is_flagged() {
    let found = rules(
        "int f(int a) {\n\
         \x20   if (a == a) {\n\
         \x20       return 1;\n\
         \x20   }\n\
         \x20   return 0;\n\
         }\n",
    );
    assert_eq!(found, [("self-comparison".to_string(), 2)]);
}

#[test]
fn a_not_equal_self_test_is_left_alone_as_the_nan_idiom() {
    let found = rules(
        "int is_nan(double x) {\n\
         \x20   return x != x;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn assigning_a_variable_to_itself_is_flagged() {
    let found = rules(
        "void f(int x) {\n\
         \x20   x = x;\n\
         }\n",
    );
    assert_eq!(found, [("self-assignment".to_string(), 2)]);
}

#[test]
fn a_field_assigned_from_a_plain_argument_is_not_self_assignment() {
    let found = rules(
        "struct S { int x; };\n\
         void f(struct S *s, int x) {\n\
         \x20   s->x = x;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn dereferencing_an_unchecked_allocation_is_flagged() {
    let found = rules(
        "void f(int n) {\n\
         \x20   char *p = malloc(n);\n\
         \x20   *p = 0;\n\
         }\n",
    );
    assert_eq!(found, [("unchecked-allocation".to_string(), 3)]);
}

#[test]
fn a_null_checked_allocation_is_fine() {
    let found = rules(
        "void f(int n) {\n\
         \x20   char *p = malloc(n);\n\
         \x20   if (!p) {\n\
         \x20       return;\n\
         \x20   }\n\
         \x20   *p = 0;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_asserted_allocation_is_fine() {
    let found = rules(
        "void assert(int);\n\
         void f(int n) {\n\
         \x20   char *p = malloc(n);\n\
         \x20   assert(p);\n\
         \x20   *p = 0;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_field_access_through_an_unchecked_allocation_is_flagged() {
    let found = rules(
        "struct S { int x; };\n\
         void f(int n) {\n\
         \x20   struct S *p = malloc(n);\n\
         \x20   p->x = 0;\n\
         }\n",
    );
    assert_eq!(found, [("unchecked-allocation".to_string(), 4)]);
}

#[test]
fn sizeof_of_the_pointer_is_not_a_dereference() {
    let found = rules(
        "void f(void) {\n\
         \x20   int *p = malloc(sizeof(*p));\n\
         \x20   if (p) {\n\
         \x20       *p = 0;\n\
         \x20   }\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_clean_function_yields_nothing() {
    let found = rules(
        "int add(int a, int b) {\n\
         \x20   return a + b;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn reading_an_uninitialized_local_is_flagged() {
    let found = rules(
        "int f(void) {\n\
         \x20   int x;\n\
         \x20   return x;\n\
         }\n",
    );
    assert_eq!(found, [("uninitialized-read".to_string(), 3)]);
}

#[test]
fn an_initialized_local_is_fine() {
    let found = rules(
        "int f(void) {\n\
         \x20   int x = 0;\n\
         \x20   return x;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_write_before_the_read_is_fine() {
    let found = rules(
        "int f(int c) {\n\
         \x20   int x;\n\
         \x20   x = c;\n\
         \x20   return x;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn taking_the_address_before_the_read_is_fine() {
    let found = rules(
        "int scanf(const char *, ...);\n\
         int f(void) {\n\
         \x20   int x;\n\
         \x20   scanf(\"%d\", &x);\n\
         \x20   return x;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_operand_of_sizeof_is_not_a_read() {
    let found = rules(
        "unsigned long f(void) {\n\
         \x20   int x;\n\
         \x20   unsigned long n = sizeof(x);\n\
         \x20   x = 0;\n\
         \x20   return x + n;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn dereferencing_an_uninitialized_pointer_is_flagged() {
    let found = rules(
        "void f(void) {\n\
         \x20   char *p;\n\
         \x20   *p = 0;\n\
         }\n",
    );
    assert_eq!(found, [("uninitialized-read".to_string(), 3)]);
}

#[test]
fn a_static_local_is_not_flagged() {
    let found = rules(
        "int f(void) {\n\
         \x20   static int x;\n\
         \x20   return x;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_function_with_goto_is_left_alone() {
    let found = rules(
        "int f(int c) {\n\
         \x20   int x;\n\
         \x20   if (c) {\n\
         \x20       goto done;\n\
         \x20   }\n\
         \x20   x = 0;\n\
         done:\n\
         \x20   return x;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_opened_file_that_is_never_closed_is_flagged() {
    let found = rules(
        "void f(const char *p) {\n\
         \x20   FILE *fp = fopen(p, \"r\");\n\
         }\n",
    );
    assert_eq!(found, [("resource-leak".to_string(), 2)]);
}

#[test]
fn a_closed_file_is_fine() {
    let found = rules(
        "void f(const char *p) {\n\
         \x20   FILE *fp = fopen(p, \"r\");\n\
         \x20   fclose(fp);\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_returned_handle_is_fine() {
    let found = rules(
        "FILE *f(const char *p) {\n\
         \x20   FILE *fp = fopen(p, \"r\");\n\
         \x20   return fp;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_handle_that_is_used_is_left_alone() {
    let found = rules(
        "void f(const char *p) {\n\
         \x20   FILE *fp = fopen(p, \"a\");\n\
         \x20   fprintf(fp, \"hi\");\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn reopening_a_handle_without_closing_is_flagged() {
    let found = rules(
        "void f(const char *a, const char *b) {\n\
         \x20   FILE *fp = fopen(a, \"r\");\n\
         \x20   fp = fopen(b, \"r\");\n\
         \x20   fclose(fp);\n\
         }\n",
    );
    assert_eq!(found, [("resource-leak".to_string(), 2)]);
}

#[test]
fn a_checked_and_closed_file_descriptor_is_fine() {
    let found = rules(
        "int f(const char *p) {\n\
         \x20   int fd = open(p, 0);\n\
         \x20   if (fd < 0) {\n\
         \x20       return -1;\n\
         \x20   }\n\
         \x20   close(fd);\n\
         \x20   return 0;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn multiplying_two_variables_for_an_allocation_size_is_flagged() {
    let found = rules(
        "void *f(int n, int m) {\n\
         \x20   return malloc(n * m);\n\
         }\n",
    );
    assert_eq!(found, [("allocation-size-overflow".to_string(), 2)]);
}

#[test]
fn a_count_times_sizeof_allocation_is_flagged() {
    let found = rules(
        "void *f(int n) {\n\
         \x20   return malloc(n * sizeof(int));\n\
         }\n",
    );
    assert_eq!(found, [("allocation-size-overflow".to_string(), 2)]);
}

#[test]
fn multiplying_by_a_constant_is_left_alone() {
    let found = rules(
        "void *f(int n) {\n\
         \x20   return malloc(n * 4);\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn calloc_detects_its_own_overflow_and_is_left_alone() {
    let found = rules(
        "void *f(int n, int m) {\n\
         \x20   return calloc(n, m);\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_overflowing_realloc_size_is_flagged() {
    let found = rules(
        "void *f(void *p, int n, int m) {\n\
         \x20   return realloc(p, n * m);\n\
         }\n",
    );
    assert_eq!(found, [("allocation-size-overflow".to_string(), 2)]);
}

#[test]
fn a_plain_constant_allocation_is_fine() {
    let found = rules(
        "void *f(void) {\n\
         \x20   return malloc(16);\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_unsigned_less_than_zero_guard_is_flagged() {
    let found = rules(
        "void f(size_t n) {\n\
         \x20   if (n < 0) {\n\
         \x20       return;\n\
         \x20   }\n\
         }\n",
    );
    assert_eq!(found, [("unsigned-comparison".to_string(), 2)]);
}

#[test]
fn an_unsigned_at_least_zero_test_is_flagged() {
    let found = rules(
        "void f(unsigned x) {\n\
         \x20   if (x >= 0) {\n\
         \x20       return;\n\
         \x20   }\n\
         }\n",
    );
    assert_eq!(found, [("unsigned-comparison".to_string(), 2)]);
}

#[test]
fn a_signed_less_than_zero_check_is_fine() {
    let found = rules(
        "void f(int n) {\n\
         \x20   if (n < 0) {\n\
         \x20       return;\n\
         \x20   }\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_unsigned_at_most_zero_test_is_not_a_tautology() {
    let found = rules(
        "void f(size_t n) {\n\
         \x20   if (n <= 0) {\n\
         \x20       return;\n\
         \x20   }\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_reversed_zero_at_most_unsigned_test_is_flagged() {
    let found = rules(
        "void f(size_t n) {\n\
         \x20   if (0 <= n) {\n\
         \x20       return;\n\
         \x20   }\n\
         }\n",
    );
    assert_eq!(found, [("unsigned-comparison".to_string(), 2)]);
}

#[test]
fn an_unsigned_countdown_loop_never_terminates() {
    let found = rules(
        "void f(size_t n) {\n\
         \x20   for (size_t i = n; i >= 0; i--) {\n\
         \x20   }\n\
         }\n",
    );
    assert_eq!(found, [("unsigned-comparison".to_string(), 2)]);
}

#[test]
fn sizeof_of_a_pointer_as_a_memset_size_is_flagged() {
    let found = rules(
        "void f(char *p) {\n\
         \x20   memset(p, 0, sizeof(p));\n\
         }\n",
    );
    assert_eq!(found, [("sizeof-pointer".to_string(), 2)]);
}

#[test]
fn sizeof_of_the_pointee_is_fine() {
    let found = rules(
        "void f(char *p) {\n\
         \x20   memset(p, 0, sizeof(*p));\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn sizeof_of_a_real_array_is_fine() {
    let found = rules(
        "void f(void) {\n\
         \x20   char buf[64];\n\
         \x20   memset(buf, 0, sizeof(buf));\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn sizeof_of_a_pointer_as_a_malloc_size_is_flagged() {
    let found = rules(
        "void *f(char *p) {\n\
         \x20   return malloc(sizeof(p));\n\
         }\n",
    );
    assert_eq!(found, [("sizeof-pointer".to_string(), 2)]);
}

#[test]
fn a_constant_index_past_the_array_end_is_flagged() {
    let found = rules(
        "void f(void) {\n\
         \x20   char buf[10];\n\
         \x20   buf[10] = 0;\n\
         }\n",
    );
    assert_eq!(found, [("array-bounds".to_string(), 3)]);
}

#[test]
fn a_constant_index_within_the_array_is_fine() {
    let found = rules(
        "void f(void) {\n\
         \x20   char buf[10];\n\
         \x20   buf[9] = 0;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_loop_that_runs_one_past_the_end_is_flagged() {
    let found = rules(
        "void f(void) {\n\
         \x20   char buf[10];\n\
         \x20   for (int i = 0; i <= 10; i++) {\n\
         \x20       buf[i] = 0;\n\
         \x20   }\n\
         }\n",
    );
    assert_eq!(found, [("array-bounds".to_string(), 4)]);
}

#[test]
fn a_loop_with_a_correct_bound_is_fine() {
    let found = rules(
        "void f(void) {\n\
         \x20   char buf[10];\n\
         \x20   for (int i = 0; i < 10; i++) {\n\
         \x20       buf[i] = 0;\n\
         \x20   }\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_loop_bound_larger_than_the_array_is_flagged() {
    let found = rules(
        "void f(void) {\n\
         \x20   char buf[10];\n\
         \x20   for (int i = 0; i < 20; i++) {\n\
         \x20       buf[i] = 0;\n\
         \x20   }\n\
         }\n",
    );
    assert_eq!(found, [("array-bounds".to_string(), 4)]);
}

#[test]
fn an_array_parameters_size_is_not_trusted() {
    let found = rules(
        "void f(char buf[10]) {\n\
         \x20   buf[10] = 0;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn logic_findings_are_tagged_and_carry_a_cwe() {
    let graph = graph_of_files(&[(
        "a.c",
        "int f(int x, int y) {\n\
         \x20   if (x = y) {\n\
         \x20       return 1;\n\
         \x20   }\n\
         \x20   return 0;\n\
         }\n",
    )]);
    let found = findings(&graph);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].tool, TOOL);
    assert_eq!(found[0].cwe, Some(481));
    assert!(
        found[0].symbol.is_some(),
        "the finding is attributed to its enclosing function"
    );
}

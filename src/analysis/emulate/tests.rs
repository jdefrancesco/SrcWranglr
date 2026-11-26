use std::collections::HashMap;

use super::{run, Trace};
use crate::index::graph_of_files;

/// Emulate the function on `line` of a one-file project with `inputs` set.
fn emulate(source: &str, line: u32, inputs: &[(&str, i128)]) -> Trace {
    let graph = graph_of_files(&[("a.c", source)]);
    let given: HashMap<String, i128> = inputs.iter().map(|(n, v)| (n.to_string(), *v)).collect();
    run(&graph, "a.c", line, &given).expect("a function to emulate")
}

/// The miniupnp-style header append: `malloc(namelen + valuelen + 2)` then two
/// copies that exactly fill it. Safe for every choice of input sizes.
const APPEND: &str = "int append(char *name, char *value) {\n\
     \x20   int namelen = strlen(name);\n\
     \x20   int valuelen = strlen(value);\n\
     \x20   char *entry = malloc(namelen + valuelen + 2);\n\
     \x20   if (entry == 0) return -1;\n\
     \x20   memcpy(entry, name, namelen + 1);\n\
     \x20   memcpy(entry + namelen + 1, value, valuelen + 1);\n\
     \x20   return 0;\n\
     }\n";

#[test]
fn the_exactly_filling_copies_never_overflow() {
    for (namelen, valuelen) in [(0, 0), (5, 10), (1000, 1), (1, 1000)] {
        let trace = emulate(APPEND, 1, &[("namelen", namelen), ("valuelen", valuelen)]);
        assert!(
            trace.first_overflow().is_none(),
            "overflow at namelen={namelen} valuelen={valuelen}: {:?}",
            trace.first_overflow()
        );
    }
}

#[test]
fn an_off_by_one_allocation_overflows_the_second_copy() {
    // One byte short: malloc(namelen + valuelen + 1).
    let source = APPEND.replace("valuelen + 2)", "valuelen + 1)");
    let trace = emulate(&source, 1, &[("namelen", 5), ("valuelen", 10)]);
    let step = trace.first_overflow().expect("an overflow");
    let overflow = step.overflow.as_ref().unwrap();
    // The second copy ends at namelen+1 + valuelen+1 = 17, buffer is 16.
    assert_eq!((overflow.end, overflow.size), (17, 16));
    assert!(step.text.contains("writes 11 bytes"), "{}", step.text);
}

#[test]
fn the_inputs_it_needs_are_discovered() {
    let trace = emulate(APPEND, 1, &[]);
    let names: Vec<&str> = trace.inputs.iter().map(|i| i.name.as_str()).collect();
    assert!(names.contains(&"namelen"), "{names:?}");
    assert!(names.contains(&"valuelen"), "{names:?}");
}

#[test]
fn a_branch_is_taken_concretely_from_the_inputs() {
    let source = "void f(int n) {\n\
         \x20   char *b = malloc(8);\n\
         \x20   if (n > 8) {\n\
         \x20       memcpy(b, src, n);\n\
         \x20   }\n\
         }\n";
    // n small: the overflowing copy isn't reached.
    assert!(emulate(source, 1, &[("n", 4)]).first_overflow().is_none());
    // n large: the branch runs and the copy overflows the 8-byte buffer.
    let big = emulate(source, 1, &[("n", 16)]);
    let overflow = big
        .first_overflow()
        .expect("overflow")
        .overflow
        .clone()
        .unwrap();
    assert_eq!((overflow.end, overflow.size), (16, 8));
}

#[test]
fn a_constant_sized_copy_that_fits_is_fine() {
    let source = "void f(void) {\n\
         \x20   char *b = malloc(16);\n\
         \x20   memcpy(b, \"x\", 8);\n\
         }\n";
    assert!(emulate(source, 1, &[]).first_overflow().is_none());
}

#[test]
fn a_function_that_does_not_parse_is_skipped() {
    let graph = graph_of_files(&[("a.c", "void f(void) {\n    char *b = ;\n}\n")]);
    assert!(run(&graph, "a.c", 1, &HashMap::new()).is_none());
}

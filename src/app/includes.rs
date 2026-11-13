//! Going to a file named in the source: an `#include`'s header, or a file
//! name such as `foo.c` in a comment. Press G over it.
//!
//! The name is looked for next to the file it is written in, then among
//! the project's files whose path ends with it. When several do (an
//! `asm/io.h` for each architecture), the one sharing the most folders
//! with the file it is written in wins. Only the project's own (indexed)
//! files are ever opened.

use std::cmp::Reverse;

use crate::model::CodeGraph;

/// File name endings that name C or C++ source.
const SOURCE_ENDINGS: [&str; 13] = [
    ".c", ".cc", ".cpp", ".cxx", ".c++", ".h", ".hh", ".hpp", ".hxx", ".h++", ".inl", ".ipp",
    ".tpp",
];
/// A name longer than this isn't looked for.
const MAX_NAME_CHARS: usize = 512;

/// The path an `#include` line names (`"x.h"` or `<sys/x.h>`), if `line`
/// is one.
pub fn included(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix('#')?.trim_start();
    let rest = rest
        .strip_prefix("include_next")
        .or_else(|| rest.strip_prefix("include"))
        .or_else(|| rest.strip_prefix("import"))?
        .trim_start();
    let (close, rest) = match rest.chars().next()? {
        '"' => ('"', &rest[1..]),
        '<' => ('>', &rest[1..]),
        _ => return None,
    };
    let name = rest[..rest.find(close)?].trim();
    is_plausible(name).then_some(name)
}

/// Whether `name` could be a file's path: not empty, not too long.
fn is_plausible(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME_CHARS
}

/// The file name around byte `at` of `line`, if it names C or C++ source
/// (`net/core.c`, `foo.h`).
pub fn file_name_at(line: &str, at: usize) -> Option<&str> {
    let is_path_char = |c: char| c.is_ascii_alphanumeric() || "_./-+".contains(c);
    if !line.get(at..)?.chars().next().is_some_and(is_path_char) {
        return None;
    }
    let start = line[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_path_char(*c))
        .last()
        .map_or(at, |(i, _)| i);
    let end = line[at..]
        .char_indices()
        .find(|(_, c)| !is_path_char(*c))
        .map_or(line.len(), |(i, _)| at + i);
    let name = line[start..end].trim_end_matches(['.', '-']);
    if !is_plausible(name) {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    SOURCE_ENDINGS
        .iter()
        .any(|ending| lower.ends_with(ending) && lower.len() > ending.len())
        .then_some(name)
}

/// The project file `name` (as written in `from`) means, if one is
/// indexed.
pub fn resolve(graph: &CodeGraph, from: &str, name: &str) -> Option<String> {
    if !is_plausible(name) {
        return None;
    }
    let name = name.trim_start_matches("./");
    let beside = normalized(&match folder_of(from) {
        "" => name.to_string(),
        folder => format!("{folder}/{name}"),
    });
    if let Some(beside) = beside.filter(|f| graph.file_contents.contains_key(f)) {
        return Some(beside);
    }
    let name = normalized(name)?;
    let suffix = format!("/{name}");
    graph
        .file_contents
        .keys()
        .filter(|file| **file == name || file.ends_with(&suffix))
        // Most folders shared, then the shortest path, then the first.
        .max_by_key(|file| {
            (
                shared_folders(file, from),
                Reverse(file.len()),
                Reverse(*file),
            )
        })
        .cloned()
}

/// `path` with `.` steps dropped and `..` steps applied; `None` if it
/// climbs above where it starts.
fn normalized(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The folder `path` is in (`""` at the top).
fn folder_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// How many leading folders `a` and `b` share.
fn shared_folders(a: &str, b: &str) -> usize {
    let (a, b) = (folder_of(a), folder_of(b));
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    a.split('/')
        .zip(b.split('/'))
        .take_while(|(x, y)| x == y)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::graph_of_files;

    fn graph() -> CodeGraph {
        graph_of_files(&[
            ("drivers/net/eth.c", "int x;\n"),
            ("drivers/net/eth.h", "int y;\n"),
            ("include/linux/slab.h", "int z;\n"),
            ("arch/x86/include/asm/io.h", "int a;\n"),
            ("arch/arm/include/asm/io.h", "int b;\n"),
            ("arch/arm/kernel/setup.c", "int c;\n"),
            ("lib/util.h", "int d;\n"),
        ])
    }

    #[test]
    fn reads_the_name_an_include_line_gives() {
        assert_eq!(included("#include \"eth.h\""), Some("eth.h"));
        assert_eq!(
            included("  #  include <linux/slab.h> // why"),
            Some("linux/slab.h")
        );
        assert_eq!(included("#include_next <stdio.h>"), Some("stdio.h"));
        assert_eq!(included("#include MACRO_HEADER"), None);
        assert_eq!(included("int include;"), None);
        assert_eq!(included("#include \"unclosed.h"), None);
        let long = format!("#include \"{}.h\"", "a".repeat(MAX_NAME_CHARS));
        assert_eq!(included(&long), None);
        assert_eq!(file_name_at(&long[10..], 0), None);
    }

    #[test]
    fn finds_a_file_name_under_the_pointer() {
        let line = "/* see net/core.c, and foo.h. Not foo.txt */";
        assert_eq!(file_name_at(line, 8), Some("net/core.c"));
        assert_eq!(file_name_at(line, 24), Some("foo.h"));
        assert_eq!(file_name_at(line, 38), None);
        assert_eq!(file_name_at(line, 0), None);
        assert_eq!(file_name_at("x .h", 2), None);
    }

    #[test]
    fn a_name_is_looked_for_beside_the_file_first() {
        let graph = graph();
        assert_eq!(
            resolve(&graph, "drivers/net/eth.c", "eth.h").as_deref(),
            Some("drivers/net/eth.h")
        );
        assert_eq!(
            resolve(&graph, "drivers/net/eth.c", "../../lib/util.h").as_deref(),
            Some("lib/util.h")
        );
    }

    #[test]
    fn otherwise_the_closest_project_file_ending_with_it() {
        let graph = graph();
        assert_eq!(
            resolve(&graph, "drivers/net/eth.c", "linux/slab.h").as_deref(),
            Some("include/linux/slab.h")
        );
        assert_eq!(
            resolve(&graph, "arch/arm/kernel/setup.c", "asm/io.h").as_deref(),
            Some("arch/arm/include/asm/io.h")
        );
        assert_eq!(resolve(&graph, "drivers/net/eth.c", "stdio.h"), None);
        // A partial name never matches the middle of another.
        assert_eq!(
            resolve(&graph, "drivers/net/eth.c", "slab.h").as_deref(),
            Some("include/linux/slab.h")
        );
        assert_eq!(resolve(&graph, "drivers/net/eth.c", "ab.h"), None);
    }

    #[test]
    fn a_name_climbing_out_of_the_project_finds_nothing() {
        let graph = graph();
        assert_eq!(resolve(&graph, "lib/util.h", "../../../etc/passwd.h"), None);
    }
}

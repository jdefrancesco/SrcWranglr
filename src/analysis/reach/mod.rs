//! Which functions outside input can reach: starting from the functions
//! that read it (with `recv`, `read`, `fread`, `getenv`, …, or `main` with
//! its command line), through the calls they pass it to, then down the call
//! graph.
//!
//! Only direct calls are followed. Calls through function pointers,
//! callbacks, and virtual calls the indexer can't resolve are not, and
//! input kept somewhere else first (a global, say) isn't tracked, so "not
//! reached" means no such path was found, not that none exists.

mod code;
mod sources;

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet, VecDeque};

use self::code::Code;
pub(crate) use self::sources::{input_source, Lands, COMMAND_LINE, MAIN_NAMES};
use self::sources::{readers, sort_key, Reader};
use crate::analysis::Finding;
use crate::model::{CodeGraph, EdgeKind, SymbolId};

/// How input gets to a function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputPath {
    /// Where the input comes in, e.g. `recv()` or `the command line`.
    pub source: String,
    /// The function that reads it, then each one called on the way, ending
    /// with the function asked about.
    pub functions: Vec<SymbolId>,
}

/// The functions outside input reaches, each with its shortest path.
#[derive(Debug, Clone, Default)]
pub struct Reach {
    /// Per reached function, the function it is called from on its path
    /// (`None` for one that reads input itself), and the source.
    hops: HashMap<SymbolId, (Option<SymbolId>, String)>,
    /// Functions that read input themselves and are also handed input by a
    /// caller, so all their code counts.
    handed: HashSet<SymbolId>,
    /// Every source of input found, in name order.
    sources: Vec<String>,
    /// The functions that read input themselves, by function, then file.
    readers: HashMap<SymbolId, HashMap<String, Reader>>,
    /// The readers' files, read as code.
    codes: HashMap<String, Code>,
}

/// The walk down the call graph, while it runs.
struct Walk {
    hops: HashMap<SymbolId, (Option<SymbolId>, String)>,
    queue: VecDeque<SymbolId>,
    readers: HashSet<SymbolId>,
    handed: HashSet<SymbolId>,
}

impl Walk {
    /// Input reaches `callee` from `from`.
    fn reach(&mut self, callee: SymbolId, from: SymbolId, source: &str) {
        match self.hops.entry(callee) {
            Entry::Vacant(slot) => {
                slot.insert((Some(from), source.to_string()));
                self.queue.push_back(callee);
            }
            // A reader handed input by a caller passes it all on, too.
            Entry::Occupied(_) if self.readers.contains(&callee) && callee != from => {
                if self.handed.insert(callee) {
                    self.queue.push_back(callee);
                }
            }
            Entry::Occupied(_) => {}
        }
    }
}

impl Reach {
    pub fn of(graph: &CodeGraph) -> Self {
        let callees = callees(graph);
        let found = readers(graph);
        let mut walk = Walk {
            hops: HashMap::new(),
            queue: VecDeque::new(),
            readers: found.readers.iter().map(|r| r.symbol).collect(),
            handed: HashSet::new(),
        };
        for reader in &found.readers {
            walk.hops
                .entry(reader.symbol)
                .or_insert_with(|| (None, reader.source.clone()));
        }
        // A reader hands input on only to the calls it passes it to, in the
        // file it reads in: same-named functions in other files (every
        // `main`, say) are one symbol to the indexer, but not one function.
        for reader in &found.readers {
            for &callee in &reader.hands_to {
                walk.reach(callee, reader.symbol, &reader.source);
            }
        }
        // Then breadth first, so each function gets a shortest path.
        while let Some(function) = walk.queue.pop_front() {
            let source = walk
                .hops
                .get(&function)
                .map(|(_, s)| s.clone())
                .unwrap_or_default();
            for &callee in callees.get(&function).into_iter().flatten() {
                walk.reach(callee, function, &source);
            }
        }
        let mut readers: HashMap<SymbolId, HashMap<String, Reader>> = HashMap::new();
        for reader in found.readers {
            readers
                .entry(reader.symbol)
                .or_default()
                .insert(reader.file.clone(), reader);
        }
        Self {
            hops: walk.hops,
            handed: walk.handed,
            sources: found.sources,
            readers,
            codes: found.codes,
        }
    }

    /// Whether input gets to the code `finding` flags: code in a function
    /// input is handed to, or code in a function that reads input that uses
    /// what it read.
    pub fn reaches_finding(&self, finding: &Finding) -> bool {
        let Some(symbol) = finding.symbol else {
            return false;
        };
        match self.hops.get(&symbol) {
            None => false,
            Some((Some(_), _)) => true,
            Some((None, _)) => {
                self.handed.contains(&symbol)
                    || self
                        .reader_of(finding)
                        .is_some_and(|reader| reader.uses_input(self.flagged_code(finding)))
            }
        }
    }

    /// How input gets to the code `finding` flags, if it does.
    pub fn finding_path(&self, finding: &Finding) -> Option<InputPath> {
        if !self.reaches_finding(finding) {
            return None;
        }
        let path = self.path_to(finding.symbol?)?;
        // In a function that reads input, it's what it reads in this file.
        match self.reader_of(finding) {
            Some(reader) if path.functions.len() == 1 => Some(InputPath {
                source: reader.source.clone(),
                ..path
            }),
            _ => Some(path),
        }
    }

    /// The reading `finding`'s function does in `finding`'s file.
    fn reader_of(&self, finding: &Finding) -> Option<&Reader> {
        self.readers
            .get(&finding.symbol?)?
            .get(finding.file.as_str())
    }

    /// The code a finding in a reader's file flags, literals and comments
    /// blanked: the whole call when the snippet starts one (even across
    /// lines), else the rest of the line.
    fn flagged_code(&self, finding: &Finding) -> &str {
        let Some(code) = self.codes.get(&finding.file) else {
            return "";
        };
        let column = finding.columns.map_or(0, |(start, _)| start);
        code.offset(finding.line, column)
            .map_or("", |at| code.call_at(at))
    }

    pub fn reaches(&self, function: SymbolId) -> bool {
        self.hops.contains_key(&function)
    }

    /// How input gets to `function`, if it does.
    pub fn path_to(&self, function: SymbolId) -> Option<InputPath> {
        self.hops.get(&function)?;
        let mut functions = vec![function];
        let mut seen: HashSet<SymbolId> = HashSet::from([function]);
        let mut at = function;
        while let Some((Some(caller), _)) = self.hops.get(&at) {
            if !seen.insert(*caller) {
                break;
            }
            functions.push(*caller);
            at = *caller;
        }
        functions.reverse();
        // Named after where the path starts.
        let source = self.hops.get(&functions[0]).map(|(_, s)| s.clone())?;
        Some(InputPath { source, functions })
    }

    /// How many functions input reaches.
    pub fn len(&self) -> usize {
        self.hops.len()
    }

    /// Where input comes in, each once, in name order.
    pub fn sources(&self) -> Vec<&str> {
        self.sources.iter().map(String::as_str).collect()
    }
}

/// Each function's callees, in name order.
fn callees(graph: &CodeGraph) -> HashMap<SymbolId, Vec<SymbolId>> {
    let mut callees: HashMap<SymbolId, Vec<SymbolId>> = HashMap::new();
    for edge in graph.edges.iter().filter(|e| e.kind == EdgeKind::Calls) {
        callees.entry(edge.from).or_default().push(edge.to);
    }
    for list in callees.values_mut() {
        list.sort_by(|a, b| sort_key(graph, *a).cmp(&sort_key(graph, *b)));
        list.dedup();
    }
    callees
}

/// `recv() in net::serve » handle_packet » parse_header`.
pub fn describe(path: &InputPath, graph: &CodeGraph) -> String {
    let names: Vec<&str> = path
        .functions
        .iter()
        .map(|f| {
            graph
                .symbols
                .get(f)
                .map_or("?", |s| s.qualified_name.as_str())
        })
        .collect();
    format!("{} in {}", path.source, names.join(" » "))
}

#[cfg(test)]
mod tests {
    use super::sources::COMMAND_LINE;
    use super::*;
    use crate::index::graph_of_files;

    fn id(graph: &CodeGraph, name: &str) -> SymbolId {
        graph
            .symbols
            .values()
            .find(|s| s.qualified_name == name)
            .unwrap_or_else(|| panic!("no {name}"))
            .id
    }

    fn names(graph: &CodeGraph, path: &InputPath) -> Vec<String> {
        path.functions
            .iter()
            .map(|f| graph.symbols[f].qualified_name.clone())
            .collect()
    }

    const NET: &str = "int parse_header(const char *p) { return p[0]; }\n\
        int handle_packet(const char *p) { return parse_header(p); }\n\
        void serve(int fd) { char b[64]; recv(fd, b, 64, 0); handle_packet(b); }\n\
        int unrelated(int x) { return x + 1; }\n";

    #[test]
    fn input_reaches_what_its_reader_calls_down_the_graph() {
        let graph = graph_of_files(&[("net.c", NET)]);
        let reach = Reach::of(&graph);
        let path = reach.path_to(id(&graph, "parse_header")).expect("reached");
        assert_eq!(path.source, "recv()");
        assert_eq!(
            names(&graph, &path),
            ["serve", "handle_packet", "parse_header"]
        );
        assert_eq!(
            describe(&path, &graph),
            "recv() in serve » handle_packet » parse_header"
        );
    }

    #[test]
    fn the_reader_itself_is_reached() {
        let graph = graph_of_files(&[("net.c", NET)]);
        let path = Reach::of(&graph)
            .path_to(id(&graph, "serve"))
            .expect("reached");
        assert_eq!(names(&graph, &path), ["serve"]);
        assert_eq!(describe(&path, &graph), "recv() in serve");
    }

    #[test]
    fn functions_input_never_calls_are_not_reached() {
        let graph = graph_of_files(&[("net.c", NET)]);
        let reach = Reach::of(&graph);
        assert!(!reach.reaches(id(&graph, "unrelated")));
        assert_eq!(reach.len(), 3);
    }

    #[test]
    fn the_command_line_comes_in_through_main() {
        let graph = graph_of_files(&[(
            "m.c",
            "int run(char *arg) { return arg[0]; }\n\
             int main(int argc, char **argv) { return run(argv[1]); }\n",
        )]);
        let path = Reach::of(&graph)
            .path_to(id(&graph, "run"))
            .expect("reached");
        assert_eq!(path.source, COMMAND_LINE);
        assert_eq!(names(&graph, &path), ["main", "run"]);
    }

    #[test]
    fn a_main_without_parameters_reads_no_command_line() {
        let graph = graph_of_files(&[(
            "m.c",
            "int run(void) { return 0; }\nint main(void) { return run(); }\n",
        )]);
        assert_eq!(Reach::of(&graph).len(), 0);
    }

    #[test]
    fn the_shortest_path_wins_and_cycles_end() {
        let graph = graph_of_files(&[(
            "c.c",
            "int sink(int x);\n\
             int b(int x) { return a(x) + sink(x); }\n\
             int a(int x) { return b(x); }\n\
             int sink(int x) { return x; }\n\
             int entry(void) { char c; read(0, &c, 1); return a(c) + sink(c); }\n",
        )]);
        let path = Reach::of(&graph)
            .path_to(id(&graph, "sink"))
            .expect("reached");
        assert_eq!(names(&graph, &path), ["entry", "sink"]);
    }

    #[test]
    fn lists_each_source_once() {
        let graph = graph_of_files(&[(
            "s.c",
            "void a(int fd) { char b[4]; read(fd, b, 4); }\n\
             void b(int fd) { char c[4]; recv(fd, c, 4, 0); read(fd, c, 4); }\n\
             int main(int argc, char **argv) { return 0; }\n",
        )]);
        assert_eq!(
            Reach::of(&graph).sources(),
            ["read()", "recv()", COMMAND_LINE]
        );
    }

    #[test]
    fn a_reader_only_hands_input_to_calls_in_the_file_it_reads_in() {
        // Every `main` is one symbol to the indexer; input read by one
        // program's main must not reach what another program's main calls.
        let graph = graph_of_files(&[
            (
                "tools/a.c",
                "int parse_a(const char *p) { return p[0]; }\n\
                 int main(void) { char b[8]; fread(b, 1, 8, stdin); return parse_a(b); }\n",
            ),
            (
                "tools/b.c",
                "int helper_b(int x) { return x; }\nint main(void) { return helper_b(1); }\n",
            ),
        ]);
        let reach = Reach::of(&graph);
        assert!(reach.reaches(id(&graph, "parse_a")));
        assert!(!reach.reaches(id(&graph, "helper_b")));
    }

    #[test]
    fn a_reader_only_hands_input_to_calls_it_passes_it_to() {
        let graph = graph_of_files(&[(
            "s.c",
            "void log_start(void) {}\nint handle(const char *p) { return p[0]; }\n\
             void serve(int fd) { char b[8]; recv(fd, b, 8, 0); log_start(); handle(b); }\n",
        )]);
        let reach = Reach::of(&graph);
        assert!(reach.reaches(id(&graph, "handle")));
        assert!(!reach.reaches(id(&graph, "log_start")));
    }

    #[test]
    fn returned_input_follows_the_variable_it_is_kept_in() {
        let graph = graph_of_files(&[(
            "e.c",
            "int use_path(const char *p) { return p[0]; }\nint other(void) { return 0; }\n\
             int setup(void) { char *home = getenv(\"HOME\"); other(); return use_path(home); }\n",
        )]);
        let reach = Reach::of(&graph);
        let path = reach.path_to(id(&graph, "use_path")).expect("reached");
        assert_eq!(path.source, "getenv()");
        assert!(!reach.reaches(id(&graph, "other")));
    }

    #[test]
    fn input_passed_straight_into_a_call_reaches_it() {
        let graph = graph_of_files(&[(
            "i.c",
            "int parse(const char *p) { return p[0]; }\n\
             int setup(void) { return parse(getenv(\"OPTS\")); }\n",
        )]);
        assert!(Reach::of(&graph).reaches(id(&graph, "parse")));
    }

    #[test]
    fn scanf_input_lands_in_its_arguments() {
        let graph = graph_of_files(&[(
            "g.c",
            "void greet(const char *n) {}\n\
             void ask(void) { char name[32]; scanf(\"%31s\", name); greet(name); }\n",
        )]);
        assert!(Reach::of(&graph).reaches(id(&graph, "greet")));
    }

    #[test]
    fn main_hands_on_the_command_line_only_where_argv_goes() {
        let graph = graph_of_files(&[(
            "m.c",
            "void init_ui(void) {}\nint run(char *arg) { return arg[0]; }\n\
             int main(int argc, char **argv) { init_ui(); return run(argv[1]); }\n",
        )]);
        let reach = Reach::of(&graph);
        assert!(reach.reaches(id(&graph, "run")));
        assert!(!reach.reaches(id(&graph, "init_ui")));
    }

    fn finding_at(graph: &CodeGraph, file: &str, line: u32, function: &str) -> Finding {
        Finding {
            tool: "test".into(),
            rule: "risky".into(),
            severity: crate::analysis::Severity::High,
            message: String::new(),
            fix: None,
            cwe: None,
            file: file.into(),
            line,
            columns: None,
            symbol: Some(id(graph, function)),
        }
    }

    #[test]
    fn in_the_reader_itself_only_code_using_the_input_is_reached() {
        let graph = graph_of_files(&[(
            "r.c",
            "void serve(int fd) { char b[8], d[8], e[8];\n\
             recv(fd, b, 8, 0);\n\
             strcpy(d, b);\n\
             strcpy(e, d0);\n}\n",
        )]);
        let reach = Reach::of(&graph);
        assert!(reach
            .finding_path(&finding_at(&graph, "r.c", 3, "serve"))
            .is_some());
        assert!(reach
            .finding_path(&finding_at(&graph, "r.c", 4, "serve"))
            .is_none());
    }

    #[test]
    fn code_input_is_handed_to_is_reached_whatever_it_uses() {
        let graph = graph_of_files(&[("net.c", NET)]);
        let reach = Reach::of(&graph);
        let path = reach
            .finding_path(&finding_at(&graph, "net.c", 1, "parse_header"))
            .expect("reached");
        assert_eq!(path.functions.len(), 3);
    }

    #[test]
    fn in_main_only_code_using_the_command_line_is_reached() {
        let graph = graph_of_files(&[(
            "m.c",
            "int main(int argc, char **argv) {\n\
             srand(1);\n\
             return atoi(argv[1]);\n}\n",
        )]);
        let reach = Reach::of(&graph);
        assert!(reach
            .finding_path(&finding_at(&graph, "m.c", 2, "main"))
            .is_none());
        assert!(reach
            .finding_path(&finding_at(&graph, "m.c", 3, "main"))
            .is_some());
    }

    /// Whether input reaches `callee` in a project made of `source`.
    fn reaches(source: &str, callee: &str) -> bool {
        let graph = graph_of_files(&[("t.cc", source)]);
        Reach::of(&graph).reaches(id(&graph, callee))
    }

    #[test]
    fn an_unterminated_argument_list_hands_nothing_on() {
        let source = "void handle(int x, int y) {}\nvoid later(char *b) {}\n\
            void serve(int fd) { char b[8]; recv(fd, b, 8, 0);\n\
            handle(1 /* can't */, 2);\n\
            later(b); }\n";
        assert!(!reaches(source, "handle"));
        assert!(reaches(source, "later"));
    }

    #[test]
    fn names_in_strings_and_comments_are_not_the_input() {
        let source = "void report(const char *m) {}\n\
            void serve(int fd) { char buf[8]; recv(fd, buf, 8, 0);\n\
            report(\"failed to read buf\" /* buf */); }\n";
        assert!(!reaches(source, "report"));
    }

    #[test]
    fn the_variable_assigned_is_the_one_kept() {
        let source = "void parse(char *b) {}\nvoid count(int n) {}\nvoid show(const char *h) {}\n\
            void take(void) { char buf[8]; int n = 0;\n\
            buf[n++] = getchar();\n\
            const char *h = (const char *)std::getenv(\"HOME\");\n\
            parse(buf); count(n); show(h); }\n";
        assert!(reaches(source, "parse"));
        assert!(!reaches(source, "count"));
        assert!(reaches(source, "show"));
    }

    #[test]
    fn an_assignment_may_span_lines() {
        let source = "void show(const char *h) {}\n\
            void take(void) { const char *h =\n    getenv(\"HOME\");\n show(h); }\n";
        assert!(reaches(source, "show"));
    }

    #[test]
    fn only_standard_library_names_are_sources() {
        let source = "void use(int x) {}\nvoid show(const std::string &s) {}\n\
            void a(int fd) { char b[8]; mylib::read(fd, b, 8); use(b[0]); }\n\
            void c(void) { std::string s; std::getline(std::cin, s); show(s); }\n";
        let graph = graph_of_files(&[("s.cc", source)]);
        let reach = Reach::of(&graph);
        assert!(!reach.reaches(id(&graph, "use")));
        assert!(reach.reaches(id(&graph, "show")));
    }

    #[test]
    fn template_calls_are_handed_the_input() {
        let source = "template <typename T> T parse(const char *p) { return T(); }\n\
            void serve(int fd) { char b[8]; recv(fd, b, 8, 0); parse<int>(b); }\n";
        assert!(reaches(source, "parse"));
    }

    #[test]
    fn a_name_inside_a_longer_name_is_not_the_source() {
        let source = "void push(int c) {}\n\
            void take(FILE *f) { int c = 1; push(ungetc(c, f)); }\n";
        assert!(!reaches(source, "push"));
    }

    #[test]
    fn a_risky_call_wrapped_over_lines_in_the_reader_is_reached() {
        let graph = graph_of_files(&[(
            "w.c",
            "void serve(int fd) { char buf[8], dst[8];\n\
             recv(fd, buf, 8, 0);\n\
             memcpy(dst,\n\
                 buf, 8);\n}\n",
        )]);
        let mut finding = finding_at(&graph, "w.c", 3, "serve");
        finding.columns = Some((0, 6));
        assert!(Reach::of(&graph).finding_path(&finding).is_some());
    }

    #[test]
    fn a_reader_that_is_also_handed_input_counts_all_its_code() {
        let graph = graph_of_files(&[(
            "h.c",
            "void parse(const char *p) { char x[8], b[8];\n\
             read(0, b, 8);\n\
             strcpy(x, p);\n}\n\
             void serve(int fd) { char buf[8]; recv(fd, buf, 8, 0); parse(buf); }\n",
        )]);
        let reach = Reach::of(&graph);
        assert!(reach
            .finding_path(&finding_at(&graph, "h.c", 3, "parse"))
            .is_some());
    }

    #[test]
    fn each_main_is_labelled_with_its_own_source() {
        let graph = graph_of_files(&[
            (
                "a.c",
                "int main(void) { char b[8]; fread(b, 1, 8, stdin); return b[0]; }\n",
            ),
            (
                "b.c",
                "int main(int argc, char **argv) {\nreturn atoi(argv[1]);\n}\n",
            ),
        ]);
        let path = Reach::of(&graph)
            .finding_path(&finding_at(&graph, "b.c", 2, "main"))
            .expect("reached");
        assert_eq!(path.source, COMMAND_LINE);
    }

    #[test]
    fn library_names_count_with_a_namespace() {
        let graph = graph_of_files(&[(
            "s.cc",
            "int load(FILE *f) { char b[8]; return std::fread(b, 1, 8, f); }\n",
        )]);
        let path = Reach::of(&graph)
            .path_to(id(&graph, "load"))
            .expect("reached");
        assert_eq!(path.source, "fread()");
    }
}

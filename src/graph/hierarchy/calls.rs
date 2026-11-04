//! Call-graph analysis for the hierarchy view: callees in source order,
//! entry points, recursion, and breadth-first layering.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::model::{CodeGraph, EdgeKind, OccurrenceRole, Span, SymbolId, SymbolKind};

/// How much of the call graph a tree shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Call levels below the roots.
    pub max_depth: usize,
    /// Callees under one function; the rest are counted.
    pub max_children: usize,
    /// Functions in the whole tree.
    pub max_nodes: usize,
}

impl Limits {
    /// Below one selected function: deep and wide, you're exploring it.
    pub const FOCUSED: Limits = Limits {
        max_depth: 6,
        max_children: 12,
        max_nodes: 200,
    };
    /// From the entry points: a readable overview to click into.
    pub const OVERVIEW: Limits = Limits {
        max_depth: 3,
        max_children: 8,
        max_nodes: 60,
    };
}

/// Entry points shown at most when nothing is selected.
pub const MAX_ROOTS: usize = 6;
/// Bound on the reach count used to rank entry points: enough to tell a
/// real entry point from a small helper, cheap for thousands of candidates.
const MAX_REACH_COUNTED: usize = 300;

pub struct CallIndex {
    /// Callees of each function, ordered by where they're first called.
    callees: HashMap<SymbolId, Vec<SymbolId>>,
    has_callers: HashSet<SymbolId>,
    /// Functions that call themselves directly.
    recursive: HashSet<SymbolId>,
}

impl CallIndex {
    pub fn new(graph: &CodeGraph) -> Self {
        // Where each (caller, callee) call first appears, from the occurrences.
        let mut first_call: HashMap<(SymbolId, SymbolId), (&str, Span)> = HashMap::new();
        let mut recursive = HashSet::new();
        for (file, occurrences) in &graph.occurrences {
            for o in occurrences {
                let (OccurrenceRole::Reference(EdgeKind::Calls), Some(caller)) =
                    (o.role, o.context)
                else {
                    continue;
                };
                if caller == o.symbol {
                    recursive.insert(caller);
                }
                let at = (file.as_str(), o.span);
                let first = first_call.entry((caller, o.symbol)).or_insert(at);
                *first = (*first).min(at);
            }
        }

        let mut callees: HashMap<SymbolId, Vec<SymbolId>> = HashMap::new();
        let mut has_callers = HashSet::new();
        for e in graph.edges.iter().filter(|e| e.kind == EdgeKind::Calls) {
            callees.entry(e.from).or_default().push(e.to);
            has_callers.insert(e.to);
        }
        let name = |id: &SymbolId| graph.symbols.get(id).map(|s| s.qualified_name.as_str());
        for (caller, list) in &mut callees {
            list.sort_by_key(|callee| {
                // Calls with a known position first, in source order.
                let position = first_call.get(&(*caller, *callee));
                (position.is_none(), position.copied(), name(callee), *callee)
            });
            list.dedup();
        }
        Self {
            callees,
            has_callers,
            recursive,
        }
    }

    pub fn callees(&self, id: SymbolId) -> &[SymbolId] {
        self.callees.get(&id).map_or(&[], Vec::as_slice)
    }

    pub fn is_recursive(&self, id: SymbolId) -> bool {
        self.recursive.contains(&id)
    }

    /// How many functions `id` reaches through calls (bounded).
    fn reach(&self, id: SymbolId) -> usize {
        let mut seen = HashSet::from([id]);
        let mut queue = VecDeque::from([id]);
        while let Some(next) = queue.pop_front() {
            if seen.len() >= MAX_REACH_COUNTED {
                break;
            }
            for callee in self.callees(next) {
                if seen.insert(*callee) {
                    queue.push_back(*callee);
                }
            }
        }
        seen.len() - 1
    }
}

/// Starting points, plus how many more were left out.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Roots {
    pub ids: Vec<SymbolId>,
    pub hidden: usize,
}

/// Functions nobody calls but that call something, the ones reaching the
/// most code first (`main` wins ties). If every caller is itself called
/// (the calls only form cycles), the functions reaching the most code.
pub fn entry_points(graph: &CodeGraph, calls: &CallIndex) -> Roots {
    let callers = || {
        graph
            .symbols
            .values()
            .filter(|s| is_function(s.kind) && !calls.callees(s.id).is_empty())
            .map(|s| s.id)
    };
    let uncalled: Vec<SymbolId> = callers()
        .filter(|id| !calls.has_callers.contains(id))
        .collect();
    if uncalled.is_empty() {
        return ranked_roots(callers(), graph, calls);
    }
    ranked_roots(uncalled.into_iter(), graph, calls)
}

/// Roots for a selected symbol: a function itself, or the methods of a
/// class that call something. Other kinds have no call hierarchy.
pub fn roots_for(graph: &CodeGraph, calls: &CallIndex, id: SymbolId) -> Roots {
    let Some(symbol) = graph.symbols.get(&id) else {
        return Roots::default();
    };
    match symbol.kind {
        kind if is_function(kind) => Roots {
            ids: vec![id],
            hidden: 0,
        },
        SymbolKind::Class | SymbolKind::Struct => {
            let methods: Vec<SymbolId> = graph
                .edges
                .iter()
                .filter(|e| e.kind == EdgeKind::Contains && e.from == id)
                .map(|e| e.to)
                .filter(|m| !calls.callees(*m).is_empty())
                .collect();
            // Helpers called by a sibling method appear under that sibling.
            let is_sibling_called = |m: &SymbolId| {
                methods
                    .iter()
                    .any(|other| calls.callees(*other).contains(m))
            };
            let outer: Vec<SymbolId> = methods
                .iter()
                .copied()
                .filter(|m| !is_sibling_called(m))
                .collect();
            let roots = if outer.is_empty() { methods } else { outer };
            ranked_roots(roots.into_iter(), graph, calls)
        }
        _ => Roots::default(),
    }
}

fn ranked_roots(
    ids: impl Iterator<Item = SymbolId>,
    graph: &CodeGraph,
    calls: &CallIndex,
) -> Roots {
    let name = |id: SymbolId| {
        graph
            .symbols
            .get(&id)
            .map_or("", |s| s.qualified_name.as_str())
    };
    let mut ranked: Vec<(usize, SymbolId)> = ids.map(|id| (calls.reach(id), id)).collect();
    ranked.sort_by_cached_key(|&(reach, id)| {
        (
            std::cmp::Reverse(reach),
            name(id) != "main",
            name(id).to_string(),
        )
    });
    let hidden = ranked.len().saturating_sub(MAX_ROOTS);
    Roots {
        ids: ranked
            .into_iter()
            .take(MAX_ROOTS)
            .map(|(_, id)| id)
            .collect(),
        hidden,
    }
}

fn is_function(kind: SymbolKind) -> bool {
    matches!(kind, SymbolKind::Function | SymbolKind::Method)
}

/// A breadth-first spanning tree of the calls below the roots: each
/// function appears once, under the caller that reaches it first (so at its
/// shallowest depth). Other calls between shown functions are not tree edges.
#[derive(Debug, PartialEq, Eq)]
pub struct CallTree {
    pub limits: Limits,
    pub roots: Vec<SymbolId>,
    /// Shown callees of each function, in call order.
    pub children: HashMap<SymbolId, Vec<SymbolId>>,
    pub depth: HashMap<SymbolId, usize>,
    /// Callees of each function left out by the caps.
    pub hidden_children: HashMap<SymbolId, usize>,
    /// Distinct functions left out by the caps.
    pub hidden: usize,
}

impl CallTree {
    pub fn children_of(&self, id: SymbolId) -> &[SymbolId] {
        self.children.get(&id).map_or(&[], Vec::as_slice)
    }

    pub fn contains(&self, id: SymbolId) -> bool {
        self.depth.contains_key(&id)
    }
}

pub fn build_tree(calls: &CallIndex, roots: &[SymbolId], limits: Limits) -> CallTree {
    let mut tree = CallTree {
        limits,
        roots: roots.to_vec(),
        children: HashMap::new(),
        depth: roots.iter().map(|r| (*r, 0)).collect(),
        hidden_children: HashMap::new(),
        hidden: 0,
    };
    let mut skipped: HashSet<SymbolId> = HashSet::new();
    let mut queue: VecDeque<SymbolId> = roots.iter().copied().collect();
    while let Some(caller) = queue.pop_front() {
        let depth = tree.depth.get(&caller).copied().unwrap_or(0);
        if depth >= limits.max_depth {
            continue;
        }
        let mut shown = Vec::new();
        let mut hidden = 0;
        for callee in calls.callees(caller) {
            if tree.contains(*callee) {
                continue;
            }
            if shown.len() < limits.max_children && tree.depth.len() < limits.max_nodes {
                tree.depth.insert(*callee, depth + 1);
                shown.push(*callee);
                queue.push_back(*callee);
            } else {
                hidden += 1;
                skipped.insert(*callee);
            }
        }
        if !shown.is_empty() {
            tree.children.insert(caller, shown);
        }
        if hidden > 0 {
            tree.hidden_children.insert(caller, hidden);
        }
    }
    tree.hidden = skipped.iter().filter(|id| !tree.contains(**id)).count();
    tree
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::index::{build_graph, Extractor, SourceFile};

    pub(crate) fn graph_of(source: &str) -> CodeGraph {
        let facts = Extractor::new()
            .expect("grammar")
            .extract(source)
            .expect("parse");
        build_graph(vec![SourceFile {
            path: "a.cpp".into(),
            contents: source.into(),
            facts,
        }])
    }

    fn id(graph: &CodeGraph, name: &str) -> SymbolId {
        graph
            .symbols
            .values()
            .find(|s| s.qualified_name == name)
            .map(|s| s.id)
            .unwrap_or_else(|| panic!("no symbol {name}"))
    }

    fn names(graph: &CodeGraph, ids: &[SymbolId]) -> Vec<String> {
        ids.iter()
            .map(|i| graph.symbols[i].qualified_name.clone())
            .collect()
    }

    const PROGRAM: &str = "void zeta() {}\n\
        void alpha() { zeta(); }\n\
        int fact(int n) { return n ? fact(n - 1) : 1; }\n\
        void run() { zeta(); alpha(); fact(3); }\n\
        int main() { run(); return 0; }\n\
        void unused_helper() { zeta(); }\n";

    #[test]
    fn callees_follow_source_order_not_name_order() {
        let graph = graph_of(PROGRAM);
        let calls = CallIndex::new(&graph);
        assert_eq!(
            names(&graph, calls.callees(id(&graph, "run"))),
            ["zeta", "alpha", "fact"]
        );
    }

    #[test]
    fn detects_direct_recursion() {
        let graph = graph_of(PROGRAM);
        let calls = CallIndex::new(&graph);
        assert!(calls.is_recursive(id(&graph, "fact")));
        assert!(!calls.is_recursive(id(&graph, "run")));
    }

    #[test]
    fn entry_points_are_uncalled_callers_ranked_by_reach() {
        let graph = graph_of(PROGRAM);
        let roots = entry_points(&graph, &CallIndex::new(&graph));
        assert_eq!(names(&graph, &roots.ids), ["main", "unused_helper"]);
        assert_eq!(roots.hidden, 0);
    }

    #[test]
    fn tree_places_each_function_once_under_its_first_caller() {
        let graph = graph_of(PROGRAM);
        let calls = CallIndex::new(&graph);
        let tree = build_tree(&calls, &[id(&graph, "main")], Limits::FOCUSED);
        assert_eq!(names(&graph, tree.children_of(id(&graph, "main"))), ["run"]);
        assert_eq!(
            names(&graph, tree.children_of(id(&graph, "run"))),
            ["zeta", "alpha", "fact"]
        );
        // alpha also calls zeta, but zeta already hangs under run.
        assert!(tree.children_of(id(&graph, "alpha")).is_empty());
        assert_eq!(tree.depth[&id(&graph, "zeta")], 2);
    }

    #[test]
    fn roots_for_a_class_are_its_calling_methods() {
        let graph = graph_of(
            "void log() {}\nstruct App { void start() { log(); } void idle() {} int x; };\n",
        );
        let calls = CallIndex::new(&graph);
        let roots = roots_for(&graph, &calls, id(&graph, "App"));
        assert_eq!(names(&graph, &roots.ids), ["App::start"]);
        assert!(roots_for(&graph, &calls, id(&graph, "App::x"))
            .ids
            .is_empty());
    }

    #[test]
    fn caps_callees_per_function_and_depth() {
        let extra = 5;
        let defs: String = (0..Limits::FOCUSED.max_children + extra)
            .map(|i| format!("void f{i}() {{}}\n"))
            .collect();
        let calls_all: String = (0..Limits::FOCUSED.max_children + extra)
            .map(|i| format!("f{i}();"))
            .collect();
        let graph = graph_of(&format!("{defs}void hub() {{ {calls_all} }}\n"));
        let hub = id(&graph, "hub");
        let tree = build_tree(&CallIndex::new(&graph), &[hub], Limits::FOCUSED);
        assert_eq!(tree.children_of(hub).len(), Limits::FOCUSED.max_children);
        assert_eq!((tree.hidden_children[&hub], tree.hidden), (extra, extra));

        let chain: String = (0..Limits::FOCUSED.max_depth + 4)
            .map(|i| format!("void c{}() {{ c{}(); }}\n", i, i + 1))
            .collect();
        let graph = graph_of(&format!(
            "void c{}() {{}}\n{chain}",
            Limits::FOCUSED.max_depth + 4
        ));
        let tree = build_tree(
            &CallIndex::new(&graph),
            &[id(&graph, "c0")],
            Limits::FOCUSED,
        );
        assert_eq!(tree.depth.values().max(), Some(&Limits::FOCUSED.max_depth));
    }

    #[test]
    fn class_roots_skip_helpers_called_by_sibling_methods() {
        let graph = graph_of(
            "void log() {}\nstruct App { void helper() { log(); } void start() { helper(); } };\n",
        );
        let calls = CallIndex::new(&graph);
        let roots = roots_for(&graph, &calls, id(&graph, "App"));
        assert_eq!(names(&graph, &roots.ids), ["App::start"]);
        let tree = build_tree(&calls, &roots.ids, Limits::FOCUSED);
        assert_eq!(
            names(&graph, tree.children_of(id(&graph, "App::start"))),
            ["App::helper"]
        );
    }

    #[test]
    fn calls_that_only_form_cycles_still_have_entry_points() {
        let graph = graph_of("void b();\nvoid a() { b(); }\nvoid b() { a(); }\n");
        let roots = entry_points(&graph, &CallIndex::new(&graph));
        assert_eq!(roots.ids.len(), 2);
    }

    #[test]
    fn mutual_recursion_terminates() {
        let graph = graph_of("void b();\nvoid a() { b(); }\nvoid b() { a(); }\n");
        let tree = build_tree(&CallIndex::new(&graph), &[id(&graph, "a")], Limits::FOCUSED);
        assert_eq!(tree.depth.len(), 2);
    }
}

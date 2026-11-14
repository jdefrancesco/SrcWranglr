//! The currently loaded project: its graph plus indexes computed once per
//! load (symbols in name order, how many symbols use each one, the library
//! functions it calls).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::analysis::risky::bare_library_name;
use crate::demo::demo_codebase;
use crate::model::{CodeGraph, EdgeKind, Symbol, SymbolId};

/// A library function the project calls: one it doesn't define.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryFunction {
    /// Bare, as cross references know it: `memcpy` for `std::memcpy`.
    pub name: String,
    pub calls: usize,
}

pub struct Project {
    pub name: String,
    /// Folder it was indexed from; `None` for the built-in demo.
    pub root: Option<PathBuf>,
    /// Shared, so work in the background can read it without a copy.
    pub graph: Arc<CodeGraph>,
    /// All symbol ids sorted by qualified name (computed once, not per frame).
    sorted_ids: Vec<SymbolId>,
    /// `(id, qualified name)` in name order, for quick-open search.
    search_names: Vec<(SymbolId, String)>,
    /// How many distinct symbols call, use, or derive from each symbol.
    usage: HashMap<SymbolId, usize>,
    /// Symbols by qualified name (overloads and repeated names share one).
    by_qualified_name: HashMap<String, Vec<SymbolId>>,
    /// Most called first.
    library_functions: Vec<LibraryFunction>,
    /// `(index in library_functions, name)`, for quick-open search.
    library_names: Vec<(usize, String)>,
}

impl Project {
    pub fn new(name: impl Into<String>, root: Option<PathBuf>, graph: CodeGraph) -> Self {
        let mut sorted_ids: Vec<SymbolId> = graph.symbols.keys().copied().collect();
        sorted_ids.sort_by(|a, b| {
            let name = |id| graph.symbols.get(id).map(|s| s.qualified_name.as_str());
            name(a).cmp(&name(b)).then(a.cmp(b))
        });
        let search_names = sorted_ids
            .iter()
            .filter_map(|id| Some((*id, graph.symbols.get(id)?.qualified_name.clone())))
            .collect();
        let usage = usage_counts(&graph);
        let by_qualified_name = sorted_ids
            .iter()
            .filter_map(|id| Some((graph.symbols.get(id)?.qualified_name.clone(), *id)))
            .fold(
                HashMap::new(),
                |mut map: HashMap<String, Vec<SymbolId>>, (name, id)| {
                    map.entry(name).or_default().push(id);
                    map
                },
            );
        let library_functions = library_functions(&graph);
        let library_names = library_functions
            .iter()
            .enumerate()
            .map(|(i, f)| (i, f.name.clone()))
            .collect();
        Self {
            name: name.into(),
            root,
            graph: Arc::new(graph),
            sorted_ids,
            search_names,
            usage,
            by_qualified_name,
            library_functions,
            library_names,
        }
    }

    pub fn demo() -> Result<Self, String> {
        demo_codebase().map(|graph| Self::new("Demo", None, graph))
    }

    pub fn empty() -> Self {
        Self::new("No project", None, CodeGraph::default())
    }

    /// The symbol with exactly this qualified name, else the only symbol
    /// with this short name.
    pub fn find(&self, name: &str) -> Option<SymbolId> {
        if let Some(exact) = self.symbols_by_name().find(|s| s.qualified_name == name) {
            return Some(exact.id);
        }
        let mut by_name = self.symbols_by_name().filter(|s| s.name == name);
        let first = by_name.next()?;
        by_name.next().is_none().then_some(first.id)
    }

    /// `(id, qualified name)` for every symbol, in name order.
    pub fn search_candidates(&self) -> &[(SymbolId, String)] {
        &self.search_names
    }

    /// The library functions the project calls, most called first.
    pub fn library_functions(&self) -> &[LibraryFunction] {
        &self.library_functions
    }

    /// `(index in library_functions, name)`, most called first.
    pub fn library_candidates(&self) -> &[(usize, String)] {
        &self.library_names
    }

    /// Every symbol, in qualified-name order.
    pub fn symbols_by_name(&self) -> impl Iterator<Item = &Symbol> {
        self.sorted_ids
            .iter()
            .filter_map(|id| self.graph.symbols.get(id))
    }

    /// Every symbol with exactly this qualified name.
    pub fn ids_named(&self, qualified_name: &str) -> &[SymbolId] {
        self.by_qualified_name
            .get(qualified_name)
            .map_or(&[], Vec::as_slice)
    }

    /// How many distinct symbols reference `id` (calls, uses, base
    /// classes; being a member doesn't count).
    pub fn usage(&self, id: SymbolId) -> usize {
        self.usage.get(&id).copied().unwrap_or(0)
    }
}

/// Calls to functions the project doesn't define, counted by bare name
/// (a namespace's other than `std` has none), most called first, then by
/// name.
fn library_functions(graph: &CodeGraph) -> Vec<LibraryFunction> {
    let mut calls: HashMap<&str, usize> = HashMap::new();
    for call in graph.external_calls.values().flatten() {
        if let Some(name) = bare_library_name(&call.name) {
            *calls.entry(name).or_default() += 1;
        }
    }
    let mut functions: Vec<LibraryFunction> = calls
        .into_iter()
        .map(|(name, calls)| LibraryFunction {
            name: name.to_string(),
            calls,
        })
        .collect();
    functions.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.name.cmp(&b.name)));
    functions
}

fn usage_counts(graph: &CodeGraph) -> HashMap<SymbolId, usize> {
    let users: HashSet<(SymbolId, SymbolId)> = graph
        .edges
        .iter()
        .filter(|e| e.kind != EdgeKind::Contains && e.from != e.to)
        .map(|e| (e.from, e.to))
        .collect();
    users
        .into_iter()
        .fold(HashMap::new(), |mut counts, (_, to)| {
            *counts.entry(to).or_insert(0) += 1;
            counts
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_prefers_exact_qualified_name_then_unique_short_name() {
        let project = Project::demo().expect("demo");
        let name_of = |id: SymbolId| project.graph.symbols[&id].qualified_name.clone();
        assert_eq!(
            project.find("Circle::area").map(name_of).as_deref(),
            Some("Circle::area")
        );
        assert_eq!(
            project.find("radius").map(name_of).as_deref(),
            Some("Circle::radius")
        );
        // "area" is ambiguous (Shape/Circle/Square).
        assert!(project.find("area").is_none());
        assert!(project.find("nope").is_none());
    }

    #[test]
    fn usage_counts_distinct_symbols_that_reference_one() {
        let project = Project::demo().expect("demo");
        let usage = |name: &str| project.usage(project.find(name).expect(name));
        // Circle and Square both derive from Shape.
        assert_eq!(usage("Shape"), 2);
        // main calls total_area (members don't count as uses).
        assert_eq!(usage("total_area"), 1);
        assert_eq!(usage("main"), 0);
    }

    #[test]
    fn ids_named_finds_symbols_by_exact_qualified_name() {
        let project = Project::demo().expect("demo");
        assert_eq!(project.ids_named("Circle::area").len(), 1);
        assert!(project.ids_named("area").is_empty());
    }

    #[test]
    fn library_functions_are_counted_by_bare_name_most_called_first() {
        let graph = crate::index::graph_of_files(&[
            (
                "x.c",
                "void f(char *d, char *s) { system(d); memcpy(d, s, 1); system(s); }\n",
            ),
            (
                "y.cc",
                "void g(char *d) { std::memcpy(d, d, 1); ::abort(); boost::thing(d); f(d, d); }\n",
            ),
        ]);
        let project = Project::new("t", None, graph);
        let found: Vec<(&str, usize)> = project
            .library_functions()
            .iter()
            .map(|f| (f.name.as_str(), f.calls))
            .collect();
        // Not boost::thing (no bare name), not f (the project's own).
        assert_eq!(found, [("memcpy", 2), ("system", 2), ("abort", 1)]);
        let names: Vec<(usize, &str)> = project
            .library_candidates()
            .iter()
            .map(|(i, name)| (*i, name.as_str()))
            .collect();
        assert_eq!(names, [(0, "memcpy"), (1, "system"), (2, "abort")]);
    }

    #[test]
    fn empty_project_has_no_symbols() {
        let project = Project::empty();
        assert_eq!(project.symbols_by_name().count(), 0);
        assert!(project.library_functions().is_empty());
        assert!(project.root.is_none());
    }
}

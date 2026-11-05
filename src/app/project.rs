//! The currently loaded project: its graph plus indexes computed once per
//! load (symbols in name order, how many symbols use each one).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::demo::demo_codebase;
use crate::model::{CodeGraph, EdgeKind, Symbol, SymbolId};

pub struct Project {
    pub name: String,
    /// Folder it was indexed from; `None` for the built-in demo.
    pub root: Option<PathBuf>,
    pub graph: CodeGraph,
    /// All symbol ids sorted by qualified name (computed once, not per frame).
    sorted_ids: Vec<SymbolId>,
    /// `(id, qualified name)` in name order, for quick-open search.
    search_names: Vec<(SymbolId, String)>,
    /// How many distinct symbols call, use, or derive from each symbol.
    usage: HashMap<SymbolId, usize>,
    /// Symbols by qualified name (overloads and repeated names share one).
    by_qualified_name: HashMap<String, Vec<SymbolId>>,
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
        Self {
            name: name.into(),
            root,
            graph,
            sorted_ids,
            search_names,
            usage,
            by_qualified_name,
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
    fn empty_project_has_no_symbols() {
        let project = Project::empty();
        assert_eq!(project.symbols_by_name().count(), 0);
        assert!(project.root.is_none());
    }
}

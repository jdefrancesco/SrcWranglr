//! The currently loaded project: its graph plus a name-sorted symbol index
//! for the symbol list.

use std::path::PathBuf;

use crate::demo::demo_codebase;
use crate::model::{CodeGraph, SymbolId};

pub struct Project {
    pub name: String,
    /// Folder it was indexed from; `None` for the built-in demo.
    pub root: Option<PathBuf>,
    pub graph: CodeGraph,
    /// All symbol ids sorted by qualified name (computed once, not per frame).
    sorted_ids: Vec<SymbolId>,
    /// `(id, qualified name)` in name order, for quick-open search.
    search_names: Vec<(SymbolId, String)>,
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
        Self {
            name: name.into(),
            root,
            graph,
            sorted_ids,
            search_names,
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
        let symbols = || {
            self.sorted_ids
                .iter()
                .filter_map(|id| self.graph.symbols.get(id))
        };
        if let Some(exact) = symbols().find(|s| s.qualified_name == name) {
            return Some(exact.id);
        }
        let mut by_name = symbols().filter(|s| s.name == name);
        let first = by_name.next()?;
        by_name.next().is_none().then_some(first.id)
    }

    /// `(id, qualified name)` for every symbol, in name order.
    pub fn search_candidates(&self) -> &[(SymbolId, String)] {
        &self.search_names
    }

    /// Ids whose qualified name contains `query` (case-insensitive), in name order.
    pub fn filter(&self, query: &str) -> Vec<SymbolId> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return self.sorted_ids.clone();
        }
        self.sorted_ids
            .iter()
            .copied()
            .filter(|id| {
                self.graph
                    .symbols
                    .get(id)
                    .is_some_and(|s| s.qualified_name.to_lowercase().contains(&needle))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(project: &Project, ids: &[SymbolId]) -> Vec<String> {
        ids.iter()
            .map(|id| project.graph.symbols[id].qualified_name.clone())
            .collect()
    }

    #[test]
    fn empty_query_lists_all_symbols_sorted_by_name() {
        let project = Project::demo().expect("demo");
        let all = names(&project, &project.filter(""));
        assert_eq!(all.len(), project.graph.symbols.len());
        assert!(all.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn filter_is_case_insensitive_substring_match() {
        let project = Project::demo().expect("demo");
        let hits = names(&project, &project.filter("  cIRcle::A "));
        assert_eq!(hits, ["Circle::area"]);
    }

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
    fn filter_with_no_matches_is_empty() {
        let project = Project::demo().expect("demo");
        assert!(project.filter("zzz_nothing").is_empty());
    }

    #[test]
    fn empty_project_has_no_symbols() {
        let project = Project::empty();
        assert!(project.filter("").is_empty());
        assert!(project.root.is_none());
    }
}

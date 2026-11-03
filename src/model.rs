//! Data model for the indexed codebase.
//!
//! A set of symbols (functions, classes, fields, ...) and typed edges between
//! them (calls, inherits, uses, contains), as produced by `crate::index`.

use std::collections::HashMap;

pub type SymbolId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Class,
    Struct,
    Function,
    Method,
    Field,
    Namespace,
}

impl SymbolKind {
    pub fn label(self) -> &'static str {
        match self {
            SymbolKind::Class => "class",
            SymbolKind::Struct => "struct",
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Field => "field",
            SymbolKind::Namespace => "namespace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EdgeKind {
    Calls,
    Inherits,
    Uses,
    Contains,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub from: SymbolId,
    pub to: SymbolId,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Default)]
pub struct CodeGraph {
    pub symbols: HashMap<SymbolId, Symbol>,
    pub edges: Vec<Edge>,
    /// Source text per file, keyed by project-relative path, for the code view.
    pub file_contents: HashMap<String, String>,
}

impl CodeGraph {
    pub fn edges_for(&self, id: SymbolId) -> impl Iterator<Item = &Edge> {
        self.edges
            .iter()
            .filter(move |e| e.from == id || e.to == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_for_covers_both_directions() {
        let edge = |from, to| Edge {
            from,
            to,
            kind: EdgeKind::Calls,
        };
        let graph = CodeGraph {
            edges: vec![edge(0, 1), edge(1, 2), edge(2, 3)],
            ..CodeGraph::default()
        };
        let touching: Vec<(SymbolId, SymbolId)> =
            graph.edges_for(1).map(|e| (e.from, e.to)).collect();
        assert_eq!(touching, [(0, 1), (1, 2)]);
    }
}

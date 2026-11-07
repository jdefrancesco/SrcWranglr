//! Calls between the project's functions, as a trace follows them: where
//! each function is called from, and the calls a walk has stepped into.

use std::collections::HashMap;

use tree_sitter::Node;

use crate::model::{CodeGraph, EdgeKind, OccurrenceRole, Span, SymbolId};

/// A call, by where it starts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct CallRef {
    pub file: String,
    pub at: usize,
}

/// A place a function is called from: file, the name's span, and the
/// function the call is in.
pub(super) type CallSite = (String, Span, Option<SymbolId>);

/// Where each project function is called from, worked out when first
/// asked.
#[derive(Default)]
pub(super) struct CallSites(Option<HashMap<SymbolId, Vec<CallSite>>>);

impl CallSites {
    /// Where `symbol` is called from, files in name order.
    pub(super) fn of(&mut self, graph: &CodeGraph, symbol: SymbolId) -> Vec<CallSite> {
        self.0
            .get_or_insert_with(|| call_sites(graph))
            .get(&symbol)
            .cloned()
            .unwrap_or_default()
    }
}

/// `returns_to` with `call` (in `file`) stepped into, innermost.
pub(super) fn stepped_into(returns_to: &[CallRef], file: &str, call: Node) -> Vec<CallRef> {
    returns_to
        .iter()
        .cloned()
        .chain([CallRef {
            file: file.to_string(),
            at: call.start_byte(),
        }])
        .collect()
}

/// Where every project function is called from, files in name order.
fn call_sites(graph: &CodeGraph) -> HashMap<SymbolId, Vec<CallSite>> {
    let mut files: Vec<&String> = graph.occurrences.keys().collect();
    files.sort_unstable();
    let mut sites: HashMap<SymbolId, Vec<CallSite>> = HashMap::new();
    for file in files {
        let calls = graph.occurrences[file]
            .iter()
            .filter(|o| o.role == OccurrenceRole::Reference(EdgeKind::Calls));
        for occurrence in calls {
            sites.entry(occurrence.symbol).or_default().push((
                file.clone(),
                occurrence.span,
                occurrence.context,
            ));
        }
    }
    sites
}

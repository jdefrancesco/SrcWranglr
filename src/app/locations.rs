//! The list of places a symbol appears, and which one the code panel is
//! showing ("location 3 of 12").

use crate::model::{CodeGraph, Location, OccurrenceRole, Span, SymbolId};

#[derive(Debug, Clone)]
pub struct LocationCursor {
    symbol: SymbolId,
    locations: Vec<Location>,
    index: usize,
}

impl LocationCursor {
    /// All locations of `symbol`, starting at its definition: the declaration
    /// in the symbol's own file at or after its recorded line, else its first
    /// declaration, else its first use.
    pub fn new(graph: &CodeGraph, symbol: SymbolId) -> Self {
        let locations = graph.locations_of(symbol);
        let is_declaration = |l: &Location| l.occurrence.role == OccurrenceRole::Declaration;
        let definition = graph.symbols.get(&symbol).and_then(|s| {
            locations.iter().position(|l| {
                is_declaration(l) && l.file == s.file && l.occurrence.span.line >= s.line
            })
        });
        let index = definition
            .or_else(|| locations.iter().position(is_declaration))
            .unwrap_or(0);
        Self {
            symbol,
            locations,
            index,
        }
    }

    pub fn symbol(&self) -> SymbolId {
        self.symbol
    }

    pub fn current(&self) -> Option<&Location> {
        self.locations.get(self.index)
    }

    /// 1-based position of the current location.
    pub fn position(&self) -> usize {
        self.index + 1
    }

    pub fn len(&self) -> usize {
        self.locations.len()
    }

    /// The cursor moved to the location at `span` in `file`, if there is one.
    pub fn focused_on(self, file: &str, span: Span) -> Self {
        let found = self
            .locations
            .iter()
            .position(|l| l.file == file && l.occurrence.span == span);
        match found {
            Some(index) => Self { index, ..self },
            None => self,
        }
    }

    /// The cursor moved `delta` locations, wrapping around at either end.
    pub fn stepped(self, delta: isize) -> Self {
        let Ok(len) = isize::try_from(self.locations.len()) else {
            return self;
        };
        if len == 0 {
            return self;
        }
        let current = isize::try_from(self.index).unwrap_or(0);
        let index = usize::try_from((current + delta).rem_euclid(len)).unwrap_or(0);
        Self { index, ..self }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::model::{EdgeKind, Occurrence, Symbol, SymbolKind};

    fn occurrence(line: u32, role: OccurrenceRole) -> Occurrence {
        Occurrence {
            span: Span {
                line,
                start: 0,
                end: 3,
            },
            symbol: 1,
            role,
            context: None,
        }
    }

    const USE: OccurrenceRole = OccurrenceRole::Reference(EdgeKind::Calls);
    const DECL: OccurrenceRole = OccurrenceRole::Declaration;

    /// Symbol 1 is defined in b.cc at line 5, declared in a.h, used elsewhere.
    fn graph() -> CodeGraph {
        let symbol = Symbol {
            id: 1,
            name: "run".into(),
            qualified_name: "run".into(),
            kind: SymbolKind::Function,
            file: "b.cc".into(),
            line: 5,
        };
        CodeGraph {
            symbols: HashMap::from([(1, symbol)]),
            occurrences: HashMap::from([
                ("a.h".into(), vec![occurrence(2, DECL)]),
                (
                    "b.cc".into(),
                    vec![occurrence(1, USE), occurrence(6, DECL), occurrence(9, USE)],
                ),
            ]),
            ..CodeGraph::default()
        }
    }

    fn current(cursor: &LocationCursor) -> (String, u32) {
        let location = cursor.current().expect("a location");
        (location.file.clone(), location.occurrence.span.line)
    }

    #[test]
    fn starts_at_the_definition() {
        let cursor = LocationCursor::new(&graph(), 1);
        assert_eq!(current(&cursor), ("b.cc".into(), 6));
        assert_eq!((cursor.position(), cursor.len()), (3, 4));
    }

    #[test]
    fn steps_through_locations_and_wraps() {
        let cursor = LocationCursor::new(&graph(), 1).stepped(1);
        assert_eq!(current(&cursor), ("b.cc".into(), 9));
        let wrapped = cursor.stepped(1);
        assert_eq!(current(&wrapped), ("a.h".into(), 2));
        assert_eq!(current(&wrapped.stepped(-1)), ("b.cc".into(), 9));
    }

    #[test]
    fn falls_back_to_first_declaration_then_first_use() {
        let mut g = graph();
        g.symbols.get_mut(&1).expect("symbol").file = "elsewhere.cc".into();
        assert_eq!(current(&LocationCursor::new(&g, 1)), ("a.h".into(), 2));

        g.occurrences.remove("a.h");
        g.occurrences
            .insert("b.cc".into(), vec![occurrence(1, USE), occurrence(9, USE)]);
        assert_eq!(current(&LocationCursor::new(&g, 1)), ("b.cc".into(), 1));
    }

    #[test]
    fn focuses_on_a_given_location_or_stays_put() {
        let cursor = LocationCursor::new(&graph(), 1);
        let span = |line| Span {
            line,
            start: 0,
            end: 3,
        };
        let moved = cursor.focused_on("b.cc", span(1));
        assert_eq!(current(&moved), ("b.cc".into(), 1));
        let unknown = moved.focused_on("a.h", span(40));
        assert_eq!(current(&unknown), ("b.cc".into(), 1));
    }

    #[test]
    fn symbol_without_locations_is_empty_and_stepping_is_a_no_op() {
        let cursor = LocationCursor::new(&CodeGraph::default(), 7).stepped(3);
        assert!(cursor.current().is_none());
        assert_eq!((cursor.len(), cursor.symbol()), (0, 7));
    }
}

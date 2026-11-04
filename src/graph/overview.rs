//! What the canvas shows when nothing is selected: one box per source file
//! listing its top-level symbols, arranged in a tidy grid. No wires — it is
//! a map to click into, not a relationship view.

use std::collections::{BTreeMap, HashSet};

use super::scene::{
    block_size, stack_column, Align, Block, Group, Measure, Pill, Scene, BLOCK_GAP,
};
use crate::model::{CodeGraph, EdgeKind, Symbol, SymbolId};

/// Top-level symbols listed per file box.
pub const MAX_FILE_MEMBERS: usize = 12;
/// File boxes shown at most.
pub const MAX_FILES: usize = 60;
const COLUMN_SPACING: f32 = 40.0;

pub fn build_overview(graph: &CodeGraph, measure: Measure) -> Scene {
    let contained: HashSet<SymbolId> = graph
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Contains)
        .map(|e| e.to)
        .collect();
    let mut by_file: BTreeMap<&str, Vec<&Symbol>> = BTreeMap::new();
    for symbol in graph
        .symbols
        .values()
        .filter(|s| !contained.contains(&s.id))
    {
        by_file
            .entry(symbol.file.as_str())
            .or_default()
            .push(symbol);
    }

    let hidden_neighbors = by_file.values().skip(MAX_FILES).map(Vec::len).sum();
    let blocks: Vec<Block> = by_file
        .into_iter()
        .take(MAX_FILES)
        .map(|(file, mut symbols)| {
            symbols.sort_by_key(|s| (s.line, s.id));
            Block::Group {
                id: None,
                title: file.to_string(),
                kind: None,
                hidden_members: symbols.len().saturating_sub(MAX_FILE_MEMBERS),
                members: symbols
                    .iter()
                    .take(MAX_FILE_MEMBERS)
                    .map(|s| (s.id, s.name.clone(), s.kind))
                    .collect(),
            }
        })
        .collect();

    let (groups, pills) = grid(blocks, measure);
    Scene {
        active: None,
        groups,
        pills,
        wires: Vec::new(),
        hidden_neighbors,
    }
}

/// Lays blocks out in roughly square grid columns, top-aligned.
fn grid(blocks: Vec<Block>, measure: Measure) -> (Vec<Group>, Vec<Pill>) {
    let columns = (blocks.len() as f32).sqrt().ceil().max(1.0) as usize;
    let per_column = blocks.len().div_ceil(columns).max(1);
    let mut groups = Vec::new();
    let mut pills = Vec::new();
    let mut x = 0.0;
    let mut remaining = blocks.into_iter().peekable();
    while remaining.peek().is_some() {
        let column: Vec<Block> = remaining.by_ref().take(per_column).collect();
        let sizes: Vec<_> = column.iter().map(|b| block_size(b, measure)).collect();
        let width = sizes.iter().map(|s| s.x).fold(0.0, f32::max);
        let height = sizes.iter().map(|s| s.y).sum::<f32>()
            + BLOCK_GAP * sizes.len().saturating_sub(1) as f32;
        let (g, p) = stack_column(column, x, Align::Left, height / 2.0, measure);
        groups.extend(g);
        pills.extend(p);
        x += width + COLUMN_SPACING;
    }
    (groups, pills)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::demo_codebase;
    use crate::graph::scene::tests::{assert_disjoint, fake_measure};
    use crate::model::SymbolKind;
    use egui::Rect;
    use std::collections::HashMap;

    #[test]
    fn one_box_per_file_with_top_level_symbols_only() {
        let graph = demo_codebase().expect("demo");
        let scene = build_overview(&graph, &fake_measure);
        let titles: Vec<&str> = scene.groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles.len(), graph.file_contents.len());

        let shown: Vec<&str> = scene.all_pills().map(|p| p.label.as_str()).collect();
        for name in ["Shape", "Circle", "Square", "total_area", "main"] {
            assert!(shown.contains(&name), "missing {name}");
        }
        // Members are reachable by clicking their class, not listed here.
        assert!(!shown.contains(&"radius"));
        assert!(scene.wires.is_empty());
        assert!(scene.groups.iter().all(|g| g.id.is_none()));
    }

    #[test]
    fn boxes_do_not_overlap() {
        let graph = demo_codebase().expect("demo");
        let scene = build_overview(&graph, &fake_measure);
        let rects: Vec<Rect> = scene.groups.iter().map(|g| g.rect).collect();
        assert_disjoint(&rects);
    }

    #[test]
    fn caps_files_and_members() {
        let symbols: HashMap<SymbolId, Symbol> = (0..2_000)
            .map(|id| {
                let symbol = Symbol {
                    id,
                    name: format!("f{id}"),
                    qualified_name: format!("f{id}"),
                    kind: SymbolKind::Function,
                    file: format!("file{}.c", id % 100),
                    line: 1,
                };
                (id, symbol)
            })
            .collect();
        let graph = CodeGraph {
            symbols,
            edges: Vec::new(),
            ..CodeGraph::default()
        };
        let scene = build_overview(&graph, &fake_measure);
        assert_eq!(scene.groups.len(), MAX_FILES);
        assert!(scene
            .groups
            .iter()
            .all(|g| g.pills.len() <= MAX_FILE_MEMBERS));
        assert!(scene
            .groups
            .iter()
            .all(|g| g.hidden_members == 20 - MAX_FILE_MEMBERS));
        assert_eq!(scene.hidden_neighbors, 40 * 20);
    }

    #[test]
    fn empty_graph_gives_empty_overview() {
        assert!(build_overview(&CodeGraph::default(), &fake_measure)
            .groups
            .is_empty());
    }
}

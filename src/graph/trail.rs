//! Sourcetrail-style layout around one active symbol:
//!
//! ```text
//!   [who uses it]  ──▶  [ active symbol, inside its class box ]  ──▶  [what it uses]
//!      left column             center column                      right column
//! ```
//!
//! Members are drawn inside their class's box instead of as separate nodes
//! joined by "contains" edges, which removes most of the clutter.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::scene::{stack_column, Align, Block, Measure, Scene, Wire};
use crate::model::{CodeGraph, Edge, EdgeKind, SymbolId, SymbolKind};

/// Horizontal gap between the three columns (room for the wires).
pub const COLUMN_GAP: f32 = 160.0;
/// Related symbols shown at most; the rest are counted as hidden.
pub const MAX_NEIGHBORS: usize = 40;
/// Members listed at most inside the active symbol's box.
pub const MAX_GROUP_MEMBERS: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

pub fn build_trail(graph: &CodeGraph, active: SymbolId, measure: Measure) -> Scene {
    if !graph.symbols.contains_key(&active) {
        return Scene::default();
    }
    let index = Hierarchy::new(graph);
    let (center, center_ids) = center_block(graph, &index, active);
    let relations = relations_of(graph, &center_ids, active);
    let (neighbors, hidden_neighbors) = ranked_neighbors(graph, &relations, &center_ids);
    let shown: HashSet<SymbolId> = center_ids.iter().chain(neighbors.keys()).copied().collect();

    let center_size = super::scene::block_size(&center, measure);
    let (mut groups, mut pills) = stack_column(vec![center], 0.0, Align::Center, 0.0, measure);
    let (left, right) = side_blocks(graph, &index, &neighbors, &center_ids);
    let half = center_size.x / 2.0;
    for (blocks, x, align) in [
        (left, -half - COLUMN_GAP, Align::Right),
        (right, half + COLUMN_GAP, Align::Left),
    ] {
        let (g, p) = stack_column(blocks, x, align, 0.0, measure);
        groups.extend(g);
        pills.extend(p);
    }

    let mut seen = HashSet::new();
    let wires = relations
        .iter()
        .filter(|e| shown.contains(&e.from) && shown.contains(&e.to))
        .map(|e| Wire {
            from: e.from,
            to: e.to,
            kind: e.kind,
        })
        .filter(|w| seen.insert(*w))
        .collect();
    Scene {
        active: Some(active),
        groups,
        pills,
        wires,
        hidden_neighbors,
    }
}

/// Parent/child lookups from `Contains` edges.
struct Hierarchy {
    parent: HashMap<SymbolId, SymbolId>,
    children: HashMap<SymbolId, Vec<SymbolId>>,
}

impl Hierarchy {
    fn new(graph: &CodeGraph) -> Self {
        let mut parent = HashMap::new();
        let mut children: HashMap<SymbolId, Vec<SymbolId>> = HashMap::new();
        for edge in graph.edges.iter().filter(|e| e.kind == EdgeKind::Contains) {
            parent.insert(edge.to, edge.from);
            children.entry(edge.from).or_default().push(edge.to);
        }
        for list in children.values_mut() {
            list.sort_by_key(|id| member_order(graph, *id));
        }
        Self { parent, children }
    }

    /// The class/struct a symbol is drawn inside, if any.
    fn owning_type(&self, graph: &CodeGraph, id: SymbolId) -> Option<SymbolId> {
        let parent = *self.parent.get(&id)?;
        is_type(graph, parent).then_some(parent)
    }
}

fn is_type(graph: &CodeGraph, id: SymbolId) -> bool {
    graph
        .symbols
        .get(&id)
        .is_some_and(|s| matches!(s.kind, SymbolKind::Class | SymbolKind::Struct))
}

fn is_container(graph: &CodeGraph, id: SymbolId) -> bool {
    is_type(graph, id)
        || graph
            .symbols
            .get(&id)
            .is_some_and(|s| s.kind == SymbolKind::Namespace)
}

/// Functions first, then fields, then nested types; by source line within.
fn member_order(graph: &CodeGraph, id: SymbolId) -> (u8, u32, SymbolId) {
    let Some(s) = graph.symbols.get(&id) else {
        return (u8::MAX, 0, id);
    };
    let rank = match s.kind {
        SymbolKind::Method | SymbolKind::Function => 0,
        SymbolKind::Field => 1,
        _ => 2,
    };
    (rank, s.line, id)
}

fn member_entry(graph: &CodeGraph, id: SymbolId) -> Option<(SymbolId, String, SymbolKind)> {
    graph.symbols.get(&id).map(|s| (id, s.name.clone(), s.kind))
}

/// The active symbol's block and every symbol id drawn inside it.
fn center_block(
    graph: &CodeGraph,
    index: &Hierarchy,
    active: SymbolId,
) -> (Block, HashSet<SymbolId>) {
    let symbol = &graph.symbols[&active];
    if is_container(graph, active) {
        let all = index.children.get(&active).cloned().unwrap_or_default();
        let hidden_members = all.len().saturating_sub(MAX_GROUP_MEMBERS);
        let members: Vec<_> = all
            .iter()
            .take(MAX_GROUP_MEMBERS)
            .filter_map(|id| member_entry(graph, *id))
            .collect();
        let ids = std::iter::once(active)
            .chain(members.iter().map(|m| m.0))
            .collect();
        let block = Block::Group {
            id: Some(active),
            title: symbol.name.clone(),
            kind: Some(symbol.kind),
            members,
            hidden_members,
        };
        return (block, ids);
    }
    let Some(owner) = index.owning_type(graph, active) else {
        let block = Block::Pill {
            id: active,
            label: symbol.qualified_name.clone(),
            kind: symbol.kind,
        };
        return (block, HashSet::from([active]));
    };
    // The active member plus siblings it is directly related to.
    let related: HashSet<SymbolId> = graph
        .edges
        .iter()
        .filter(|e| e.kind != EdgeKind::Contains && (e.from == active || e.to == active))
        .flat_map(|e| [e.from, e.to])
        .collect();
    let members: Vec<_> = index.children[&owner]
        .iter()
        .filter(|id| **id == active || related.contains(id))
        .filter_map(|id| member_entry(graph, *id))
        .collect();
    let ids = std::iter::once(owner)
        .chain(members.iter().map(|m| m.0))
        .collect();
    let owner_symbol = &graph.symbols[&owner];
    let block = Block::Group {
        id: Some(owner),
        title: owner_symbol.name.clone(),
        kind: Some(owner_symbol.kind),
        members,
        hidden_members: 0,
    };
    (block, ids)
}

/// Non-containment edges touching the focus: the active symbol, plus the
/// listed members when the active symbol is a class/namespace.
fn relations_of<'g>(
    graph: &'g CodeGraph,
    center_ids: &HashSet<SymbolId>,
    active: SymbolId,
) -> Vec<&'g Edge> {
    let focus: HashSet<SymbolId> = if is_container(graph, active) {
        center_ids.clone()
    } else {
        HashSet::from([active])
    };
    graph
        .edges
        .iter()
        .filter(|e| {
            e.kind != EdgeKind::Contains && (focus.contains(&e.from) || focus.contains(&e.to))
        })
        .collect()
}

/// Related symbols outside the center, mapped to their column, most
/// important first (inheritance, then calls, then uses), capped.
fn ranked_neighbors(
    graph: &CodeGraph,
    relations: &[&Edge],
    center_ids: &HashSet<SymbolId>,
) -> (HashMap<SymbolId, Side>, usize) {
    let mut candidates: HashMap<SymbolId, (u8, Side)> = HashMap::new();
    for edge in relations {
        let (other, side) = if center_ids.contains(&edge.from) {
            (edge.to, Side::Right)
        } else {
            (edge.from, Side::Left)
        };
        if center_ids.contains(&other) || !graph.symbols.contains_key(&other) {
            continue;
        }
        let rank = match edge.kind {
            EdgeKind::Inherits => 0,
            EdgeKind::Calls => 1,
            _ => 2,
        };
        let entry = candidates.entry(other).or_insert((rank, side));
        // Prefer the stronger relationship; anything outgoing goes right.
        *entry = (
            entry.0.min(rank),
            if side == Side::Right {
                Side::Right
            } else {
                entry.1
            },
        );
    }
    let mut ranked: Vec<(SymbolId, (u8, Side))> = candidates.into_iter().collect();
    ranked.sort_by(|(a, (ra, _)), (b, (rb, _))| {
        ra.cmp(rb).then_with(|| {
            graph.symbols[a]
                .qualified_name
                .cmp(&graph.symbols[b].qualified_name)
        })
    });
    let hidden = ranked.len().saturating_sub(MAX_NEIGHBORS);
    let shown = ranked
        .into_iter()
        .take(MAX_NEIGHBORS)
        .map(|(id, (_, side))| (id, side))
        .collect();
    (shown, hidden)
}

/// Turns neighbors into column blocks: members are boxed with their class
/// (on the side where the class first shows up), the rest are loose pills.
fn side_blocks(
    graph: &CodeGraph,
    index: &Hierarchy,
    neighbors: &HashMap<SymbolId, Side>,
    center_ids: &HashSet<SymbolId>,
) -> (Vec<Block>, Vec<Block>) {
    let mut ordered: Vec<(&SymbolId, &Side)> = neighbors.iter().collect();
    ordered.sort_by(|(a, _), (b, _)| {
        graph.symbols[a]
            .qualified_name
            .cmp(&graph.symbols[b].qualified_name)
    });

    // owner -> (side, members); BTreeMap keeps output deterministic.
    let mut boxes: BTreeMap<SymbolId, (Side, Vec<SymbolId>)> = BTreeMap::new();
    let mut loose: Vec<(SymbolId, Side)> = Vec::new();
    for (&id, &side) in &ordered {
        let owner = index
            .owning_type(graph, id)
            .filter(|o| !center_ids.contains(o));
        match owner {
            Some(owner) => boxes.entry(owner).or_insert((side, Vec::new())).1.push(id),
            None if is_type(graph, id) => {
                boxes.entry(id).or_insert((side, Vec::new()));
            }
            None => loose.push((id, side)),
        }
    }

    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut push = |side: Side, block: Block| match side {
        Side::Left => left.push(block),
        Side::Right => right.push(block),
    };
    for (owner, (side, mut members)) in boxes {
        members.sort_by_key(|id| member_order(graph, *id));
        let s = &graph.symbols[&owner];
        let members = members
            .iter()
            .filter_map(|id| member_entry(graph, *id))
            .collect();
        push(
            side,
            Block::Group {
                id: Some(owner),
                title: s.name.clone(),
                kind: Some(s.kind),
                members,
                hidden_members: 0,
            },
        );
    }
    for (id, side) in loose {
        let s = &graph.symbols[&id];
        push(
            side,
            Block::Pill {
                id,
                label: s.qualified_name.clone(),
                kind: s.kind,
            },
        );
    }
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::demo_codebase;
    use crate::graph::scene::tests::{assert_disjoint, fake_measure};
    use crate::model::Symbol;
    use egui::Rect;

    fn id(graph: &CodeGraph, qualified: &str) -> SymbolId {
        graph
            .symbols
            .values()
            .find(|s| s.qualified_name == qualified)
            .map(|s| s.id)
            .unwrap_or_else(|| panic!("no symbol {qualified}"))
    }

    fn trail(graph: &CodeGraph, active: &str) -> Scene {
        build_trail(graph, id(graph, active), &fake_measure)
    }

    fn center_x(scene: &Scene, symbol: SymbolId) -> f32 {
        scene.anchor(symbol).expect("anchored").center().x
    }

    fn group_titled<'a>(scene: &'a Scene, title: &str) -> &'a crate::graph::scene::Group {
        scene
            .groups
            .iter()
            .find(|g| g.title == title)
            .unwrap_or_else(|| {
                panic!(
                    "no group {title}: {:?}",
                    scene.groups.iter().map(|g| &g.title).collect::<Vec<_>>()
                )
            })
    }

    fn has_wire(scene: &Scene, from: SymbolId, to: SymbolId, kind: EdgeKind) -> bool {
        scene.wires.contains(&Wire { from, to, kind })
    }

    fn all_rects(scene: &Scene) -> Vec<Rect> {
        scene
            .pills
            .iter()
            .map(|p| p.rect)
            .chain(scene.groups.iter().map(|g| g.rect))
            .collect()
    }

    #[test]
    fn function_trail_puts_callers_left_and_callees_right() {
        let graph = demo_codebase().expect("demo");
        let scene = trail(&graph, "total_area");
        let active = id(&graph, "total_area");
        assert_eq!(scene.active, Some(active));

        let x = center_x(&scene, active);
        assert!(center_x(&scene, id(&graph, "main")) < x);
        assert!(center_x(&scene, id(&graph, "Circle::area")) > x);
        assert!(center_x(&scene, id(&graph, "Square::area")) > x);

        assert!(has_wire(
            &scene,
            active,
            id(&graph, "Circle::area"),
            EdgeKind::Calls
        ));
        assert!(has_wire(
            &scene,
            id(&graph, "main"),
            active,
            EdgeKind::Calls
        ));
    }

    #[test]
    fn callee_members_are_grouped_inside_their_class_box() {
        let graph = demo_codebase().expect("demo");
        let scene = trail(&graph, "total_area");
        let circle = group_titled(&scene, "Circle");
        assert_eq!(circle.id, Some(id(&graph, "Circle")));
        assert!(circle
            .pills
            .iter()
            .any(|p| p.id == id(&graph, "Circle::area")));
    }

    #[test]
    fn member_trail_shows_its_class_with_related_siblings_in_center() {
        let graph = demo_codebase().expect("demo");
        let scene = trail(&graph, "Circle::area");
        let circle = group_titled(&scene, "Circle");
        let members: HashSet<SymbolId> = circle.pills.iter().map(|p| p.id).collect();
        assert!(members.contains(&id(&graph, "Circle::area")));
        assert!(members.contains(&id(&graph, "Circle::radius")));
        // Sibling wire stays inside the center box.
        assert!(has_wire(
            &scene,
            id(&graph, "Circle::area"),
            id(&graph, "Circle::radius"),
            EdgeKind::Uses
        ));
        assert!(center_x(&scene, id(&graph, "total_area")) < circle.rect.left());
    }

    #[test]
    fn class_trail_lists_members_and_shows_base_class() {
        let graph = demo_codebase().expect("demo");
        let scene = trail(&graph, "Circle");
        let circle = group_titled(&scene, "Circle");
        let members: HashSet<SymbolId> = circle.pills.iter().map(|p| p.id).collect();
        assert!(members.contains(&id(&graph, "Circle::area")));
        assert!(members.contains(&id(&graph, "Circle::radius")));
        assert!(scene.anchor(id(&graph, "Shape")).is_some());
        assert!(has_wire(
            &scene,
            id(&graph, "Circle"),
            id(&graph, "Shape"),
            EdgeKind::Inherits
        ));
    }

    #[test]
    fn contains_edges_are_never_wires() {
        let graph = demo_codebase().expect("demo");
        for symbol in graph.symbols.values() {
            let scene = build_trail(&graph, symbol.id, &fake_measure);
            assert!(scene.wires.iter().all(|w| w.kind != EdgeKind::Contains));
            assert_disjoint(&all_rects(&scene));
            // Every wire endpoint is drawn.
            for wire in &scene.wires {
                assert!(
                    scene.anchor(wire.from).is_some() && scene.anchor(wire.to).is_some(),
                    "{wire:?}"
                );
            }
        }
    }

    /// Hub function 0 called by 1..=n.
    fn hub(n: SymbolId) -> CodeGraph {
        let symbol = |id: SymbolId| Symbol {
            id,
            name: format!("f{id}"),
            qualified_name: format!("f{id}"),
            kind: SymbolKind::Function,
            file: "a.c".into(),
            line: 1,
        };
        CodeGraph {
            symbols: (0..=n).map(|id| (id, symbol(id))).collect(),
            edges: (1..=n)
                .map(|from| Edge {
                    from,
                    to: 0,
                    kind: EdgeKind::Calls,
                })
                .collect(),
            ..CodeGraph::default()
        }
    }

    #[test]
    fn caps_neighbors_and_counts_the_rest() {
        let graph = hub(100);
        let scene = build_trail(&graph, 0, &fake_measure);
        assert_eq!(scene.all_pills().count(), MAX_NEIGHBORS + 1);
        assert_eq!(scene.hidden_neighbors, 100 - MAX_NEIGHBORS);
        assert_disjoint(&all_rects(&scene));
    }

    #[test]
    fn unknown_active_symbol_gives_empty_scene() {
        let graph = demo_codebase().expect("demo");
        let scene = build_trail(&graph, 9_999, &fake_measure);
        assert!(scene.bounds().is_none());
    }
}

//! Call hierarchy: who calls whom, as a tree growing left to right from the
//! program's entry points (or from the selected function).
//!
//! ```text
//!  main ──▶ init ──▶ load_config
//!       │        └─▶ open_window
//!       └─▶ run  ──▶ update ──▶ physics
//!                ├─▶ draw
//!                └─▶ poll_events
//! ```
//!
//! Each function appears once, under the caller that reaches it first, so
//! the tree's wires never cross. Calls that aren't tree edges (a second
//! caller, a call back up the tree) are drawn only while you hover one of
//! their ends. Callees are listed top to bottom in the order they are
//! called in the source.

mod calls;

pub use calls::CallIndex;

use std::collections::HashMap;

use egui::{Pos2, Rect, Vec2};

use super::scene::{pill_width, Measure, Pill, Scene, Wire, PILL_HEIGHT};
use crate::model::{CodeGraph, EdgeKind, SymbolId, SymbolKind};
use calls::{build_tree, entry_points, roots_for, CallTree, Limits};

/// Horizontal space between call levels (room for the wires).
pub const COLUMN_GAP: f32 = 90.0;
/// Vertical space between neighbouring pills.
pub const ROW_GAP: f32 = 8.0;
/// Extra vertical space between the trees of different entry points.
pub const TREE_GAP: f32 = 28.0;
/// Marks a function that calls itself.
const RECURSION_MARK: &str = " ↺";
/// Marks a function at the depth limit whose callees are not shown.
const MORE_MARK: &str = " …";

/// The call tree below `focus`, or below the program's entry points.
/// `calls` must be built from `graph` (callers cache it per project).
pub fn build_hierarchy(
    graph: &CodeGraph,
    calls: &CallIndex,
    focus: Option<SymbolId>,
    measure: Measure,
) -> Scene {
    let (roots, limits) = match focus {
        Some(id) => (roots_for(graph, calls, id), Limits::FOCUSED),
        None => (entry_points(graph, calls), Limits::OVERVIEW),
    };
    let tree = build_tree(calls, &roots.ids, limits);
    let labels: HashMap<SymbolId, String> = tree
        .depth
        .keys()
        .map(|id| (*id, label(graph, calls, &tree, *id)))
        .collect();
    let pills = Placer::new(graph, &tree, &labels, measure).place();
    let (wires, secondary_wires) = split_wires(calls, &tree);
    Scene {
        active: focus,
        pills,
        wires,
        secondary_wires,
        hidden_neighbors: tree.hidden + roots.hidden,
        ..Scene::default()
    }
}

/// Calls between shown functions: tree edges, and all the others.
fn split_wires(calls: &CallIndex, tree: &CallTree) -> (Vec<Wire>, Vec<Wire>) {
    let mut wires = Vec::new();
    let mut secondary = Vec::new();
    // Sorted, so wires (and their drawing order) are the same every build.
    let mut callers: Vec<SymbolId> = tree.depth.keys().copied().collect();
    callers.sort_unstable();
    for caller in &callers {
        let tree_children = tree.children_of(*caller);
        for callee in calls.callees(*caller).iter().filter(|c| tree.contains(**c)) {
            let wire = Wire {
                from: *caller,
                to: *callee,
                kind: EdgeKind::Calls,
            };
            if tree_children.contains(callee) {
                wires.push(wire);
            } else {
                secondary.push(wire);
            }
        }
    }
    (wires, secondary)
}

/// Lays the tree out: one column per depth, each subtree in its own band
/// of rows, each caller vertically centered on its callees.
struct Placer<'a> {
    graph: &'a CodeGraph,
    tree: &'a CallTree,
    labels: &'a HashMap<SymbolId, String>,
    widths: HashMap<SymbolId, f32>,
    column_x: Vec<f32>,
    pills: Vec<Pill>,
}

impl<'a> Placer<'a> {
    fn new(
        graph: &'a CodeGraph,
        tree: &'a CallTree,
        labels: &'a HashMap<SymbolId, String>,
        measure: Measure,
    ) -> Self {
        let widths: HashMap<SymbolId, f32> = labels
            .iter()
            .map(|(id, l)| (*id, pill_width(l, measure)))
            .collect();
        let mut column_width = vec![0.0f32; tree.limits.max_depth + 1];
        for (id, depth) in &tree.depth {
            let width = widths.get(id).copied().unwrap_or(0.0);
            if let Some(column) = column_width.get_mut(*depth) {
                *column = column.max(width);
            }
        }
        let column_x = column_width
            .iter()
            .scan(0.0, |x, width| {
                let left = *x;
                *x += width + COLUMN_GAP;
                Some(left)
            })
            .collect();
        Self {
            graph,
            tree,
            labels,
            widths,
            column_x,
            pills: Vec::new(),
        }
    }

    fn place(mut self) -> Vec<Pill> {
        let mut top = 0.0;
        for root in &self.tree.roots {
            let (_, bottom) = self.place_subtree(*root, 0, top);
            top = bottom + TREE_GAP;
        }
        self.pills
    }

    /// Places `id` and its subtree starting at `top`. Returns the pill's
    /// center y and the bottom of the band the subtree used.
    fn place_subtree(&mut self, id: SymbolId, depth: usize, top: f32) -> (f32, f32) {
        let children = self.tree.children_of(id).to_vec();
        let (center_y, bottom) = if children.is_empty() {
            (top + PILL_HEIGHT / 2.0, top + PILL_HEIGHT)
        } else {
            let mut next_top = top;
            let mut child_centers = Vec::with_capacity(children.len());
            for child in children {
                let (center, bottom) = self.place_subtree(child, depth + 1, next_top);
                child_centers.push(center);
                next_top = bottom + ROW_GAP;
            }
            let first = child_centers.first().copied().unwrap_or(top);
            let last = child_centers.last().copied().unwrap_or(top);
            ((first + last) / 2.0, next_top - ROW_GAP)
        };
        let width = self.widths.get(&id).copied().unwrap_or(0.0);
        let x = self.column_x.get(depth).copied().unwrap_or(0.0);
        let min = Pos2::new(x, center_y - PILL_HEIGHT / 2.0);
        let kind = self
            .graph
            .symbols
            .get(&id)
            .map_or(SymbolKind::Function, |s| s.kind);
        self.pills.push(Pill {
            id,
            label: self.labels.get(&id).cloned().unwrap_or_default(),
            kind,
            rect: Rect::from_min_size(min, Vec2::new(width, PILL_HEIGHT)),
        });
        (center_y, bottom)
    }
}

/// `Class::method` for methods, the plain name otherwise, plus markers for
/// recursion and for callees that aren't shown.
fn label(graph: &CodeGraph, calls: &CallIndex, tree: &CallTree, id: SymbolId) -> String {
    let Some(symbol) = graph.symbols.get(&id) else {
        return String::new();
    };
    let mut segments = symbol.qualified_name.rsplit("::");
    let name = segments.next().unwrap_or(&symbol.name);
    let base = match (symbol.kind, segments.next()) {
        (SymbolKind::Method, Some(owner)) => format!("{owner}::{name}"),
        _ => name.to_string(),
    };
    let recursion = if calls.is_recursive(id) {
        RECURSION_MARK
    } else {
        ""
    };
    let at_depth_limit = tree.depth.get(&id) == Some(&tree.limits.max_depth);
    let unexplored = at_depth_limit && !calls.callees(id).is_empty();
    let more = match tree.hidden_children.get(&id) {
        Some(n) => format!("  +{n}"),
        None if unexplored => MORE_MARK.to_string(),
        None => String::new(),
    };
    format!("{base}{recursion}{more}")
}

#[cfg(test)]
mod tests {
    use super::calls::tests::graph_of;
    use super::*;
    use crate::graph::scene::tests::{assert_disjoint, fake_measure};

    fn id(graph: &CodeGraph, name: &str) -> SymbolId {
        graph
            .symbols
            .values()
            .find(|s| s.qualified_name == name)
            .map(|s| s.id)
            .unwrap_or_else(|| panic!("no symbol {name}"))
    }

    const PROGRAM: &str = "void zeta() {}\n\
        void alpha() { zeta(); }\n\
        int fact(int n) { return n ? fact(n - 1) : 1; }\n\
        struct App { void run() { zeta(); alpha(); fact(3); } };\n\
        int main() { App a; a.run(); return 0; }\n";

    fn pill<'a>(scene: &'a Scene, graph: &CodeGraph, name: &str) -> &'a Pill {
        let target = id(graph, name);
        scene
            .pills
            .iter()
            .find(|p| p.id == target)
            .unwrap_or_else(|| panic!("no pill {name}"))
    }

    fn hierarchy(graph: &CodeGraph) -> Scene {
        build_hierarchy(graph, &CallIndex::new(graph), None, &fake_measure)
    }

    fn wire(from: SymbolId, to: SymbolId) -> Wire {
        Wire {
            from,
            to,
            kind: EdgeKind::Calls,
        }
    }

    #[test]
    fn grows_left_to_right_from_main_one_column_per_level() {
        let graph = graph_of(PROGRAM);
        let scene = hierarchy(&graph);
        let left = |name| pill(&scene, &graph, name).rect.left();
        assert!(left("main") < left("App::run"));
        assert!(left("App::run") < left("zeta"));
        assert_eq!(left("zeta"), left("fact"));
    }

    #[test]
    fn callees_go_top_to_bottom_in_call_order_with_the_caller_centered() {
        let graph = graph_of(PROGRAM);
        let scene = hierarchy(&graph);
        let y = |name| pill(&scene, &graph, name).rect.center().y;
        assert!(y("zeta") < y("alpha") && y("alpha") < y("fact"));
        assert!((y("App::run") - (y("zeta") + y("fact")) / 2.0).abs() < 0.01);
    }

    #[test]
    fn labels_show_owner_and_recursion() {
        let graph = graph_of(PROGRAM);
        let scene = hierarchy(&graph);
        assert_eq!(pill(&scene, &graph, "App::run").label, "App::run");
        assert_eq!(
            pill(&scene, &graph, "fact").label,
            format!("fact{RECURSION_MARK}")
        );
    }

    #[test]
    fn non_tree_calls_are_secondary_wires() {
        let graph = graph_of(PROGRAM);
        let scene = hierarchy(&graph);
        let (alpha, zeta) = (id(&graph, "alpha"), id(&graph, "zeta"));
        let run = id(&graph, "App::run");
        assert!(scene.wires.contains(&wire(run, zeta)));
        assert!(scene.secondary_wires.contains(&wire(alpha, zeta)));
        assert!(!scene.wires.contains(&wire(alpha, zeta)));
    }

    #[test]
    fn calls_back_up_the_tree_are_kept_as_secondary_wires() {
        let graph =
            graph_of("void b();\nvoid a() { b(); }\nvoid b() { a(); }\nint main() { a(); }\n");
        let scene = hierarchy(&graph);
        let (a, b) = (id(&graph, "a"), id(&graph, "b"));
        assert!(scene.wires.contains(&wire(a, b)));
        assert!(scene.secondary_wires.contains(&wire(b, a)));
    }

    #[test]
    fn focus_roots_the_hierarchy_at_that_function() {
        let graph = graph_of(PROGRAM);
        let alpha = id(&graph, "alpha");
        let scene = build_hierarchy(&graph, &CallIndex::new(&graph), Some(alpha), &fake_measure);
        let mut shown: Vec<&str> = scene.pills.iter().map(|p| p.label.as_str()).collect();
        shown.sort_unstable();
        assert_eq!(shown, ["alpha", "zeta"]);
        assert_eq!(scene.active, Some(alpha));
    }

    #[test]
    fn hidden_callees_are_counted_on_the_caller() {
        let n = Limits::OVERVIEW.max_children + 3;
        let defs: String = (0..n).map(|i| format!("void f{i}() {{}}\n")).collect();
        let body: String = (0..n).map(|i| format!("f{i}();")).collect();
        let graph = graph_of(&format!("{defs}int main() {{ {body} }}\n"));
        let scene = hierarchy(&graph);
        assert_eq!(pill(&scene, &graph, "main").label, "main  +3");
        assert_eq!(scene.hidden_neighbors, 3);
    }

    #[test]
    fn pills_never_overlap_on_the_demo_and_wide_trees() {
        let graph = crate::demo::demo_codebase().expect("demo");
        let scene = hierarchy(&graph);
        assert!(!scene.pills.is_empty());
        assert_disjoint(&scene.pills.iter().map(|p| p.rect).collect::<Vec<_>>());

        let fan: String = (0..10)
            .map(|i| format!("void l{i}() {{}}\nvoid m{i}() {{ l{i}(); }}\n"))
            .collect();
        let body: String = (0..10).map(|i| format!("m{i}();")).collect();
        let graph = graph_of(&format!("{fan}int main() {{ {body} }}\n"));
        let scene = hierarchy(&graph);
        assert_disjoint(&scene.pills.iter().map(|p| p.rect).collect::<Vec<_>>());
    }

    #[test]
    fn no_functions_gives_empty_scene() {
        let scene = hierarchy(&CodeGraph::default());
        assert!(scene.pills.is_empty() && scene.wires.is_empty());
    }
}

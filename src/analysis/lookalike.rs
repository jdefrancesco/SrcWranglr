//! Look-alike code: functions and blocks (loops, `if`s, `switch`es) that
//! have the same shape once names, numbers, strings and formatting are set
//! aside — copy-pasted parsers, vendored copies, one bug pattern written
//! in several places. And, for one function, the others most like it.
//!
//! A unit's shape is its syntax as a stream of tokens, each name a `$`,
//! each number `0`, each string `""`. Units with equal streams are look-
//! alikes. How alike two functions are is the share of short runs of
//! tokens they have in common, estimated from a small sketch of each.
//!
//! Each function is tokenized once; its blocks' shapes are read off its
//! tokens with a rolling hash, so nested blocks cost no more than flat
//! ones.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use tree_sitter::{Node, Parser};

use crate::index::functions_holding_lines;
use crate::index::syntax::{declarator_name, find_function_declarator, line_of};
use crate::model::{CodeGraph, SymbolId, SymbolKind};

mod tokens;

use tokens::{sketch_of, FunctionTokens};

/// Functions shorter than this (in tokens) aren't compared: getters and
/// one-liners look alike without meaning anything.
const MIN_FUNCTION_TOKENS: usize = 30;
const MIN_BLOCK_TOKENS: usize = 40;
/// Units kept per project.
const MAX_UNITS: usize = 400_000;
/// Blocks kept per function, and how deeply nested one may be.
const MAX_BLOCKS_PER_FUNCTION: usize = 2_000;
const MAX_BLOCK_DEPTH: usize = 64;
/// A file larger than this isn't read for shapes.
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
/// Syntax nodes walked between checks of the stop flag.
const NODES_PER_STOP_CHECK: usize = 4096;
/// Groups listed, largest first.
const MAX_GROUPS: usize = 500;
/// Functions listed as like one.
const MAX_LIKE: usize = 50;
/// Least share of runs in common for a function to be listed as like one.
const MIN_LIKENESS: f32 = 0.4;
/// Sizes compared: a function this many times longer (or shorter) than
/// the one asked about isn't like it.
const MAX_SIZE_RATIO: f32 = 2.5;

/// Statements whose blocks are compared.
const BLOCK_KINDS: [&str; 6] = [
    "for_statement",
    "for_range_loop",
    "while_statement",
    "do_statement",
    "if_statement",
    "switch_statement",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    Function,
    Block,
}

/// A function or block, and its shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub kind: UnitKind,
    pub file: String,
    /// 1-based lines it spans.
    pub first_line: u32,
    pub last_line: u32,
    /// The function it is (a function) or is in (a block), if indexed.
    pub symbol: Option<SymbolId>,
    pub tokens: usize,
    /// Equal for units of the same shape.
    key: u64,
    /// The smallest hashes of its runs of tokens, sorted (functions only).
    sketch: Vec<u64>,
    /// The unit it is inside, by index into `Shapes::units`.
    parent: Option<usize>,
}

/// The shapes of a project's code.
#[derive(Debug, Clone, Default)]
pub struct Shapes {
    pub units: Vec<Unit>,
    /// More units than kept, or files too large to read.
    pub is_cut_short: bool,
}

/// Units of the same shape, by index into `Shapes::units`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub kind: UnitKind,
    pub tokens: usize,
    pub units: Vec<usize>,
}

/// The shapes of every function and block in `graph`'s files; `None` if
/// `stop` was set.
pub fn shapes_of(graph: &CodeGraph, stop: &AtomicBool) -> Option<Shapes> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .ok()?;
    let functions = functions_by_place(graph);
    let mut files: Vec<&String> = graph.file_contents.keys().collect();
    files.sort();
    let mut shapes = Shapes::default();
    for file in files {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        let source = &graph.file_contents[file];
        if source.len() > MAX_FILE_BYTES {
            shapes.is_cut_short = true;
            continue;
        }
        let Some(tree) = parser.parse(source, None) else {
            continue;
        };
        let walk = Walk {
            source,
            file,
            functions: &functions,
            stop,
        };
        let found = walk.units_in(tree.root_node())?;
        let room = MAX_UNITS.saturating_sub(shapes.units.len());
        shapes.is_cut_short |= found.len() > room;
        let base = shapes.units.len();
        // Parents come before their children, so a cut keeps them whole.
        shapes
            .units
            .extend(found.into_iter().take(room).map(|unit| Unit {
                parent: unit.parent.map(|p| p + base),
                ..unit
            }));
    }
    place_in_functions(graph, &mut shapes.units);
    Some(shapes)
}

/// Gives units whose function the parse couldn't name (macros in the way)
/// the indexed function that holds their first line.
fn place_in_functions(graph: &CodeGraph, units: &mut [Unit]) {
    let unplaced: Vec<usize> = (0..units.len())
        .filter(|&i| units[i].symbol.is_none())
        .collect();
    let places: Vec<(&str, u32)> = unplaced
        .iter()
        .map(|&i| (units[i].file.as_str(), units[i].first_line))
        .collect();
    let found = functions_holding_lines(graph, &places);
    for (index, symbol) in unplaced.into_iter().zip(found) {
        units[index].symbol = symbol;
    }
}

/// Indexed functions by where their names are: file and line.
fn functions_by_place(graph: &CodeGraph) -> HashMap<(&str, u32), SymbolId> {
    graph
        .symbols
        .values()
        .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
        .map(|s| ((s.file.as_str(), s.line), s.id))
        .collect()
}

/// What reading one file needs.
struct Walk<'a> {
    source: &'a str,
    file: &'a str,
    functions: &'a HashMap<(&'a str, u32), SymbolId>,
    stop: &'a AtomicBool,
}

/// The function being walked through.
struct InFunction {
    end: usize,
    symbol: Option<SymbolId>,
    tokens: FunctionTokens,
    blocks: usize,
}

/// A unit (or block) the walk is inside: where it ends, and its index
/// among the units found if it is one.
struct Open {
    end: usize,
    unit: Option<usize>,
}

impl Walk<'_> {
    /// The units under `root`, parents before children, walked without
    /// recursion; `None` if stopped.
    fn units_in(&self, root: Node) -> Option<Vec<Unit>> {
        let mut units = Vec::new();
        let mut function: Option<InFunction> = None;
        let mut open: Vec<Open> = Vec::new();
        let mut cursor = root.walk();
        for visited in 0usize.. {
            if visited % NODES_PER_STOP_CHECK == 0 && self.stop.load(Ordering::Relaxed) {
                return None;
            }
            let node = cursor.node();
            let start = node.start_byte();
            while open.last().is_some_and(|o| start >= o.end) {
                open.pop();
            }
            if function.as_ref().is_some_and(|f| start >= f.end) {
                function = None;
            }
            let kind = node.kind();
            if kind == "function_definition" && function.is_none() {
                let entered = self.function(node);
                let unit = entered.1.map(|unit| {
                    units.push(unit);
                    units.len() - 1
                });
                open.push(Open {
                    end: node.end_byte(),
                    unit,
                });
                function = Some(entered.0);
            } else if let Some(inside) = function.as_mut().filter(|_| BLOCK_KINDS.contains(&kind)) {
                let parent = open.iter().rev().find_map(|o| o.unit);
                let unit = match open.len() <= MAX_BLOCK_DEPTH {
                    true => self.block(node, inside, parent),
                    false => None,
                };
                let unit = unit.map(|unit| {
                    units.push(unit);
                    units.len() - 1
                });
                open.push(Open {
                    end: node.end_byte(),
                    unit,
                });
            }
            if cursor.goto_first_child() {
                continue;
            }
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    return Some(units);
                }
            }
        }
        Some(units)
    }

    /// Entering `node`, a function definition: its tokens, and its unit if
    /// it is long enough to compare.
    fn function(&self, node: Node) -> (InFunction, Option<Unit>) {
        let symbol = node
            .child_by_field_name("declarator")
            .and_then(find_function_declarator)
            .and_then(declarator_name)
            .and_then(|name| self.functions.get(&(self.file, line_of(name))).copied());
        let tokens = FunctionTokens::of(node, self.source);
        let count = tokens.len();
        let unit = (count >= MIN_FUNCTION_TOKENS).then(|| Unit {
            kind: UnitKind::Function,
            file: self.file.to_string(),
            first_line: line_of(node),
            last_line: last_line(node),
            symbol,
            tokens: count,
            key: tokens.key(0..count),
            sketch: sketch_of(tokens.hashes()),
            parent: None,
        });
        let entered = InFunction {
            end: node.end_byte(),
            symbol,
            tokens,
            blocks: 0,
        };
        (entered, unit)
    }

    /// The unit for `node`, a block in `inside`, if it is long enough to
    /// compare and the function hasn't too many.
    fn block(&self, node: Node, inside: &mut InFunction, parent: Option<usize>) -> Option<Unit> {
        if inside.blocks == MAX_BLOCKS_PER_FUNCTION {
            return None;
        }
        let range = inside.tokens.range_of(node.start_byte(), node.end_byte());
        if range.len() < MIN_BLOCK_TOKENS {
            return None;
        }
        inside.blocks += 1;
        Some(Unit {
            kind: UnitKind::Block,
            file: self.file.to_string(),
            first_line: line_of(node),
            last_line: last_line(node),
            symbol: inside.symbol,
            tokens: range.len(),
            key: inside.tokens.key(range),
            sketch: Vec::new(),
            parent,
        })
    }
}

fn last_line(node: Node) -> u32 {
    u32::try_from(node.end_position().row + 1).unwrap_or(u32::MAX)
}

/// Units of the same shape, the biggest first: whole functions, and blocks
/// not already inside look-alike functions (or bigger look-alike blocks).
/// Whether more were found than are listed.
pub fn groups(shapes: &Shapes) -> (Vec<Group>, bool) {
    let mut by_key: HashMap<(u64, bool), Vec<usize>> = HashMap::new();
    for (index, unit) in shapes.units.iter().enumerate() {
        let is_function = unit.kind == UnitKind::Function;
        by_key
            .entry((unit.key, is_function))
            .or_default()
            .push(index);
    }
    let mut candidates: Vec<Group> = by_key
        .into_values()
        .filter(|units| units.len() > 1)
        .map(|units| {
            let first = &shapes.units[units[0]];
            Group {
                kind: first.kind,
                tokens: first.tokens,
                units,
            }
        })
        .collect();
    // Biggest first; functions before blocks of the same size.
    candidates.sort_by_key(|g| {
        (
            std::cmp::Reverse(g.tokens),
            g.kind == UnitKind::Block,
            g.units[0],
        )
    });
    let mut is_listed = vec![false; shapes.units.len()];
    let mut kept = Vec::new();
    let mut is_cut_short = false;
    for group in candidates {
        // Inside a unit already listed: its parents are at most
        // `MAX_BLOCK_DEPTH` deep, so this stays quick.
        let is_inside = |index: &usize| {
            std::iter::successors(shapes.units[*index].parent, |&p| shapes.units[p].parent)
                .any(|p| is_listed[p])
        };
        if group.units.iter().all(is_inside) {
            continue;
        }
        if kept.len() == MAX_GROUPS {
            is_cut_short = true;
            break;
        }
        for &index in &group.units {
            is_listed[index] = true;
        }
        kept.push(group);
    }
    (kept, is_cut_short)
}

/// The functions most like `symbol`'s, likest first, with how alike
/// (0 to 1). Empty if it isn't a function compared.
pub fn like(shapes: &Shapes, symbol: SymbolId) -> Vec<(usize, f32)> {
    let Some(target) = shapes
        .units
        .iter()
        .position(|u| u.kind == UnitKind::Function && u.symbol == Some(symbol))
    else {
        return Vec::new();
    };
    let wanted = &shapes.units[target];
    let size = wanted.tokens as f32;
    let mut found: Vec<(usize, f32)> = shapes
        .units
        .iter()
        .enumerate()
        .filter(|(i, u)| *i != target && u.kind == UnitKind::Function)
        .filter(|(_, u)| {
            let ratio = u.tokens as f32 / size;
            (1.0 / MAX_SIZE_RATIO..=MAX_SIZE_RATIO).contains(&ratio)
        })
        .filter_map(|(i, u)| {
            let score = match u.key == wanted.key {
                true => 1.0,
                false => tokens::likeness(&wanted.sketch, &u.sketch),
            };
            (score >= MIN_LIKENESS).then_some((i, score))
        })
        .collect();
    found.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    found.truncate(MAX_LIKE);
    found
}

/// The functions with a unit in `group`, each once.
#[cfg(test)]
pub fn symbols_in(shapes: &Shapes, group: &Group) -> std::collections::HashSet<SymbolId> {
    group
        .units
        .iter()
        .filter_map(|&i| shapes.units[i].symbol)
        .collect()
}

#[cfg(test)]
#[path = "lookalike_tests.rs"]
mod tests;

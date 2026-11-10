//! What several places of code share: their code walked side by side.
//! Where every place has the same token, it stays; where they differ, the
//! smallest expression holding the difference becomes a metavariable (one
//! per distinct set of differing texts, so a value used twice stays one
//! metavariable), a string `"..."`, and arguments only some places pass
//! `...`. Statements are lined up by their kind and the call they make;
//! those not in every place are left out, with `...` where they were.

use std::collections::HashMap;

use tree_sitter::{Node, Tree};

use super::pick::{picked, statement_of, Picked, MAX_STATEMENTS};
use super::{
    code_on, cut_short, first_callee, has_gap, is_pattern_syntax, is_string, is_token, is_variable,
    parse, row_of, rule_id, Draft, Example, Metavariables, MAX_LONG_CODE_BYTES,
    MAX_LONG_PATTERN_BYTES,
};

/// Places to draft one rule from, at most.
pub const MAX_EXAMPLES: usize = 50;
/// How deep the places are walked side by side; deeper code that differs
/// becomes one metavariable.
const MAX_DEPTH: usize = 200;
/// Token kinds that may differ between places and become a metavariable:
/// a called name, a field, a type, a number.
const GENERALIZABLE: [&str; 7] = [
    "identifier",
    "field_identifier",
    "type_identifier",
    "namespace_identifier",
    "primitive_type",
    "number_literal",
    "char_literal",
];

/// A rule finding what `examples` (several places) share.
pub(super) fn shared(examples: &[(Example, &str)], allow_gaps: bool) -> Result<Draft, String> {
    if examples.len() > MAX_EXAMPLES {
        return Err(format!(
            "That's {} places; draft from at most {MAX_EXAMPLES}.",
            examples.len()
        ));
    }
    // Each file once, however many places are in it.
    let mut trees: HashMap<&str, Tree> = HashMap::new();
    for (example, source) in examples {
        if !trees.contains_key(example.file.as_str()) {
            trees.insert(&example.file, parse(source, &example.file)?);
        }
    }
    let picks = examples
        .iter()
        .map(|(example, _)| code_of(trees[example.file.as_str()].root_node(), example))
        .collect::<Result<Vec<Vec<Node>>, String>>()?;
    let sources: Vec<&str> = examples.iter().map(|(_, source)| *source).collect();
    let (first, first_source) = &examples[0];
    let callee = picks[0]
        .iter()
        .find_map(|node| first_callee(*node, first_source));
    let mut unifier = Unifier {
        sources: &sources,
        names: Metavariables::default(),
        is_anchored: false,
    };
    let pattern = match picks.iter().all(|nodes| nodes.len() == 1) {
        true => {
            let nodes: Vec<Node> = picks.iter().map(|nodes| nodes[0]).collect();
            unifier.render(&nodes, 0).map_err(|()| {
                "These places aren't the same kind of code (a call and a declaration, say), \
                 so no one rule fits them."
                    .to_string()
            })?
        }
        false => {
            // A line picked among ranges of lines stands for its statement.
            let lists: Vec<Vec<Node>> = picks
                .into_iter()
                .map(|nodes| match nodes.as_slice() {
                    [one] => vec![statement_of(*one)],
                    _ => nodes,
                })
                .collect();
            unifier
                .sequence(&lists, 0, allow_gaps, false)
                .map_err(|()| "These places have no statement in common.".to_string())?
        }
    };
    if !unifier.is_anchored {
        return Err(
            "All these places share is variables and punctuation, so a rule from them \
                    would match almost anything. Pick places that share a call, a field or a \
                    literal."
                .into(),
        );
    }
    if pattern.len() > MAX_LONG_PATTERN_BYTES {
        return Err("What these places share is too long for a rule: pick less code.".into());
    }
    let row = first.first_line.saturating_sub(1) as usize;
    let quoted_code = cut_short(first_source.lines().nth(row).unwrap_or("").trim());
    Ok(Draft {
        id: rule_id(callee.as_deref(), &first.file, examples.len() as u32) + "-places",
        message: format!(
            "Like {} places, such as {}: {quoted_code}",
            examples.len(),
            first.place()
        ),
        same_shape: pattern,
        any_values: None,
        names: unifier.names.names(),
        callee,
    })
}

/// The code `example` picks in the tree at `root`: one node, or the
/// statements on its lines.
fn code_of<'t>(root: Node<'t>, example: &Example) -> Result<Vec<Node<'t>>, String> {
    let (first, last) = (row_of(example.first_line)?, row_of(example.last_line)?);
    let nothing = || format!("No call or statement starts on {}.", example.place());
    let nodes = match first == last {
        true => vec![code_on(root, first).ok_or_else(nothing)?],
        false => match picked(root, first, last) {
            Some(Picked::One(node)) => vec![node],
            Some(Picked::Sequence(nodes)) => nodes,
            None => return Err(nothing()),
        },
    };
    let bytes: usize = nodes.iter().map(|n| n.byte_range().len()).sum();
    if bytes > MAX_LONG_CODE_BYTES {
        return Err(format!(
            "The code at {} is too long for a rule.",
            example.place()
        ));
    }
    Ok(nodes)
}

struct Unifier<'s> {
    /// Each place's file text, in the places' order.
    sources: &'s [&'s str],
    names: Metavariables,
    /// Whether anything but variables and punctuation was kept.
    is_anchored: bool,
}

impl<'s> Unifier<'s> {
    fn text(&self, place: usize, node: Node) -> &'s str {
        self.sources[place].get(node.byte_range()).unwrap_or("")
    }

    /// The pattern for `nodes` (one per place) if they have the same
    /// structure, down to what can differ.
    fn render(&mut self, nodes: &[Node], depth: usize) -> Result<String, ()> {
        if depth > MAX_DEPTH {
            return Err(());
        }
        let first = nodes[0];
        if nodes.iter().all(|n| is_token(*n)) {
            return self.leaf(nodes);
        }
        if nodes
            .iter()
            .any(|n| is_token(*n) || n.kind() != first.kind())
        {
            return Err(());
        }
        let parts: Vec<Vec<Node>> = nodes.iter().map(|n| parts_of(*n)).collect();
        if parts.iter().any(|p| p.len() != parts[0].len()) {
            return match first.kind() {
                "argument_list" => self.arguments(nodes, depth),
                "compound_statement" => self.block(nodes, depth),
                _ => Err(()),
            };
        }
        if first.kind() == "compound_statement" {
            // Blocks as long as each other may still differ statement by
            // statement: line them up as blocks of different lengths are.
            let mark = (self.names.mark(), self.is_anchored);
            return self.side_by_side(&parts, depth).or_else(|()| {
                self.names.roll_back(mark.0);
                self.is_anchored = mark.1;
                self.block(nodes, depth)
            });
        }
        self.side_by_side(&parts, depth)
    }

    /// `parts` (each node's, one list per place) rendered column by
    /// column, spaced as in the first place.
    fn side_by_side(&mut self, parts: &[Vec<Node>], depth: usize) -> Result<String, ()> {
        let mut out = String::new();
        let mut previous_end = None;
        for i in 0..parts[0].len() {
            let column: Vec<Node> = parts.iter().map(|p| p[i]).collect();
            let piece = self.child(&column, depth + 1)?;
            if previous_end.is_some_and(|end| has_gap(self.sources[0], end, column[0].start_byte()))
            {
                out.push(' ');
            }
            out.push_str(&piece);
            previous_end = Some(column[0].end_byte());
        }
        Ok(out)
    }

    /// `column` rendered, or, if the places differ there, a metavariable
    /// when it is an expression in each.
    fn child(&mut self, column: &[Node], depth: usize) -> Result<String, ()> {
        let (mark, was_anchored) = (self.names.mark(), self.is_anchored);
        self.render(column, depth).or_else(|()| {
            self.names.roll_back(mark);
            self.is_anchored = was_anchored;
            match column.iter().all(|n| is_expression(n.kind())) {
                true => Ok(self.metavariable(column)),
                false => Err(()),
            }
        })
    }

    fn metavariable(&mut self, nodes: &[Node]) -> String {
        let texts: Vec<&str> = nodes
            .iter()
            .enumerate()
            .map(|(place, n)| self.text(place, *n))
            .collect();
        self.names.of(&texts)
    }

    /// One token from each place.
    fn leaf(&mut self, nodes: &[Node]) -> Result<String, ()> {
        let texts: Vec<&str> = nodes
            .iter()
            .enumerate()
            .map(|(place, n)| self.text(place, *n))
            .collect();
        let first = nodes[0];
        if nodes.iter().zip(&texts).all(|(n, t)| is_variable(*n, t)) {
            return Ok(self.names.of(&texts));
        }
        if texts.iter().all(|t| *t == texts[0]) {
            if is_pattern_syntax(first, texts[0]) {
                self.is_anchored = true;
                return Ok("\"...\"".into());
            }
            self.is_anchored |= first.is_named();
            return Ok(texts[0].to_string());
        }
        if nodes.iter().all(|n| is_string(n.kind())) {
            self.is_anchored = true;
            return Ok("\"...\"".into());
        }
        let is_same_kind = nodes.iter().all(|n| n.kind() == first.kind());
        match is_same_kind && GENERALIZABLE.contains(&first.kind()) {
            true => Ok(self.names.of(&texts)),
            false => Err(()),
        }
    }

    /// Argument lists of different lengths: the arguments all places
    /// pass, then `...`.
    fn arguments(&mut self, nodes: &[Node], depth: usize) -> Result<String, ()> {
        let lists: Vec<Vec<Node>> = nodes.iter().map(|n| named_parts(*n)).collect();
        let shared = lists.iter().map(Vec::len).min().unwrap_or(0);
        let mut pieces = Vec::with_capacity(shared + 1);
        for i in 0..shared {
            let column: Vec<Node> = lists.iter().map(|list| list[i]).collect();
            pieces.push(self.child(&column, depth + 1)?);
        }
        pieces.push("...".to_string());
        Ok(format!("({})", pieces.join(", ")))
    }

    /// Blocks of different lengths: the statements they share.
    fn block(&mut self, nodes: &[Node], depth: usize) -> Result<String, ()> {
        let lists: Vec<Vec<Node>> = nodes.iter().map(|n| named_parts(*n)).collect();
        let body = self.sequence(&lists, depth + 1, false, true)?;
        Ok(format!("{{\n{body}\n}}"))
    }

    /// The statements in every one of `lists` (one per place), one per
    /// line, with `...` between them where `allow_gaps` says or any place
    /// has others between them; and at the ends too with `keep_edges`.
    fn sequence(
        &mut self,
        lists: &[Vec<Node>],
        depth: usize,
        allow_gaps: bool,
        keep_edges: bool,
    ) -> Result<String, ()> {
        let columns = align(lists, self.sources).ok_or(())?;
        let mut parts: Vec<String> = Vec::new();
        let starts_late = columns.first().is_some_and(|c| c.iter().any(|&i| i > 0));
        if keep_edges && starts_late {
            push_ellipsis(&mut parts);
        }
        let mut previous: Option<&Vec<usize>> = None;
        for column in &columns {
            if let Some(previous) = previous {
                let skips = column.iter().zip(previous).any(|(c, p)| *c > p + 1);
                if allow_gaps || skips {
                    push_ellipsis(&mut parts);
                }
            }
            let nodes: Vec<Node> = column.iter().zip(lists).map(|(&i, l)| l[i]).collect();
            match self.child(&nodes, depth) {
                Ok(piece) => parts.push(piece),
                Err(()) => push_ellipsis(&mut parts),
            }
            previous = Some(column);
        }
        let ends_early = columns
            .last()
            .is_some_and(|c| c.iter().zip(lists).any(|(&i, l)| i + 1 < l.len()));
        if keep_edges && ends_early {
            push_ellipsis(&mut parts);
        }
        if !keep_edges {
            while parts.last().is_some_and(|p| p == "...") {
                parts.pop();
            }
            let leading = parts.iter().take_while(|p| *p == "...").count();
            parts.drain(..leading);
        }
        match parts.iter().all(|p| p == "...") {
            true => Err(()),
            false => Ok(parts.join("\n")),
        }
    }
}

fn push_ellipsis(parts: &mut Vec<String>) {
    if parts.last().map(String::as_str) != Some("...") {
        parts.push("...".into());
    }
}

/// The statements every list shares, as indexes into each list: lined up
/// by kind and the call each makes, one list after another.
fn align(lists: &[Vec<Node>], sources: &[&str]) -> Option<Vec<Vec<usize>>> {
    if lists.is_empty() || lists.iter().any(|list| list.len() > MAX_STATEMENTS) {
        return None;
    }
    let signature = |place: usize, node: Node| (node.kind_id(), first_callee(node, sources[place]));
    let mut columns: Vec<Vec<usize>> = (0..lists[0].len()).map(|i| vec![i]).collect();
    for (place, list) in lists.iter().enumerate().skip(1) {
        let kept: Vec<_> = columns
            .iter()
            .map(|c| signature(0, lists[0][c[0]]))
            .collect();
        let other: Vec<_> = list.iter().map(|n| signature(place, *n)).collect();
        columns = common(&kept, &other)
            .into_iter()
            .map(|(i, j)| columns[i].iter().copied().chain([j]).collect())
            .collect();
    }
    Some(columns)
}

/// A longest common subsequence of `a` and `b`, as pairs of indexes.
fn common<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut lengths = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lengths[i][j] = match a[i] == b[j] {
                true => lengths[i + 1][j + 1] + 1,
                false => lengths[i + 1][j].max(lengths[i][j + 1]),
            };
        }
    }
    let (mut i, mut j, mut pairs) = (0, 0, Vec::new());
    while i < n && j < m {
        if a[i] == b[j] {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if lengths[i + 1][j] >= lengths[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// Every child of `node` but comments.
fn parts_of(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    let parts = node
        .children(&mut cursor)
        .filter(|c| c.kind() != "comment")
        .collect();
    parts
}

fn named_parts(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    let parts = node
        .named_children(&mut cursor)
        .filter(|c| c.kind() != "comment")
        .collect();
    parts
}

/// Whether a node of `kind` is an expression, which a metavariable can
/// stand for.
fn is_expression(kind: &str) -> bool {
    kind.ends_with("_expression")
        || kind.ends_with("_literal")
        || matches!(
            kind,
            "identifier"
                | "field_identifier"
                | "qualified_identifier"
                | "template_function"
                | "concatenated_string"
                | "true"
                | "false"
                | "null"
                | "nullptr"
                | "this"
                | "initializer_list"
        )
}

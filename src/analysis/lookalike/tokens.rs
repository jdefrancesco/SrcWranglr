//! A function's tokens and the hashes that compare shapes: a rolling hash
//! over the tokens (any run of them hashed in constant time, so a block's
//! shape is read off its function's tokens), and small sketches of the
//! runs, to tell how alike two functions are.

use std::hash::{Hash, Hasher};
use std::ops::Range;

use tree_sitter::Node;

/// Tokens in a run compared between functions.
const RUN: usize = 5;
/// Run hashes kept per function: the smallest ones. Two functions are
/// compared exactly when they have no more runs than this, and estimated
/// from these otherwise.
const SKETCH: usize = 64;
/// The rolling hash's base (odd, so powers of it never vanish).
const BASE: u64 = 0x0000_0100_0000_01b3;
/// Mixes a run's length into its key.
const LENGTH_MIX: u64 = 0x9e37_79b9_7f4a_7c15;

/// Kinds read as one token, their insides set aside.
const NAME_KINDS: [&str; 6] = [
    "identifier",
    "field_identifier",
    "type_identifier",
    "namespace_identifier",
    "statement_identifier",
    "destructor_name",
];
const STRING_KINDS: [&str; 5] = [
    "string_literal",
    "raw_string_literal",
    "concatenated_string",
    "char_literal",
    "user_defined_literal",
];

/// One function's tokens, where each starts, and its rolling hashes.
pub struct FunctionTokens {
    hashes: Vec<u64>,
    starts: Vec<usize>,
    /// `prefix[i]`: the rolling hash of the first `i` tokens.
    prefix: Vec<u64>,
    /// `powers[i]`: `BASE` to the `i`.
    powers: Vec<u64>,
}

impl FunctionTokens {
    /// The tokens of `node` (names and literals made alike, comments left
    /// out), walked without recursion.
    pub fn of(node: Node, source: &str) -> Self {
        let mut hashes = Vec::new();
        let mut starts = Vec::new();
        let mut cursor = node.walk();
        'walk: loop {
            let current = cursor.node();
            let kind = current.kind();
            let atom = match kind {
                "comment" => Some(None),
                _ if NAME_KINDS.contains(&kind) => Some(Some(hash_str("$"))),
                "number_literal" => Some(Some(hash_str("0"))),
                _ if STRING_KINDS.contains(&kind) => Some(Some(hash_str("\"\""))),
                "preproc_arg" => Some(Some(hash_str("#"))),
                // Keywords and operators as written (`int`, `+=`).
                _ if current.child_count() == 0 => Some(Some(match current.is_named() {
                    true => hash_str(source.get(current.byte_range()).unwrap_or_default()),
                    false => hash_str(kind),
                })),
                _ => None,
            };
            match atom {
                Some(Some(hash)) => {
                    hashes.push(hash);
                    starts.push(current.start_byte());
                }
                // A comment: nothing, and nothing inside it.
                Some(None) => {}
                None if cursor.goto_first_child() => continue,
                None => {}
            }
            loop {
                if cursor.node() == node {
                    break 'walk;
                }
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    break 'walk;
                }
            }
        }
        let mut prefix = Vec::with_capacity(hashes.len() + 1);
        let mut powers = Vec::with_capacity(hashes.len() + 1);
        prefix.push(0u64);
        powers.push(1u64);
        for hash in &hashes {
            let last = prefix[prefix.len() - 1];
            prefix.push(last.wrapping_mul(BASE).wrapping_add(*hash));
            let power = powers[powers.len() - 1];
            powers.push(power.wrapping_mul(BASE));
        }
        Self {
            hashes,
            starts,
            prefix,
            powers,
        }
    }

    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    pub fn hashes(&self) -> &[u64] {
        &self.hashes
    }

    /// The tokens between bytes `start` and `end` of the file.
    pub fn range_of(&self, start: usize, end: usize) -> Range<usize> {
        let first = self.starts.partition_point(|&s| s < start);
        let past = self.starts.partition_point(|&s| s < end);
        first..past.max(first)
    }

    /// The key of the tokens in `range`: equal for equal runs of tokens,
    /// wherever they are.
    pub fn key(&self, range: Range<usize>) -> u64 {
        let length = range.len();
        let hash = self.prefix[range.end]
            .wrapping_sub(self.prefix[range.start].wrapping_mul(self.powers[length]));
        hash ^ (length as u64).wrapping_mul(LENGTH_MIX)
    }
}

fn hash_str(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn hash_of(tokens: &[u64]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tokens.hash(&mut hasher);
    hasher.finish()
}

/// The smallest `SKETCH` hashes of the `RUN`-token runs in `tokens`,
/// sorted.
pub fn sketch_of(tokens: &[u64]) -> Vec<u64> {
    let mut runs: Vec<u64> = tokens.windows(RUN).map(hash_of).collect();
    runs.sort_unstable();
    runs.dedup();
    runs.truncate(SKETCH);
    runs
}

/// The share of runs two functions have in common, from their sketches:
/// exact when neither has more runs than a sketch holds, else estimated
/// from the smallest runs of the two together (bottom-k).
pub fn likeness(a: &[u64], b: &[u64]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    // Past the last run a full sketch holds, what the other has is unknown.
    let cutoff = [a, b]
        .iter()
        .filter(|s| s.len() == SKETCH)
        .filter_map(|s| s.last().copied())
        .min()
        .unwrap_or(u64::MAX);
    let (mut i, mut j, mut seen, mut both) = (0, 0, 0usize, 0usize);
    while seen < SKETCH {
        let (x, y) = (
            a.get(i).filter(|&&x| x <= cutoff),
            b.get(j).filter(|&&y| y <= cutoff),
        );
        match (x, y) {
            (Some(x), Some(y)) if x == y => {
                both += 1;
                i += 1;
                j += 1;
            }
            (Some(x), Some(y)) if x < y => i += 1,
            (Some(_), Some(_)) | (None, Some(_)) => j += 1,
            (Some(_), None) => i += 1,
            (None, None) => break,
        }
        seen += 1;
    }
    match seen {
        0 => 0.0,
        _ => both as f32 / seen as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens_of(source: &str) -> FunctionTokens {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .expect("grammar");
        let tree = parser.parse(source, None).expect("parse");
        let function = tree.root_node().child(0).expect("a function");
        FunctionTokens::of(function, source)
    }

    #[test]
    fn equal_runs_of_tokens_have_equal_keys_wherever_they_are() {
        let source = "int f(int a) { if (a) { a = a + 1; } x(); if (b) { b = b + 2; } }";
        let tokens = tokens_of(source);
        let first = tokens.range_of(source.find("if (a)").unwrap(), source.find(" x()").unwrap());
        let second_start = source.rfind("if (b)").unwrap();
        let second = tokens.range_of(second_start, source.len() - 2);
        assert_eq!(first.len(), second.len());
        assert_eq!(tokens.key(first.clone()), tokens.key(second));
        assert_ne!(
            tokens.key(first.clone()),
            tokens.key(first.start..first.end - 1)
        );
    }

    #[test]
    fn likeness_is_the_share_in_common() {
        assert_eq!(likeness(&[1, 2, 3, 4], &[3, 4, 5, 6]), 2.0 / 6.0);
        assert_eq!(likeness(&[], &[1]), 0.0);
        assert_eq!(likeness(&[7], &[7]), 1.0);
    }

    #[test]
    fn a_full_sketch_is_compared_only_as_far_as_it_reaches() {
        let full: Vec<u64> = (0..SKETCH as u64).map(|v| v * 2).collect();
        // The other has the same runs and more past the cutoff.
        let other: Vec<u64> = (0..SKETCH as u64).map(|v| v * 2).collect();
        assert_eq!(likeness(&full, &other), 1.0);
        let shifted: Vec<u64> = (0..SKETCH as u64).map(|v| v * 2 + 1).collect();
        assert_eq!(likeness(&full, &shifted), 0.0);
    }
}

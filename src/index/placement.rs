//! Which function a line of a file is in: for things found while indexing
//! (strings, comments) that are listed with the function holding them.

use std::collections::HashMap;
use std::ops::Range;

use crate::model::{FunctionMetrics, SymbolId};

/// `functions` grouped by the file they are in.
pub fn by_file(functions: &[FunctionMetrics]) -> HashMap<&str, Vec<&FunctionMetrics>> {
    let mut in_file: HashMap<&str, Vec<&FunctionMetrics>> = HashMap::new();
    for function in functions {
        in_file
            .entry(function.file.as_str())
            .or_default()
            .push(function);
    }
    in_file
}

/// For each of `lines` (1-based, in any order), the innermost of
/// `functions` (one file's) holding it: the one starting last among those
/// whose lines hold it. One sweep down the file, so many functions don't
/// cost time per line.
pub fn functions_holding(lines: &[u32], functions: &[&FunctionMetrics]) -> Vec<Option<SymbolId>> {
    let mut by_start: Vec<&FunctionMetrics> = functions.to_vec();
    by_start.sort_by_key(|f| f.line);
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by_key(|&i| lines[i]);
    let span = |f: &FunctionMetrics| -> Range<u32> { f.line..f.line.saturating_add(f.lines) };
    let mut holding = vec![None; lines.len()];
    // The functions started so far that may still hold the line, innermost
    // last.
    let mut open: Vec<&FunctionMetrics> = Vec::new();
    let mut next = 0;
    for i in order {
        let line = lines[i];
        while let Some(function) = by_start.get(next).filter(|f| f.line <= line) {
            open.push(function);
            next += 1;
        }
        while open.last().is_some_and(|f| !span(f).contains(&line)) {
            open.pop();
        }
        holding[i] = open.last().map(|f| f.symbol);
    }
    holding
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Signature;

    fn function(symbol: SymbolId, line: u32, lines: u32) -> FunctionMetrics {
        FunctionMetrics {
            symbol,
            file: "a.c".into(),
            line,
            lines,
            complexity: 1,
            signature: Signature::default(),
        }
    }

    #[test]
    fn each_line_gets_the_innermost_function_holding_it() {
        // 1 holds lines 10..30; 2, nested, holds 12..16.
        let outer = function(1, 10, 20);
        let inner = function(2, 12, 4);
        let lines = [13, 5, 20, 12, 29, 30];
        let holding = functions_holding(&lines, &[&inner, &outer]);
        assert_eq!(holding, [Some(2), None, Some(1), Some(2), Some(1), None]);
    }

    #[test]
    fn functions_are_grouped_by_file() {
        let mut other = function(3, 1, 2);
        other.file = "b.c".into();
        let all = [function(1, 1, 2), other, function(2, 5, 2)];
        let grouped = by_file(&all);
        assert_eq!(grouped["a.c"].len(), 2);
        assert_eq!(grouped["b.c"][0].symbol, 3);
    }
}

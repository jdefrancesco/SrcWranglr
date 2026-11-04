//! Combines syntax colours with occurrence marks into the runs of text that
//! a line is drawn as.

use std::ops::Range;

use egui::Color32;

/// How an occurrence of a symbol is decorated. Stronger marks win where
/// marks overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mark {
    /// A resolved name: clickable.
    Link,
    /// An occurrence of the selected symbol.
    Selected,
    /// The location the code view is focused on.
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub range: Range<usize>,
    pub color: Color32,
    pub mark: Option<Mark>,
}

/// Splits `0..len` at every token and mark boundary. Each piece takes the
/// colour of the token it falls in and the strongest mark covering it.
/// Mark ranges are clamped to `len`; ones that don't land on char boundaries
/// of `line` are ignored.
pub fn merge(
    line: &str,
    tokens: &[(Range<usize>, Color32)],
    marks: &[(Range<usize>, Mark)],
) -> Vec<Segment> {
    let len = line.len();
    let marks: Vec<(Range<usize>, Mark)> = marks
        .iter()
        .map(|(r, m)| (r.start.min(len)..r.end.min(len), *m))
        .filter(|(r, _)| r.start < r.end)
        .filter(|(r, _)| line.is_char_boundary(r.start) && line.is_char_boundary(r.end))
        .collect();

    let mut cuts: Vec<usize> = tokens
        .iter()
        .flat_map(|(r, _)| [r.start, r.end])
        .chain(marks.iter().flat_map(|(r, _)| [r.start, r.end]))
        .chain([0, len])
        .filter(|&c| c <= len)
        .collect();
    cuts.sort_unstable();
    cuts.dedup();

    cuts.windows(2)
        .map(|w| w[0]..w[1])
        .map(|range| Segment {
            color: tokens
                .iter()
                .find(|(r, _)| r.contains(&range.start))
                .map_or(Color32::GRAY, |(_, c)| *c),
            mark: marks
                .iter()
                .filter(|(r, _)| r.contains(&range.start))
                .map(|(_, m)| *m)
                .max(),
            range,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Color32 = Color32::RED;
    const BLUE: Color32 = Color32::BLUE;

    fn summary(segments: &[Segment]) -> Vec<(usize, usize, Option<Mark>)> {
        segments
            .iter()
            .map(|s| (s.range.start, s.range.end, s.mark))
            .collect()
    }

    #[test]
    fn splits_tokens_at_mark_boundaries() {
        // "foo(bar)": tokens "foo" "(" "bar" ")", with "bar" linked.
        let tokens = [(0..3, RED), (3..4, BLUE), (4..7, RED), (7..8, BLUE)];
        let segments = merge("foo(bar)", &tokens, &[(4..7, Mark::Link)]);
        assert_eq!(
            summary(&segments),
            [
                (0, 3, None),
                (3, 4, None),
                (4, 7, Some(Mark::Link)),
                (7, 8, None)
            ]
        );
        assert_eq!(segments[2].color, RED);
    }

    #[test]
    fn a_mark_inside_a_token_splits_it() {
        let segments = merge("abcdef", &[(0..6, RED)], &[(2..4, Mark::Selected)]);
        assert_eq!(
            summary(&segments),
            [(0, 2, None), (2, 4, Some(Mark::Selected)), (4, 6, None)]
        );
        assert!(segments.iter().all(|s| s.color == RED));
    }

    #[test]
    fn strongest_overlapping_mark_wins() {
        let marks = [
            (0..3, Mark::Link),
            (0..3, Mark::Active),
            (0..3, Mark::Selected),
        ];
        let segments = merge("abc", &[(0..3, RED)], &marks);
        assert_eq!(summary(&segments), [(0, 3, Some(Mark::Active))]);
    }

    #[test]
    fn clamps_marks_past_the_end_and_drops_bad_ones() {
        let line = "é = x"; // 'é' is two bytes
        let marks = [
            (1..2, Mark::Link),
            (6..9, Mark::Link),
            (4..40, Mark::Selected),
        ];
        let segments = merge(line, &[(0..line.len(), RED)], &marks);
        assert_eq!(
            summary(&segments),
            [(0, 4, None), (4, 6, Some(Mark::Selected))]
        );
    }

    #[test]
    fn empty_line_has_no_segments() {
        assert!(merge("", &[], &[(0..3, Mark::Link)]).is_empty());
    }
}

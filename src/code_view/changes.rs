//! What a compared commit range changed in the file shown, looked up per
//! line as it is drawn: sorted runs of new or changed lines, and where
//! lines were removed.

/// Lines removed at a place in the file shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Removal {
    /// The line they were removed just before (or, if `is_after`, just
    /// after: at the end of the file).
    pub line: u32,
    pub count: u32,
    pub is_after: bool,
}

/// What a compared range did to a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineChange<'a> {
    /// New or changed since `base`.
    Changed { base: &'a str },
    /// `count` lines just before it were removed since `base`.
    RemovedBefore { count: u32, base: &'a str },
    /// `count` lines just after it (the file's last) were removed.
    RemovedAfter { count: u32, base: &'a str },
}

/// A file's changes in a compared range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileChanges<'a> {
    pub base: &'a str,
    /// Runs of new or changed lines `(first, last)`, sorted, not
    /// overlapping.
    pub runs: &'a [(u32, u32)],
    /// Sorted by line.
    pub removals: &'a [Removal],
}

impl<'a> FileChanges<'a> {
    /// What the range did to `line`, if anything: a change first, else a
    /// removal next to it.
    pub fn at(&self, line: u32) -> Option<LineChange<'a>> {
        let base = self.base;
        if contains(self.runs, line) {
            return Some(LineChange::Changed { base });
        }
        let at = self.removals.partition_point(|r| r.line < line);
        let removal = self.removals.get(at).filter(|r| r.line == line)?;
        Some(match removal.is_after {
            true => LineChange::RemovedAfter {
                count: removal.count,
                base,
            },
            false => LineChange::RemovedBefore {
                count: removal.count,
                base,
            },
        })
    }
}

/// Whether one of the sorted, non-overlapping `runs` holds `line`.
pub fn contains(runs: &[(u32, u32)], line: u32) -> bool {
    let at = runs.partition_point(|(_, last)| *last < line);
    runs.get(at).is_some_and(|(first, _)| *first <= line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_line_is_looked_up_in_the_runs_then_the_removals() {
        let runs = [(1, 2), (5, 5)];
        let removals = [
            Removal {
                line: 4,
                count: 2,
                is_after: false,
            },
            Removal {
                line: 9,
                count: 1,
                is_after: true,
            },
        ];
        let changes = FileChanges {
            base: "v1",
            runs: &runs,
            removals: &removals,
        };
        let changed = Some(LineChange::Changed { base: "v1" });
        assert_eq!(changes.at(1), changed);
        assert_eq!(changes.at(2), changed);
        assert_eq!(changes.at(3), None);
        assert_eq!(
            changes.at(4),
            Some(LineChange::RemovedBefore {
                count: 2,
                base: "v1"
            })
        );
        assert_eq!(changes.at(5), changed);
        assert_eq!(
            changes.at(9),
            Some(LineChange::RemovedAfter {
                count: 1,
                base: "v1"
            })
        );
        assert_eq!(changes.at(10), None);
    }

    #[test]
    fn a_line_is_found_in_sorted_runs() {
        let runs = [(3, 4), (8, 12), (20, 20)];
        let found: Vec<u32> = (1..=21).filter(|&l| contains(&runs, l)).collect();
        assert_eq!(found, [3, 4, 8, 9, 10, 11, 12, 20]);
        assert!(!contains(&[], 1));
    }
}

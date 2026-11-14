//! The git history of the checked-out code: which commits say they fixed
//! a bug or a security problem, and which lines of the code as checked out
//! each of them last changed. Code fixed once is a good place to look for
//! the same bug again.
//!
//! The repository belongs to the code being audited, so it is read in
//! process (gitoxide), never by running `git`: a repository's own config
//! can make `git` run programs. It is opened isolated (no system, user or
//! environment config) and with reduced trust (its own config isn't used
//! for anything that runs a program), and only committed objects are read:
//! no working tree filters or diff drivers. Every walk has a limit.

pub mod changes;
pub mod cves;
mod file;
pub mod findings;
mod keywords;
mod read;
pub mod worker;

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use file::{commits_in, file_history, FileHistory};
pub use keywords::Strength;
pub use read::read;

/// A commit whose message says it fixed something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixCommit {
    /// Full hex id.
    pub id: String,
    pub short: String,
    /// The message's first line, cut short if long.
    pub summary: String,
    pub author: String,
    /// Seconds since 1970, UTC.
    pub time: i64,
    /// The words in its message that marked it, lower case.
    pub keywords: Vec<String>,
    pub strength: Strength,
    /// The files it changed (none listed for a merge), cut short if many.
    #[serde(default)]
    pub files: Vec<String>,
}

/// Lines of the checked-out code last changed by a fix commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedLines {
    /// Project-relative path.
    pub file: String,
    /// 1-based.
    pub first_line: u32,
    pub line_count: u32,
    /// Index into `History::fix_commits`.
    pub commit: usize,
}

impl OwnedLines {
    pub fn last_line(&self) -> u32 {
        self.first_line
            .saturating_add(self.line_count.saturating_sub(1))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    /// The checked-out commit, short.
    pub head: String,
    /// Commits read, newest first from the checked-out one.
    pub commits_read: usize,
    /// Whether there were older commits than were read.
    pub is_walk_cut_short: bool,
    /// Newest first.
    pub fix_commits: Vec<FixCommit>,
    /// By file, then line.
    pub owned: Vec<OwnedLines>,
    /// Files whose lines were traced to commits.
    pub files_traced: usize,
    /// Files fix commits changed that weren't traced (limits).
    pub files_not_traced: usize,
    /// Files that differ from the checked-out commit, so their lines can't
    /// be told apart: left out.
    pub files_changed: Vec<String>,
}

/// How much of the history is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_commits: usize,
    pub max_fix_commits: usize,
    pub max_files: usize,
    /// Files larger than this aren't traced.
    pub max_file_bytes: usize,
    pub budget: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_commits: 20_000,
            max_fix_commits: 2_000,
            max_files: 2_000,
            max_file_bytes: 2 * 1024 * 1024,
            budget: Duration::from_secs(90),
        }
    }
}

/// `seconds` since 1970 as `YYYY-MM-DD` (UTC).
pub fn date(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    // Days to civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
pub(crate) mod test_repo;
#[cfg(test)]
mod tests;

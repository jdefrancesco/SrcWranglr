//! The CVEs the history names: CVE ids in the messages of the commits that
//! fixed them, each with those commits. Read from the checkout's own
//! history only; nothing is looked up.

use std::collections::HashMap;

use super::History;

/// A CVE named by fix commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cve {
    /// `CVE-2024-1234`.
    pub id: String,
    /// The commits naming it, by index into `History::fix_commits`, newest
    /// first.
    pub commits: Vec<usize>,
}

/// The CVEs named in `history`'s fix commits, the most recently fixed
/// first.
pub fn cves(history: &History) -> Vec<Cve> {
    let mut found: Vec<Cve> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for (index, commit) in history.fix_commits.iter().enumerate() {
        for id in commit.keywords.iter().filter_map(|word| cve_id(word)) {
            match by_id.get(&id) {
                Some(&at) => {
                    let commits = &mut found[at].commits;
                    // Commits come in order, so a repeat is the last one.
                    if commits.last() != Some(&index) {
                        commits.push(index);
                    }
                }
                None => {
                    by_id.insert(id.clone(), found.len());
                    found.push(Cve {
                        id,
                        commits: vec![index],
                    });
                }
            }
        }
    }
    found
}

/// `word` (`cve-2024-1234`, lower case) as a CVE id, if it is one:
/// `CVE-`, a four-digit year, `-`, then four to seven digits.
pub fn cve_id(word: &str) -> Option<String> {
    let rest = word.to_ascii_lowercase();
    let rest = rest.strip_prefix("cve-")?;
    let (year, number) = rest.split_once('-')?;
    let is_digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    let is_id =
        year.len() == 4 && is_digits(year) && (4..=7).contains(&number.len()) && is_digits(number);
    is_id.then(|| format!("CVE-{year}-{number}"))
}

/// The CVE's page in the National Vulnerability Database, for the browser.
pub fn nvd_url(id: &str) -> Option<String> {
    let id = cve_id(id)?;
    Some(format!("https://nvd.nist.gov/vuln/detail/{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{FixCommit, Strength};

    fn commit(keywords: &[&str]) -> FixCommit {
        FixCommit {
            id: "0".repeat(40),
            short: "00000000".into(),
            summary: "fix".into(),
            author: "a".into(),
            time: 0,
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
            strength: Strength::Security,
            files: Vec::new(),
        }
    }

    #[test]
    fn each_cve_lists_the_commits_naming_it_newest_first() {
        let history = History {
            fix_commits: vec![
                commit(&["fix", "cve-2024-1234"]),
                commit(&["cve-2023-55555", "cve-2024-1234"]),
                commit(&["overflow"]),
            ],
            ..History::default()
        };
        assert_eq!(
            cves(&history),
            [
                Cve {
                    id: "CVE-2024-1234".into(),
                    commits: vec![0, 1]
                },
                Cve {
                    id: "CVE-2023-55555".into(),
                    commits: vec![1]
                },
            ]
        );
    }

    #[test]
    fn only_well_formed_ids_count_and_link_to_nvd() {
        assert_eq!(cve_id("cve-2021-44228").as_deref(), Some("CVE-2021-44228"));
        for bad in [
            "cve-21-44228",
            "cve-2021-12",
            "cve-2021-12345678",
            "cve-2021-4a228",
            "cve-2021",
        ] {
            assert_eq!(cve_id(bad), None, "{bad}");
        }
        assert_eq!(
            nvd_url("CVE-2021-44228").as_deref(),
            Some("https://nvd.nist.gov/vuln/detail/CVE-2021-44228")
        );
        assert_eq!(nvd_url("CVE-2021-44228/../x"), None);
    }
}

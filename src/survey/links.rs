//! Where the codebase may live online: its git remotes and submodules, and
//! repository URLs its README and package manifests mention. Read from
//! files only; nothing is fetched and git is never run.

use std::collections::BTreeMap;

/// Code hosts whose `host/owner/repo` URLs name a repository.
const HOSTS: &[(&str, &str)] = &[
    ("github.com", "github.com"),
    ("raw.githubusercontent.com", "github.com"),
    ("gitlab.com", "gitlab.com"),
    ("bitbucket.org", "bitbucket.org"),
    ("codeberg.org", "codeberg.org"),
    ("sourceforge.net/projects", "sourceforge.net/projects"),
];
/// First path segments that are site pages, not owners.
const NOT_OWNERS: &[&str] = &[
    "about",
    "apps",
    "collections",
    "contact",
    "explore",
    "features",
    "join",
    "login",
    "marketplace",
    "notifications",
    "orgs",
    "pricing",
    "security",
    "settings",
    "site",
    "sponsors",
    "topics",
    "user-attachments",
];
const MAX_URL_CHARS: usize = 300;

/// A repository URL and where it was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoLink {
    /// `https://host/owner/repo` for known hosts, else the remote as
    /// written (without credentials).
    pub url: String,
    /// Whether `url` is a recognized `https://host/owner/repo` link.
    pub is_web: bool,
    /// e.g. `git remote "origin"`, `README.md ×3`.
    pub sources: Vec<String>,
    /// Git remotes rank first, then by how often the URL is mentioned.
    rank: (u8, usize),
}

/// Collects links, merging repeats of the same URL.
#[derive(Debug, Default)]
pub struct LinkCollector {
    links: BTreeMap<String, RepoLink>,
}

impl LinkCollector {
    /// Adds each `url = …` remote in a git config or `.gitmodules` file.
    pub fn add_git_config(&mut self, text: &str, is_submodules: bool) {
        let mut section = String::new();
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                section = line.trim_matches(['[', ']']).to_string();
                continue;
            }
            let Some(value) = line.strip_prefix("url").map(str::trim_start) else {
                continue;
            };
            let Some(url) = value.strip_prefix('=').map(str::trim) else {
                continue;
            };
            let name = section.split('"').nth(1).unwrap_or(&section);
            let (source, rank) = if is_submodules {
                (format!("git submodule \"{name}\""), 1)
            } else {
                (
                    format!("git remote \"{name}\""),
                    u8::from(name != "origin") * 2,
                )
            };
            if let Some((url, is_web)) = normalize_remote(url) {
                self.add(url, is_web, source, (rank, 0));
            }
        }
    }

    /// Adds every repository URL mentioned in `text` (a README, manifest…).
    pub fn add_mentions(&mut self, text: &str, file: &str) {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for (host_text, host) in HOSTS {
            let pattern = format!("{host_text}/");
            for (at, _) in text.match_indices(&pattern) {
                // `github.com`, not `notgithub.com` or `pages.github.com`.
                let before = &text[..at];
                let is_host_start = !before
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
                    || before.ends_with("www.");
                if !is_host_start {
                    continue;
                }
                if let Some(url) = repo_url(host, &text[at + pattern.len()..]) {
                    *counts.entry(url).or_default() += 1;
                }
            }
        }
        for (url, count) in counts {
            let source = if count > 1 {
                format!("{file} ×{count}")
            } else {
                file.to_string()
            };
            self.add(url, true, source, (3, count));
        }
    }

    fn add(&mut self, url: String, is_web: bool, source: String, rank: (u8, usize)) {
        let link = self.links.entry(url.clone()).or_insert(RepoLink {
            url,
            is_web,
            sources: Vec::new(),
            rank: (u8::MAX, 0),
        });
        link.sources.push(source);
        link.rank = (link.rank.0.min(rank.0), link.rank.1 + rank.1);
    }

    /// The links, most likely to be the project's own first.
    pub fn finish(self) -> Vec<RepoLink> {
        let mut links: Vec<RepoLink> = self.links.into_values().collect();
        links.sort_by(|a, b| {
            (a.rank.0, std::cmp::Reverse(a.rank.1), &a.url).cmp(&(
                b.rank.0,
                std::cmp::Reverse(b.rank.1),
                &b.url,
            ))
        });
        links
    }
}

/// `https://host/owner/repo` from the text after `host/`, if it names one.
fn repo_url(host: &str, rest: &str) -> Option<String> {
    let path: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
        .take(MAX_URL_CHARS)
        .collect();
    let mut segments = path.split('/');
    let owner = segments.next().filter(|s| is_name(s))?;
    let repo = segments.next()?;
    let repo = repo
        .strip_suffix(".git")
        .unwrap_or(repo)
        .trim_end_matches('.');
    if NOT_OWNERS.contains(&owner.to_ascii_lowercase().as_str()) || !is_name(repo) {
        return None;
    }
    Some(format!("https://{host}/{owner}/{repo}"))
}

fn is_name(segment: &str) -> bool {
    !segment.is_empty()
        && !segment.starts_with('.')
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A remote URL as a web link when it is on a known host, else as written
/// without any `user:password@`; `None` if it isn't printable text.
fn normalize_remote(url: &str) -> Option<(String, bool)> {
    if url.is_empty() || url.len() > MAX_URL_CHARS || url.chars().any(char::is_control) {
        return None;
    }
    // scp-like `git@github.com:owner/repo.git`.
    let (host_and_path, scheme) = match url.split_once("://") {
        Some((scheme, rest)) => (rest, Some(scheme)),
        None => (url, None),
    };
    let without_user = host_and_path
        .split_once('@')
        .filter(|(user, _)| !user.contains('/'))
        .map_or(host_and_path, |(_, rest)| rest);
    let as_path = if scheme.is_none() {
        without_user.replacen(':', "/", 1)
    } else {
        without_user.to_string()
    };
    for (host_text, host) in HOSTS {
        if let Some(rest) = as_path.strip_prefix(&format!("{host_text}/")) {
            if let Some(web) = repo_url(host, rest) {
                return Some((web, true));
            }
        }
    }
    let shown = match scheme {
        Some(scheme) => format!("{scheme}://{without_user}"),
        None => without_user.to_string(),
    };
    Some((shown, false))
}

/// A GitHub search for repositories named like the project.
pub fn github_search_url(project_name: &str) -> String {
    let query: String = project_name
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("https://github.com/search?q={query}&type=repositories")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(links: &[RepoLink]) -> Vec<&str> {
        links.iter().map(|l| l.url.as_str()).collect()
    }

    #[test]
    fn reads_remotes_in_every_url_style() {
        let config = r#"
[core]
    bare = false
[remote "upstream"]
    url = git@github.com:google/leveldb.git
[remote "origin"]
    url = https://user:secret@gitlab.com/me/leveldb-fork.git
    fetch = +refs/heads/*:refs/remotes/origin/*
[remote "mirror"]
    url = ssh://git@git.example.com:2222/team/leveldb.git
"#;
        let mut links = LinkCollector::default();
        links.add_git_config(config, false);
        let links = links.finish();
        assert_eq!(
            urls(&links),
            vec![
                "https://gitlab.com/me/leveldb-fork",
                "https://github.com/google/leveldb",
                "ssh://git.example.com:2222/team/leveldb.git",
            ]
        );
        assert_eq!(links[0].sources, vec!["git remote \"origin\""]);
        assert!(links[0].is_web && !links[2].is_web);
        assert!(links.iter().all(|l| !l.url.contains("secret")));
    }

    #[test]
    fn finds_repositories_mentioned_in_text() {
        let readme =
            "[![CI](https://github.com/google/leveldb/actions/workflows/build.yml/badge.svg)]\n\
            Clone https://github.com/google/leveldb.git or see\n\
            https://raw.githubusercontent.com/google/leveldb/main/README.md and\n\
            https://github.com/topics/database and https://notgithub.com/x/y\n";
        let mut links = LinkCollector::default();
        links.add_mentions(readme, "README.md");
        let links = links.finish();
        assert_eq!(urls(&links), vec!["https://github.com/google/leveldb"]);
        assert_eq!(links[0].sources, vec!["README.md ×3"]);
    }

    #[test]
    fn remotes_rank_before_mentions_and_merge_with_them() {
        let mut links = LinkCollector::default();
        links.add_mentions("see https://github.com/other/thing", "README.md");
        links.add_mentions("https://github.com/google/leveldb", "CMakeLists.txt");
        links.add_git_config(
            "[remote \"origin\"]\nurl = https://github.com/google/leveldb\n",
            false,
        );
        let links = links.finish();
        assert_eq!(urls(&links)[0], "https://github.com/google/leveldb");
        assert_eq!(
            links[0].sources,
            vec!["CMakeLists.txt", "git remote \"origin\""]
        );
    }

    #[test]
    fn lists_submodules() {
        let gitmodules = "[submodule \"third_party/googletest\"]\n\tpath = third_party/googletest\n\turl = https://github.com/google/googletest.git\n";
        let mut links = LinkCollector::default();
        links.add_git_config(gitmodules, true);
        let links = links.finish();
        assert_eq!(urls(&links), vec!["https://github.com/google/googletest"]);
        assert_eq!(
            links[0].sources,
            vec!["git submodule \"third_party/googletest\""]
        );
    }

    #[test]
    fn ignores_unprintable_remotes() {
        let mut links = LinkCollector::default();
        links.add_git_config(
            "[remote \"origin\"]\nurl = https://evil\u{1b}[2J/x\n",
            false,
        );
        assert!(links.finish().is_empty());
    }

    #[test]
    fn builds_a_github_search_link() {
        assert_eq!(
            github_search_url("my proj&x"),
            "https://github.com/search?q=my%20proj%26x&type=repositories"
        );
    }
}

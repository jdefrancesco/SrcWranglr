//! Which commit messages say a bug was fixed or a problem left: words like
//! `fix`, `overflow`, `CVE-2024-1234`, `use-after-free`, `TODO`, `revert`.
//! Security words count for more than plain fixes.

/// How strongly a word says the code had a bug worth a look.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Strength {
    /// A security problem: overflow, use-after-free, a CVE, a fuzzer crash.
    Security,
    /// A fix or known problem of any kind.
    Fix,
}

/// Words (lower case, whole) that mark a security problem.
const SECURITY_WORDS: &[&str] = &[
    "security",
    "vuln",
    "vulnerability",
    "vulnerabilities",
    "exploit",
    "overflow",
    "overflows",
    "underflow",
    "overrun",
    "overread",
    "uaf",
    "use-after-free",
    "double-free",
    "oob",
    "out-of-bounds",
    "injection",
    "asan",
    "ubsan",
    "msan",
    "oss-fuzz",
    "ossfuzz",
    "fuzz",
    "fuzzer",
    "fuzzing",
    "crash",
    "crashes",
    "segfault",
    "sigsegv",
    "leak",
    "leaks",
    "race",
    "dos",
    "sanitizer",
    "uninitialized",
    "deref",
    "dereference",
];

/// Words (lower case, whole) that mark a fix or a known problem.
const FIX_WORDS: &[&str] = &[
    "fix",
    "fixes",
    "fixed",
    "fixing",
    "bugfix",
    "hotfix",
    "bug",
    "bugs",
    "revert",
    "reverts",
    "workaround",
    "hack",
    "todo",
    "fixme",
    "xxx",
    "broken",
];

/// The marking words in `message`, each once, in the order first written;
/// and the strongest of them. `None` if it has none.
pub fn keywords_in(message: &str) -> Option<(Vec<String>, Strength)> {
    let mut found: Vec<String> = Vec::new();
    let mut strongest = None;
    for word in words(message) {
        let Some(strength) = strength_of(&word) else {
            continue;
        };
        strongest = Some(strongest.map_or(strength, |s: Strength| s.min(strength)));
        if !found.contains(&word) {
            found.push(word);
        }
    }
    strongest.map(|strength| (found, strength))
}

fn strength_of(word: &str) -> Option<Strength> {
    if is_cve(word) || SECURITY_WORDS.contains(&word) {
        Some(Strength::Security)
    } else if FIX_WORDS.contains(&word) {
        Some(Strength::Fix)
    } else {
        None
    }
}

/// `cve-2024-1234`.
fn is_cve(word: &str) -> bool {
    word.strip_prefix("cve-")
        .is_some_and(|rest| rest.len() >= 6 && rest.chars().all(|c| c.is_ascii_digit() || c == '-'))
}

/// `message`'s words, lower case: letters, digits and inner hyphens.
fn words(message: &str) -> impl Iterator<Item = String> + '_ {
    message
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .map(|word| word.trim_matches('-').to_lowercase())
        .filter(|word| !word.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(message: &str) -> Option<(Vec<String>, Strength)> {
        keywords_in(message)
    }

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn fixes_and_security_words_are_found_whole_and_in_any_case() {
        assert_eq!(
            found("Fix: heap Overflow in parse_header (CVE-2024-12345)"),
            Some((
                words(&["fix", "overflow", "cve-2024-12345"]),
                Strength::Security
            ))
        );
        assert_eq!(
            found("fixes #12, fix again"),
            Some((words(&["fixes", "fix"]), Strength::Fix))
        );
        assert_eq!(
            found("Avoid use-after-free in close()"),
            Some((words(&["use-after-free"]), Strength::Security))
        );
        assert_eq!(
            found("Revert \"Add cache\""),
            Some((words(&["revert"]), Strength::Fix))
        );
    }

    #[test]
    fn other_words_and_parts_of_words_are_not() {
        for message in [
            "Add a prefix option",
            "Update docs",
            "debugging output",
            "fixture data",
            "cve-12",
            "Raceway is a word",
        ] {
            assert_eq!(found(message), None, "{message}");
        }
    }
}

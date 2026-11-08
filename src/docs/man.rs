//! Man pages for library functions, from this system's own manual (never
//! the audited project's pages). Only a plain C name is looked up, in fixed
//! sections, so nothing from the code reaches `man` as an option or a path.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::process::{run_process, Limits, ProcessError};

/// Library functions first, then system calls, then POSIX's pages (Linux).
const SECTIONS: [&str; 3] = ["3", "2", "3p"];
const MAX_NAME_BYTES: usize = 64;
/// How long one `man` may take.
const DEADLINE: Duration = Duration::from_secs(5);
/// More of a page than this isn't read.
const MAX_PAGE_BYTES: u64 = 2 * 1024 * 1024;
/// Columns a page is laid out in.
const PAGE_WIDTH: &str = "80";
const SYSTEM_MAN: &str = "/usr/bin/man";

/// A function's man page, as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManPage {
    pub name: String,
    pub section: String,
    pub text: String,
}

impl ManPage {
    /// What its NAME section says, e.g. "stpcpy, strcpy – copy strings".
    pub fn summary(&self) -> Option<String> {
        let mut lines = self.text.lines().map(str::trim);
        lines.find(|line| *line == "NAME")?;
        let first = lines.find(|line| !line.is_empty())?;
        Some(first.to_string())
    }

    /// "strcpy(3)".
    pub fn title(&self) -> String {
        format!("{}({})", self.name, self.section)
    }
}

/// The name `called` is looked up by: `memcpy` of `std::memcpy` or
/// `::memcpy`. `None` for anything else than a plain C name.
pub fn lookup_name(called: &str) -> Option<&str> {
    let name = called
        .strip_prefix("std::")
        .or_else(|| called.strip_prefix("::"))
        .unwrap_or(called);
    let mut chars = name.chars();
    let is_start = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    let is_rest = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    (is_start && is_rest && name.len() <= MAX_NAME_BYTES).then_some(name)
}

/// The man page for the function `called`, from the first section that
/// has one; `None` if no section does or the name isn't a C name.
pub fn man_page(called: &str, stop: &AtomicBool) -> Result<Option<ManPage>, String> {
    let Some(name) = lookup_name(called) else {
        return Ok(None);
    };
    let program = man_program();
    for section in SECTIONS {
        let text = page_in(&program, section, name, stop)?;
        if !text.trim().is_empty() {
            return Ok(Some(ManPage {
                name: name.to_string(),
                section: section.to_string(),
                text,
            }));
        }
    }
    Ok(None)
}

fn man_program() -> PathBuf {
    match Path::new(SYSTEM_MAN).is_file() {
        true => PathBuf::from(SYSTEM_MAN),
        false => PathBuf::from("man"),
    }
}

/// The page for `name` in `section`, as plain text: empty if there is none.
fn page_in(program: &Path, section: &str, name: &str, stop: &AtomicBool) -> Result<String, String> {
    let args = [OsString::from(section), OsString::from(name)];
    let env = [
        ("MANPAGER", "cat"),
        ("PAGER", "cat"),
        ("MANWIDTH", PAGE_WIDTH),
        ("GROFF_NO_SGR", "1"),
    ];
    let limits = Limits {
        deadline: DEADLINE,
        max_output: MAX_PAGE_BYTES,
    };
    match run_process("man", program, &args, &env, b"", limits, stop) {
        Ok(out) if out.len() as u64 >= MAX_PAGE_BYTES => Ok(format!(
            "{}\n\n[The page is longer than this; the rest isn't shown.]",
            plain(&String::from_utf8_lossy(&out))
        )),
        Ok(out) => Ok(plain(&String::from_utf8_lossy(&out))),
        Err(ProcessError::TookTooLong) => Err("Reading the man page took too long.".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// `text` without the typewriter overstrikes (`c\bc` for bold, `_\bc` for
/// underline), color codes and other control characters a terminal would
/// act on.
pub fn plain(text: &str) -> String {
    let mut out: Vec<char> = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{8}' => {
                out.pop();
            }
            '\u{1b}' => skip_escape(&mut chars),
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.into_iter().collect()
}

/// Skips the rest of an escape sequence: `[`, parameters, a final letter.
fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars>) {
    if chars.next_if_eq(&'[').is_none() {
        return;
    }
    for c in chars.by_ref() {
        if c.is_ascii_alphabetic() || c == '~' {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_c_names_are_looked_up() {
        assert_eq!(lookup_name("memcpy"), Some("memcpy"));
        assert_eq!(lookup_name("std::memcpy"), Some("memcpy"));
        assert_eq!(lookup_name("::_exit"), Some("_exit"));
        for bad in [
            "",
            "-P",
            "--pager=sh",
            "../etc/passwd",
            "a b",
            "obj.read",
            "p->read",
            "ns::parse",
            "1abc",
            "strcpy\u{202e}",
            &"a".repeat(65),
        ] {
            assert_eq!(lookup_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn overstrikes_and_escapes_are_dropped() {
        assert_eq!(plain("N\u{8}NA\u{8}AM\u{8}ME\u{8}E"), "NAME");
        assert_eq!(plain("_\u{8}d_\u{8}s_\u{8}t"), "dst");
        assert_eq!(plain("\u{1b}[1mbold\u{1b}[0m\tx\r\n"), "bold\tx\n");
    }

    #[test]
    fn the_summary_is_the_line_under_name() {
        let page = ManPage {
            name: "strcpy".into(),
            section: "3".into(),
            text: "STRCPY(3)   Library Functions Manual\n\nNAME\n     stpcpy, strcpy – copy \
                   strings\n\nSYNOPSIS\n"
                .into(),
        };
        assert_eq!(
            page.summary().as_deref(),
            Some("stpcpy, strcpy – copy strings")
        );
        assert_eq!(page.title(), "strcpy(3)");
    }

    #[test]
    fn a_name_that_is_not_c_is_not_looked_up() {
        assert_eq!(man_page("--help", &AtomicBool::new(false)), Ok(None));
    }

    /// Reads this system's real page for `strcpy`; needs man pages.
    #[test]
    #[ignore = "needs the system's man pages"]
    fn reads_strcpy_from_this_system() {
        let page = man_page("strcpy", &AtomicBool::new(false))
            .expect("man ran")
            .expect("a page");
        assert_eq!(page.section, "3");
        assert!(page.summary().expect("a NAME line").contains("strcpy"));
        assert!(!page.text.contains('\u{8}'));
        let read = man_page("read", &AtomicBool::new(false))
            .expect("man ran")
            .expect("a page");
        assert!(["2", "3", "3p"].contains(&read.section.as_str()));
        assert_eq!(
            man_page("no_such_function_xyz", &AtomicBool::new(false)),
            Ok(None)
        );
    }
}

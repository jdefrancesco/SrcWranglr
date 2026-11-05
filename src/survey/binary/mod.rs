//! Compiled programs and libraries found in the project folder (ELF, PE,
//! Mach-O): what they are, whether they are stripped, which exploit
//! mitigations they were built with (ASLR/PIE, NX, RELRO, canaries, …),
//! and which IPC their imports point to. Parsing is done by `goblin` on a
//! copy of the file's bytes; a malformed file is reported, never trusted.

mod elf;
mod macho;
mod pe;

use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::analysis::capabilities::Category;

/// How many bytes identify a binary format.
pub const MAGIC_BYTES: usize = 8;

/// A mitigation's verdict, for coloring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Present and effective.
    Good,
    /// Partial, or absent but less important.
    Weak,
    /// Missing.
    Bad,
    /// Can't be told from the file.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

impl Check {
    fn new(name: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name,
            status,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryReport {
    /// Project-relative path (plus `[arch]` for a slice of a universal binary).
    pub path: String,
    pub size: u64,
    /// e.g. `ELF 64-bit`, `PE32+`, `Mach-O 64-bit`.
    pub format: String,
    pub arch: String,
    /// e.g. `PIE executable`, `shared library`, `DLL`.
    pub kind: String,
    pub is_stripped: bool,
    /// What symbols or debug information the file carries.
    pub symbols: String,
    pub checks: Vec<Check>,
    pub libraries: Vec<String>,
    /// Imported functions by what they let the program do.
    pub capabilities: Vec<(Category, Vec<String>)>,
}

/// What a file turned out to be.
#[derive(Debug)]
pub enum Inspection {
    /// Programs and libraries (several for a universal Mach-O).
    Binaries(Vec<BinaryReport>),
    /// Object files and static archives: build leftovers, not listed.
    Object,
    /// A binary that couldn't be parsed, and why.
    Unreadable(String),
}

/// Whether `magic` (a file's first bytes) starts a binary format.
pub fn looks_like_binary(magic: &[u8]) -> bool {
    const MACH_MAGICS: [[u8; 4]; 4] = [
        [0xfe, 0xed, 0xfa, 0xce],
        [0xfe, 0xed, 0xfa, 0xcf],
        [0xce, 0xfa, 0xed, 0xfe],
        [0xcf, 0xfa, 0xed, 0xfe],
    ];
    magic.starts_with(b"\x7fELF")
        || magic.starts_with(b"MZ")
        || magic.starts_with(b"!<arch>\n")
        || MACH_MAGICS.iter().any(|m| magic.starts_with(m))
        || is_fat_mach(magic)
}

/// `CAFEBABE` starts both universal Mach-O files and Java classes; a
/// universal binary has a small architecture count where a class file
/// has its version number.
fn is_fat_mach(magic: &[u8]) -> bool {
    const MAX_FAT_ARCHES: u32 = 30;
    magic.len() >= 8
        && magic.starts_with(&[0xca, 0xfe, 0xba, 0xbe])
        && u32::from_be_bytes([magic[4], magic[5], magic[6], magic[7]]) < MAX_FAT_ARCHES
}

/// Inspects a whole file's `bytes`, found at `path`.
pub fn inspect(path: &str, bytes: &[u8]) -> Inspection {
    // goblin is memory-safe, but a malformed file must not take the app
    // down even if it hits a panic somewhere in the parser.
    catch_unwind(AssertUnwindSafe(|| inspect_bytes(path, bytes)))
        .unwrap_or_else(|_| Inspection::Unreadable("the parser gave up on this file".into()))
}

fn inspect_bytes(path: &str, bytes: &[u8]) -> Inspection {
    let size = bytes.len() as u64;
    match goblin::Object::parse(bytes) {
        Ok(goblin::Object::Elf(elf)) => elf::report(path, size, &elf),
        Ok(goblin::Object::PE(pe)) => pe::report(path, size, &pe),
        Ok(goblin::Object::Mach(mach)) => macho::report(path, size, mach),
        Ok(goblin::Object::Archive(_) | goblin::Object::COFF(_)) => Inspection::Object,
        Ok(_) => Inspection::Unreadable("not a format slurcetrail inspects".into()),
        Err(error) => Inspection::Unreadable(error.to_string()),
    }
}

/// Names of fortified libc calls (`__memcpy_chk`), without the canary.
fn fortified<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut found: Vec<String> = names
        .into_iter()
        .map(|n| n.trim_start_matches('_'))
        .filter(|n| n.ends_with("_chk"))
        .map(|n| format!("__{n}"))
        .collect();
    found.sort();
    found.dedup();
    found
}

const MAX_LISTED: usize = 4;

/// `a, b, c, d and 3 more`.
fn short_list(items: &[String]) -> String {
    let shown = items
        .iter()
        .take(MAX_LISTED)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match items.len().saturating_sub(MAX_LISTED) {
        0 => shown,
        more => format!("{shown} and {more} more"),
    }
}

fn fortify_check(fortified: &[String]) -> Check {
    if fortified.is_empty() {
        Check::new(
            "FORTIFY_SOURCE",
            Status::Weak,
            "no fortified (__*_chk) calls found",
        )
    } else {
        Check::new("FORTIFY_SOURCE", Status::Good, short_list(fortified))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_binary_magic_numbers() {
        assert!(looks_like_binary(b"\x7fELF\x02\x01\x01\0"));
        assert!(looks_like_binary(b"MZ\x90\0\x03\0\0\0"));
        assert!(looks_like_binary(&[0xcf, 0xfa, 0xed, 0xfe, 7, 0, 0, 1]));
        assert!(looks_like_binary(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2]));
        assert!(looks_like_binary(b"!<arch>\n"));
        // A Java class (version 52) is not a universal binary.
        assert!(!looks_like_binary(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 52]));
        assert!(!looks_like_binary(b"#include"));
    }

    #[test]
    fn reports_garbage_as_unreadable() {
        let mut bytes = b"\x7fELF\x02\x01\x01".to_vec();
        bytes.resize(40, 0xff);
        assert!(matches!(inspect("junk", &bytes), Inspection::Unreadable(_)));
    }

    #[test]
    fn finds_fortified_calls_but_not_the_canary() {
        let names = ["__memcpy_chk", "___sprintf_chk", "__stack_chk_fail", "puts"];
        assert_eq!(fortified(names), vec!["__memcpy_chk", "__sprintf_chk"]);
    }

    #[test]
    fn shortens_long_lists() {
        let items: Vec<String> = (1..=6).map(|i| i.to_string()).collect();
        assert_eq!(short_list(&items), "1, 2, 3, 4 and 2 more");
        assert_eq!(short_list(&items[..2]), "1, 2");
    }
}

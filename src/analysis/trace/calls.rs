//! What library calls do with their arguments: which ones make a risky
//! call risky, and which calls copy some arguments into another.

/// Some of a call's arguments, by position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Args {
    One(usize),
    /// This one and every one after it.
    From(usize),
    None,
}

impl Args {
    /// The positions this names among `count` arguments.
    pub(super) fn indices(self, count: usize) -> Vec<usize> {
        match self {
            Self::One(i) => (i < count).then_some(i).into_iter().collect(),
            Self::From(i) => (i..count).collect(),
            Self::None => Vec::new(),
        }
    }
}

/// A library call that writes `from` arguments into the `into` ones.
#[derive(Debug, Clone, Copy)]
pub(super) struct Write {
    pub into: Args,
    pub from: Args,
    /// Adds to what is there (`strcat`) instead of replacing it.
    pub is_append: bool,
}

const fn write(into: Args, from: Args, is_append: bool) -> Write {
    Write {
        into,
        from,
        is_append,
    }
}

const WRITES: &[(&[&str], Write)] = &[
    (
        &[
            "strcpy",
            "stpcpy",
            "wcscpy",
            "lstrcpy",
            "lstrcpyA",
            "lstrcpyW",
            "_tcscpy",
            "_mbscpy",
            "StrCpy",
            "strncpy",
            "stpncpy",
            "wcsncpy",
            "lstrcpyn",
            "strlcpy",
            "memcpy",
            "mempcpy",
            "memmove",
            "wmemcpy",
            "wmemmove",
            "CopyMemory",
            "RtlCopyMemory",
            "MoveMemory",
            "RtlMoveMemory",
        ],
        write(Args::One(0), Args::One(1), false),
    ),
    (
        &[
            "strcat", "wcscat", "lstrcat", "lstrcatA", "lstrcatW", "_tcscat", "_mbscat", "StrCat",
            "strncat", "wcsncat", "strlcat",
        ],
        write(Args::One(0), Args::One(1), true),
    ),
    (
        &["strcpy_s", "wcscpy_s", "strncpy_s", "memcpy_s", "memmove_s"],
        write(Args::One(0), Args::One(2), false),
    ),
    (
        &["strcat_s", "wcscat_s", "strncat_s"],
        write(Args::One(0), Args::One(2), true),
    ),
    (&["bcopy"], write(Args::One(1), Args::One(0), false)),
    (
        &[
            "sprintf",
            "vsprintf",
            "wsprintf",
            "wsprintfA",
            "wsprintfW",
            "_stprintf",
        ],
        write(Args::One(0), Args::From(1), false),
    ),
    (
        &[
            "snprintf",
            "vsnprintf",
            "_snprintf",
            "swprintf",
            "vswprintf",
        ],
        write(Args::One(0), Args::From(2), false),
    ),
    (
        &["sscanf", "vsscanf", "swscanf"],
        write(Args::From(2), Args::One(0), false),
    ),
    (
        &[
            "memset",
            "wmemset",
            "bzero",
            "explicit_bzero",
            "ZeroMemory",
            "SecureZeroMemory",
            "RtlZeroMemory",
        ],
        write(Args::One(0), Args::None, false),
    ),
];

/// What `name` (a bare library name) writes, if it copies into an argument.
pub(super) fn write_of(name: &str) -> Option<Write> {
    WRITES
        .iter()
        .find(|(names, _)| names.contains(&name))
        .map(|(_, write)| *write)
}

/// Methods that write their arguments into their object, and whether they
/// add to it: `s.assign(x)`, `s.append(x)`.
pub(super) fn method_write(method: &str) -> Option<bool> {
    match method {
        "assign" => Some(false),
        "append" | "push_back" | "emplace_back" | "insert" | "replace" => Some(true),
        _ => None,
    }
}

/// The argument positions that make a call to `name` risky, out of
/// `count`: the source and length of a copy, a format's arguments, a
/// command, a size, a path. Every argument for a call not listed.
pub(super) fn risky_args(name: &str, count: usize) -> Vec<usize> {
    let (some, from): (&[usize], Option<usize>) = match name {
        "strcpy" | "stpcpy" | "wcscpy" | "lstrcpy" | "lstrcpyA" | "lstrcpyW" | "_tcscpy"
        | "_mbscpy" | "StrCpy" | "strcat" | "wcscat" | "lstrcat" | "lstrcatA" | "lstrcatW"
        | "_tcscat" | "_mbscat" | "StrCat" | "realloc" => (&[1], None),
        "strncpy" | "stpncpy" | "wcsncpy" | "strncat" | "wcsncat" | "strlcpy" | "strlcat"
        | "memcpy" | "mempcpy" | "memmove" | "wmemcpy" | "wmemmove" | "CopyMemory"
        | "RtlCopyMemory" | "MoveMemory" | "RtlMoveMemory" | "calloc" | "rename" | "link"
        | "symlink" => (&[1, 2], None),
        "strcpy_s" | "wcscpy_s" | "strcat_s" | "wcscat_s" => (&[1, 2], None),
        "strncpy_s" | "strncat_s" | "memcpy_s" | "memmove_s" => (&[1, 2, 3], None),
        "bcopy" => (&[0, 2], None),
        "sprintf" | "vsprintf" | "wsprintf" | "wsprintfA" | "wsprintfW" | "_stprintf"
        | "fprintf" | "vfprintf" | "dprintf" | "syslog" | "fwprintf" => (&[], Some(1)),
        "snprintf" | "vsnprintf" | "_snprintf" | "swprintf" | "vswprintf" => (&[1], Some(2)),
        "printf" | "vprintf" | "wprintf" => (&[], Some(0)),
        "sscanf" | "vsscanf" | "swscanf" => (&[0, 1], None),
        "system" | "_wsystem" | "popen" | "_popen" | "WinExec" | "malloc" | "alloca"
        | "_alloca" | "valloc" | "fopen" | "open" | "access" | "stat" | "lstat" | "unlink"
        | "remove" | "chmod" | "chown" | "mkdir" | "rmdir" | "opendir" | "dlopen"
        | "LoadLibrary" | "LoadLibraryA" | "LoadLibraryW" => (&[0], None),
        _ => (&[], Some(0)),
    };
    let listed = some.iter().copied().filter(|&i| i < count);
    let rest = from.map_or(Vec::new(), |i| Args::From(i).indices(count));
    let mut all: Vec<usize> = listed.chain(rest).collect();
    all.sort_unstable();
    all.dedup();
    all
}

/// `std::strcpy` and `::strcpy` are `strcpy`; other qualified names keep
/// their scope, so they match nothing here.
pub(super) fn library_name(called: &str) -> &str {
    called
        .strip_prefix("std::")
        .or_else(|| called.strip_prefix("::"))
        .unwrap_or(called)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_is_risky_for_its_source_and_length() {
        assert_eq!(risky_args("strcpy", 2), [1]);
        assert_eq!(risky_args("memcpy", 3), [1, 2]);
        assert_eq!(risky_args("snprintf", 5), [1, 2, 3, 4]);
        assert_eq!(risky_args("system", 1), [0]);
        assert_eq!(risky_args("my_unknown", 2), [0, 1]);
        assert_eq!(risky_args("strcpy", 1), Vec::<usize>::new());
    }

    #[test]
    fn knows_what_copies_into_what() {
        let sprintf = write_of("sprintf").expect("listed");
        assert_eq!(sprintf.into, Args::One(0));
        assert_eq!(sprintf.from.indices(4), [1, 2, 3]);
        assert!(write_of("strcat").is_some_and(|w| w.is_append));
        assert_eq!(write_of("memset").map(|w| w.from), Some(Args::None));
        assert!(write_of("strlen").is_none());
        assert_eq!(library_name("std::memcpy"), "memcpy");
        assert_eq!(library_name("mylib::memcpy"), "mylib::memcpy");
    }
}

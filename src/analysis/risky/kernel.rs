//! The Linux kernel: where data crosses from and to user space, and
//! helpers the kernel has replaced. The kernel defines these itself, so
//! they are checked where the tree defines them too (see
//! `is_checked_in_tree`). Its copies of the C library (`strcpy`,
//! `sprintf`) and its allocators (`kmalloc(n * size)`) are judged by the
//! C library's rules and the argument checks.

use super::Rule;
use crate::analysis::Severity;

pub(super) const RULES: &[Rule] = &[
    Rule {
        names: &[
            "copy_from_user",
            "_copy_from_user",
            "raw_copy_from_user",
            "copy_from_user_nofault",
            "get_user",
        ],
        severity: Severity::Medium,
        cwe: 20,
        why: "Reads memory user space controls: every size, index and offset in it is \
              attacker-chosen, and reading it twice can give two different values.",
        fix: "Copy it once into a kernel buffer, check every size and index in that copy, \
              and check the call's result.",
    },
    Rule {
        names: &[
            "__copy_from_user",
            "__copy_from_user_inatomic",
            "__get_user",
            "__copy_to_user",
            "__copy_to_user_inatomic",
            "__put_user",
        ],
        severity: Severity::Medium,
        cwe: 20,
        why: "Skips the access_ok() check: unless the caller checked the user address range, \
              a pointer from user space can reach kernel memory.",
        fix: "Call access_ok() on the whole range first, or use the checked form \
              (copy_from_user, get_user).",
    },
    Rule {
        names: &[
            "copy_to_user",
            "_copy_to_user",
            "raw_copy_to_user",
            "copy_to_user_nofault",
            "put_user",
        ],
        severity: Severity::Medium,
        cwe: 200,
        why: "Writes kernel memory out to user space: any part not set first, such as a \
              struct's padding, leaks kernel data.",
        fix: "Zero the buffer or struct (padding included) before filling it, and copy only \
              what was set.",
    },
    Rule {
        names: &["memdup_user", "memdup_user_nul", "vmemdup_user"],
        severity: Severity::Low,
        cwe: 770,
        why: "Allocates as much as it is asked to: a length that comes from user space must \
              be capped first.",
        fix: "Bound the length before the call.",
    },
    Rule {
        names: &["strncpy_from_user"],
        severity: Severity::Low,
        cwe: 170,
        why: "The copy isn't NUL-terminated when the string fills the count.",
        fix: "Pass one less than the buffer's size and terminate it, or use strndup_user \
              with a capped length.",
    },
    Rule {
        names: &[
            "simple_strtol",
            "simple_strtoul",
            "simple_strtoll",
            "simple_strtoull",
        ],
        severity: Severity::Low,
        cwe: 190,
        why: "Obsolete: doesn't detect overflow or text after the number.",
        fix: "Use kstrtol / kstrtoul and check the result.",
    },
];

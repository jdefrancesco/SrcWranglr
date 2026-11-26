//! The library functions the emulator models: allocators (how big a region
//! they make) and copies (which argument is the destination and which is the
//! byte count). Unlisted functions don't touch modeled memory.

/// How an allocator's size is formed from its arguments.
pub enum AllocKind {
    /// `malloc(n)` → argument `i`.
    Arg(usize),
    /// `calloc(a, b)` → argument `i` times argument `j`.
    Product(usize, usize),
    /// `strdup(s)` → a real but unknown size.
    UnknownSize,
}

/// The allocator `name` is, if any (namespace-qualified names allowed).
pub fn alloc_size(name: &str) -> Option<AllocKind> {
    match base(name) {
        "malloc" | "alloca" | "valloc" | "xmalloc" | "g_malloc" | "g_malloc0" | "g_try_malloc"
        | "vmalloc" | "kmalloc" | "kzalloc" | "kvmalloc" => Some(AllocKind::Arg(0)),
        "realloc" | "g_realloc" | "xrealloc" => Some(AllocKind::Arg(1)),
        "calloc" | "kcalloc" | "reallocarray" | "g_malloc_n" => Some(AllocKind::Product(0, 1)),
        "strdup" | "strndup" | "g_strdup" => Some(AllocKind::UnknownSize),
        _ => None,
    }
}

/// A modeled copy: which argument is the destination, and which (if any) is the
/// number of bytes written.
pub struct Copy {
    pub dest: usize,
    pub size: Option<usize>,
}

/// The copy `name` is, if any.
pub fn copy_bytes(name: &str) -> Option<Copy> {
    match base(name) {
        "memcpy" | "memmove" | "memset" | "strncpy" => Some(Copy {
            dest: 0,
            size: Some(2),
        }),
        // Implicit length: modeled as a write of an unknown size (not flagged).
        "strcpy" | "strcat" | "sprintf" => Some(Copy {
            dest: 0,
            size: None,
        }),
        _ => None,
    }
}

/// The function name without any `ns::` qualifier.
fn base(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

//! ELF executables and shared libraries (Linux, BSD, embedded).

use goblin::elf::dynamic::{DF_1_NOW, DF_1_PIE, DF_BIND_NOW, DT_BIND_NOW};
use goblin::elf::header::{ET_DYN, ET_EXEC, ET_REL};
use goblin::elf::program_header::{PF_X, PT_GNU_RELRO, PT_GNU_STACK};
use goblin::elf::Elf;

use super::{fortified, fortify_check, BinaryReport, Check, Inspection, Status};
use crate::analysis::capabilities::classify_imports;

const CANARY_SYMBOLS: &[&str] = &[
    "__stack_chk_fail",
    "__stack_chk_fail_local",
    "__stack_chk_guard",
    "__intel_security_cookie",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ElfKind {
    Executable,
    Pie,
    SharedLibrary,
    Object,
    Other,
}

/// What the mitigation checks need to know about one ELF file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ElfFacts {
    kind: ElfKind,
    /// From `PT_GNU_STACK`: whether the stack is executable.
    is_stack_executable: Option<bool>,
    has_relro: bool,
    is_bind_now: bool,
    has_canary: bool,
    /// No interpreter and no shared libraries.
    is_static: bool,
    /// Entries in `.symtab` (0 when stripped).
    symbol_count: usize,
    has_debug_info: bool,
    fortified: Vec<String>,
    rpaths: Vec<String>,
}

impl ElfFacts {
    fn of(elf: &Elf) -> Self {
        let info = elf.dynamic.as_ref().map(|d| &d.info);
        let flags = info.map_or(0, |i| i.flags);
        let flags_1 = info.map_or(0, |i| i.flags_1);
        let has_bind_now_tag = elf
            .dynamic
            .as_ref()
            .is_some_and(|d| d.dyns.iter().any(|entry| entry.d_tag == DT_BIND_NOW));
        let names = symbol_names(elf);
        let section_names = elf
            .section_headers
            .iter()
            .filter_map(|sh| elf.shdr_strtab.get_at(sh.sh_name));
        Self {
            kind: kind_of(
                elf.header.e_type,
                elf.interpreter.is_some(),
                flags_1 & DF_1_PIE != 0,
            ),
            is_stack_executable: elf
                .program_headers
                .iter()
                .find(|ph| ph.p_type == PT_GNU_STACK)
                .map(|ph| ph.p_flags & PF_X != 0),
            has_relro: elf
                .program_headers
                .iter()
                .any(|ph| ph.p_type == PT_GNU_RELRO),
            is_bind_now: has_bind_now_tag || flags & DF_BIND_NOW != 0 || flags_1 & DF_1_NOW != 0,
            has_canary: names.iter().any(|n| CANARY_SYMBOLS.contains(n)),
            is_static: elf.interpreter.is_none() && elf.libraries.is_empty(),
            symbol_count: elf.syms.len(),
            has_debug_info: section_names
                .into_iter()
                .any(|n| n.starts_with(".debug_") || n.starts_with(".zdebug_")),
            fortified: fortified(names.iter().copied()),
            rpaths: elf
                .rpaths
                .iter()
                .chain(&elf.runpaths)
                .map(|p| p.to_string())
                .collect(),
        }
    }
}

fn kind_of(e_type: u16, has_interpreter: bool, has_pie_flag: bool) -> ElfKind {
    match e_type {
        ET_REL => ElfKind::Object,
        ET_EXEC => ElfKind::Executable,
        ET_DYN if has_interpreter || has_pie_flag => ElfKind::Pie,
        ET_DYN => ElfKind::SharedLibrary,
        _ => ElfKind::Other,
    }
}

/// Dynamic and static symbol names.
fn symbol_names<'a>(elf: &'a Elf) -> Vec<&'a str> {
    let dynamic = elf
        .dynsyms
        .iter()
        .filter_map(|s| elf.dynstrtab.get_at(s.st_name));
    let local = elf.syms.iter().filter_map(|s| elf.strtab.get_at(s.st_name));
    dynamic.chain(local).filter(|n| !n.is_empty()).collect()
}

/// Functions the program takes from shared libraries; for a static
/// program, the functions linked into it.
fn imported_functions<'a>(elf: &'a Elf) -> Vec<&'a str> {
    let imports: Vec<&str> = elf
        .dynsyms
        .iter()
        .filter(|s| s.is_import() && s.is_function())
        .filter_map(|s| elf.dynstrtab.get_at(s.st_name))
        .collect();
    if !imports.is_empty() {
        return imports;
    }
    elf.syms
        .iter()
        .filter(|s| s.is_function())
        .filter_map(|s| elf.strtab.get_at(s.st_name))
        .collect()
}

pub(super) fn report(path: &str, size: u64, elf: &Elf) -> Inspection {
    let facts = ElfFacts::of(elf);
    if facts.kind == ElfKind::Object {
        return Inspection::Object;
    }
    let bits = if elf.is_64 { 64 } else { 32 };
    Inspection::Binaries(vec![BinaryReport {
        path: path.to_string(),
        size,
        format: format!("ELF {bits}-bit"),
        arch: goblin::elf::header::machine_to_str(elf.header.e_machine).to_string(),
        kind: kind_label(&facts),
        is_stripped: facts.symbol_count == 0,
        symbols: symbols_label(&facts),
        checks: checks(&facts),
        libraries: elf.libraries.iter().map(|l| l.to_string()).collect(),
        capabilities: classify_imports(imported_functions(elf)),
    }])
}

fn kind_label(facts: &ElfFacts) -> String {
    let kind = match facts.kind {
        ElfKind::Executable => "executable",
        ElfKind::Pie => "PIE executable",
        ElfKind::SharedLibrary => "shared library",
        ElfKind::Object => "object file",
        ElfKind::Other => "other ELF file",
    };
    if facts.is_static && facts.kind != ElfKind::SharedLibrary {
        format!("{kind}, statically linked")
    } else {
        kind.to_string()
    }
}

fn symbols_label(facts: &ElfFacts) -> String {
    match (facts.symbol_count, facts.has_debug_info) {
        (0, false) => "stripped: only dynamic symbols".into(),
        (0, true) => "debug info, but no symbol table".into(),
        (n, false) => format!("not stripped: {n} symbols"),
        (n, true) => format!("not stripped: {n} symbols and DWARF debug info"),
    }
}

fn checks(facts: &ElfFacts) -> Vec<Check> {
    let mut checks = vec![
        aslr_check(facts.kind),
        nx_check(facts.is_stack_executable),
        relro_check(facts.has_relro, facts.is_bind_now),
        canary_check(facts),
        fortify_check(&facts.fortified),
    ];
    if !facts.rpaths.is_empty() {
        checks.push(Check::new(
            "RPATH / RUNPATH",
            Status::Weak,
            format!(
                "libraries are looked for first in {}",
                facts.rpaths.join(":")
            ),
        ));
    }
    checks
}

fn aslr_check(kind: ElfKind) -> Check {
    const NAME: &str = "ASLR (PIE)";
    match kind {
        ElfKind::Pie => Check::new(
            NAME,
            Status::Good,
            "position-independent: loaded at a random address",
        ),
        ElfKind::SharedLibrary => {
            Check::new(NAME, Status::Good, "shared library: position-independent")
        }
        ElfKind::Executable => Check::new(
            NAME,
            Status::Bad,
            "not PIE: the code is at a fixed address, so ASLR can't move it",
        ),
        ElfKind::Object | ElfKind::Other => Check::new(NAME, Status::Unknown, "not a program"),
    }
}

fn nx_check(is_stack_executable: Option<bool>) -> Check {
    const NAME: &str = "NX stack";
    match is_stack_executable {
        Some(false) => Check::new(NAME, Status::Good, "the stack is not executable"),
        Some(true) => Check::new(
            NAME,
            Status::Bad,
            "the stack is executable (PT_GNU_STACK has X)",
        ),
        None => Check::new(
            NAME,
            Status::Weak,
            "no PT_GNU_STACK: some loaders make the stack executable",
        ),
    }
}

fn relro_check(has_relro: bool, is_bind_now: bool) -> Check {
    const NAME: &str = "RELRO";
    match (has_relro, is_bind_now) {
        (true, true) => Check::new(
            NAME,
            Status::Good,
            "full: the GOT is read-only after startup",
        ),
        (true, false) => Check::new(
            NAME,
            Status::Weak,
            "partial: the GOT stays writable (lazy binding)",
        ),
        (false, _) => Check::new(NAME, Status::Bad, "none: relocation tables stay writable"),
    }
}

fn canary_check(facts: &ElfFacts) -> Check {
    const NAME: &str = "Stack canary";
    if facts.has_canary {
        Check::new(NAME, Status::Good, "uses __stack_chk_fail")
    } else if facts.is_static && facts.symbol_count == 0 {
        Check::new(NAME, Status::Unknown, "static and stripped: can't tell")
    } else {
        Check::new(
            NAME,
            Status::Bad,
            "no __stack_chk_fail: functions have no stack protector",
        )
    }
}

#[cfg(test)]
#[path = "elf_tests.rs"]
mod tests;

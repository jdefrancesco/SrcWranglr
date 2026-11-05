//! Tests for ELF inspection, on minimal ELF files built byte by byte.

use super::*;
use crate::survey::binary::inspect;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const DT_FLAGS_1: u64 = 0x6fff_fffb;
const PF_RW: u32 = 6;
const HEADER_SIZE: usize = 64;
const PHDR_SIZE: usize = 56;

/// A 64-bit little-endian x86-64 ELF file of type `e_type` with the given
/// program headers (`PT_GNU_STACK`/`PT_GNU_RELRO` by type and flags), an
/// interpreter if `has_interp`, and these dynamic entries.
fn elf_file(
    e_type: u16,
    extra: &[(u32, u32)],
    has_interp: bool,
    dynamic: &[(u64, u64)],
) -> Vec<u8> {
    let interp = b"/lib64/ld-linux-x86-64.so.2\0";
    let phnum = 1 + extra.len() + usize::from(has_interp) + usize::from(!dynamic.is_empty());
    let data_start = HEADER_SIZE + phnum * PHDR_SIZE;
    let dyn_start = data_start + interp.len();
    let dyn_size = (dynamic.len() + 1) * 16;
    let total = dyn_start + dyn_size;

    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
    out.extend_from_slice(&e_type.to_le_bytes());
    out.extend_from_slice(&62u16.to_le_bytes()); // x86-64
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes()); // entry
    out.extend_from_slice(&(HEADER_SIZE as u64).to_le_bytes()); // phoff
    out.extend_from_slice(&0u64.to_le_bytes()); // shoff
    out.extend_from_slice(&0u32.to_le_bytes()); // flags
    for half in [HEADER_SIZE, PHDR_SIZE, phnum, 64, 0, 0] {
        out.extend_from_slice(&(half as u16).to_le_bytes());
    }
    let mut phdr = |p_type: u32, flags: u32, offset: usize, size: usize| {
        out.extend_from_slice(&p_type.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        for value in [offset, offset, offset, size, size, 8] {
            out.extend_from_slice(&(value as u64).to_le_bytes());
        }
    };
    phdr(PT_LOAD, 5, 0, total);
    for (p_type, flags) in extra {
        phdr(*p_type, *flags, 0, 0);
    }
    if has_interp {
        phdr(PT_INTERP, 4, data_start, interp.len());
    }
    if !dynamic.is_empty() {
        phdr(PT_DYNAMIC, PF_RW, dyn_start, dyn_size);
    }
    out.extend_from_slice(interp);
    for (tag, value) in dynamic.iter().chain(&[(0, 0)]) {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn only_report(bytes: &[u8]) -> BinaryReport {
    match inspect("bin/app", bytes) {
        Inspection::Binaries(mut reports) if reports.len() == 1 => reports.remove(0),
        other => panic!("expected one binary, got {other:?}"),
    }
}

fn status_of(report: &BinaryReport, name: &str) -> Status {
    report
        .checks
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no {name} check"))
        .status
}

#[test]
fn a_hardened_pie_passes_aslr_nx_and_relro() {
    let bytes = elf_file(
        ET_DYN,
        &[(PT_GNU_STACK, PF_RW), (PT_GNU_RELRO, 4)],
        true,
        &[(DT_FLAGS_1, DF_1_NOW | DF_1_PIE)],
    );
    let report = only_report(&bytes);
    assert_eq!(
        (
            report.format.as_str(),
            report.arch.as_str(),
            report.kind.as_str()
        ),
        ("ELF 64-bit", "X86_64", "PIE executable")
    );
    assert_eq!(status_of(&report, "ASLR (PIE)"), Status::Good);
    assert_eq!(status_of(&report, "NX stack"), Status::Good);
    assert_eq!(status_of(&report, "RELRO"), Status::Good);
    assert!(report.is_stripped);
    assert_eq!(report.size, bytes.len() as u64);
}

#[test]
fn an_old_style_executable_fails_aslr_nx_and_relro() {
    let report = only_report(&elf_file(ET_EXEC, &[], false, &[]));
    assert_eq!(report.kind, "executable, statically linked");
    assert_eq!(status_of(&report, "ASLR (PIE)"), Status::Bad);
    assert_eq!(status_of(&report, "NX stack"), Status::Weak);
    assert_eq!(status_of(&report, "RELRO"), Status::Bad);
    assert_eq!(status_of(&report, "Stack canary"), Status::Unknown);
}

#[test]
fn an_executable_stack_and_partial_relro_are_flagged() {
    let bytes = elf_file(ET_DYN, &[(PT_GNU_STACK, 7), (PT_GNU_RELRO, 4)], true, &[]);
    let report = only_report(&bytes);
    assert_eq!(status_of(&report, "NX stack"), Status::Bad);
    assert_eq!(status_of(&report, "RELRO"), Status::Weak);
}

#[test]
fn a_library_without_an_interpreter_is_a_shared_library() {
    let report = only_report(&elf_file(ET_DYN, &[], false, &[(DT_FLAGS_1, DF_1_NOW)]));
    assert_eq!(report.kind, "shared library");
    assert_eq!(status_of(&report, "ASLR (PIE)"), Status::Good);
}

#[test]
fn object_files_are_not_listed() {
    assert!(matches!(
        inspect("a.o", &elf_file(ET_REL, &[], false, &[])),
        Inspection::Object
    ));
}

#[test]
fn checks_cover_canaries_fortify_and_rpath() {
    let facts = ElfFacts {
        kind: ElfKind::Pie,
        is_stack_executable: Some(false),
        has_relro: true,
        is_bind_now: false,
        has_canary: true,
        is_static: false,
        symbol_count: 12,
        has_debug_info: true,
        fortified: vec!["__memcpy_chk".into()],
        rpaths: vec!["$ORIGIN/../lib".into()],
    };
    let checks = checks(&facts);
    let summary: Vec<(&str, Status)> = checks.iter().map(|c| (c.name, c.status)).collect();
    assert_eq!(
        summary,
        vec![
            ("ASLR (PIE)", Status::Good),
            ("NX stack", Status::Good),
            ("RELRO", Status::Weak),
            ("Stack canary", Status::Good),
            ("FORTIFY_SOURCE", Status::Good),
            ("RPATH / RUNPATH", Status::Weak),
        ]
    );
    assert!(checks[5].detail.contains("$ORIGIN/../lib"));
    assert_eq!(
        symbols_label(&facts),
        "not stripped: 12 symbols and DWARF debug info"
    );
    let dynamic_without_canary = ElfFacts {
        has_canary: false,
        ..facts
    };
    assert_eq!(canary_check(&dynamic_without_canary).status, Status::Bad);
}

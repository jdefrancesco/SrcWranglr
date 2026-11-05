//! PE executables and DLLs (Windows).

use goblin::pe::characteristic::IMAGE_FILE_RELOCS_STRIPPED;
use goblin::pe::dll_characteristic::{
    IMAGE_DLLCHARACTERISTICS_DYNAMIC_BASE, IMAGE_DLLCHARACTERISTICS_GUARD_CF,
    IMAGE_DLLCHARACTERISTICS_HIGH_ENTROPY_VA, IMAGE_DLLCHARACTERISTICS_NO_SEH,
    IMAGE_DLLCHARACTERISTICS_NX_COMPAT,
};
use goblin::pe::PE;

use super::{BinaryReport, Check, Inspection, Status};
use crate::analysis::capabilities::classify_imports;

/// What the mitigation checks need to know about one PE file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PeFacts {
    is_64: bool,
    is_dotnet: bool,
    dll_characteristics: u16,
    are_relocations_stripped: bool,
    /// From the load config: `None` when there is none.
    has_security_cookie: Option<bool>,
    /// MinGW-style canaries (`__stack_chk_fail` imported).
    imports_stack_chk: bool,
    has_safe_seh_table: bool,
    is_signed: bool,
}

impl PeFacts {
    fn of(pe: &PE) -> Self {
        let load_config = pe.load_config_data.as_ref().map(|l| &l.directory);
        Self {
            is_64: pe.is_64,
            is_dotnet: pe.clr_data.is_some(),
            dll_characteristics: pe
                .header
                .optional_header
                .map_or(0, |h| h.windows_fields.dll_characteristics),
            are_relocations_stripped: pe.header.coff_header.characteristics
                & IMAGE_FILE_RELOCS_STRIPPED
                != 0,
            has_security_cookie: load_config.map(|c| c.security_cookie.is_some_and(|v| v != 0)),
            imports_stack_chk: pe.imports.iter().any(|i| i.name == "__stack_chk_fail"),
            has_safe_seh_table: load_config.is_some_and(|c| c.se_handler_count.unwrap_or(0) > 0),
            is_signed: !pe.certificates.is_empty(),
        }
    }

    fn has(&self, flag: u16) -> bool {
        self.dll_characteristics & flag != 0
    }
}

pub(super) fn report(path: &str, size: u64, pe: &PE) -> Inspection {
    let facts = PeFacts::of(pe);
    let coff = &pe.header.coff_header;
    let kind = if pe.is_lib { "DLL" } else { "executable" };
    let symbol_count = coff.number_of_symbol_table;
    let pdb = pe
        .debug_data
        .as_ref()
        .and_then(|d| d.codeview_pdb70_debug_info.as_ref())
        .map(|info| {
            String::from_utf8_lossy(info.filename)
                .trim_end_matches('\0')
                .to_string()
        });
    let symbols = match (symbol_count, &pdb) {
        (0, Some(pdb)) => format!("no symbol table; debug info in {pdb}"),
        (0, None) => "stripped: no symbols or debug info".to_string(),
        (n, _) => format!("not stripped: COFF symbol table ({n} symbols)"),
    };
    Inspection::Binaries(vec![BinaryReport {
        path: path.to_string(),
        size,
        format: if pe.is_64 { "PE32+" } else { "PE32" }.to_string(),
        arch: goblin::pe::header::machine_to_str(coff.machine).to_string(),
        kind: if facts.is_dotnet {
            format!("{kind} (.NET)")
        } else {
            kind.to_string()
        },
        is_stripped: symbol_count == 0,
        symbols,
        checks: checks(&facts),
        libraries: pe.libraries.iter().map(|l| l.to_string()).collect(),
        capabilities: classify_imports(pe.imports.iter().map(|i| i.name.as_ref())),
    }])
}

fn checks(facts: &PeFacts) -> Vec<Check> {
    let (nx, cfg) = (
        facts.has(IMAGE_DLLCHARACTERISTICS_NX_COMPAT),
        facts.has(IMAGE_DLLCHARACTERISTICS_GUARD_CF),
    );
    vec![
        aslr_check(facts),
        if nx {
            Check::new(
                "DEP (NX)",
                Status::Good,
                "NXCOMPAT: data pages aren't executable",
            )
        } else {
            Check::new("DEP (NX)", Status::Bad, "no NXCOMPAT")
        },
        if cfg {
            Check::new("Control Flow Guard", Status::Good, "built with /guard:cf")
        } else {
            Check::new(
                "Control Flow Guard",
                Status::Weak,
                "not built with /guard:cf",
            )
        },
        seh_check(facts),
        cookie_check(facts),
        if facts.is_signed {
            Check::new(
                "Signature",
                Status::Good,
                "Authenticode signature present (not verified)",
            )
        } else {
            Check::new("Signature", Status::Weak, "unsigned")
        },
    ]
}

fn aslr_check(facts: &PeFacts) -> Check {
    const NAME: &str = "ASLR (DYNAMICBASE)";
    if !facts.has(IMAGE_DLLCHARACTERISTICS_DYNAMIC_BASE) {
        return Check::new(
            NAME,
            Status::Bad,
            "no DYNAMICBASE: loaded at a fixed address",
        );
    }
    if facts.are_relocations_stripped {
        return Check::new(
            NAME,
            Status::Bad,
            "DYNAMICBASE, but relocations are stripped",
        );
    }
    match (
        facts.is_64,
        facts.has(IMAGE_DLLCHARACTERISTICS_HIGH_ENTROPY_VA),
    ) {
        (true, true) => Check::new(NAME, Status::Good, "random base, high-entropy (64-bit)"),
        (true, false) => Check::new(NAME, Status::Weak, "random base, but no HIGHENTROPYVA"),
        (false, _) => Check::new(NAME, Status::Good, "random base"),
    }
}

fn seh_check(facts: &PeFacts) -> Check {
    const NAME: &str = "SEH";
    if facts.has(IMAGE_DLLCHARACTERISTICS_NO_SEH) {
        Check::new(
            NAME,
            Status::Good,
            "NO_SEH: no exception handlers to overwrite",
        )
    } else if facts.is_64 {
        Check::new(
            NAME,
            Status::Good,
            "table-based exception handling (64-bit)",
        )
    } else if facts.has_safe_seh_table {
        Check::new(NAME, Status::Good, "SafeSEH handler table")
    } else {
        Check::new(
            NAME,
            Status::Bad,
            "no SafeSEH: handlers on the stack can be overwritten",
        )
    }
}

fn cookie_check(facts: &PeFacts) -> Check {
    const NAME: &str = "Stack cookies (/GS)";
    if facts.is_dotnet {
        return Check::new(NAME, Status::Unknown, "managed (.NET) code");
    }
    match (facts.has_security_cookie, facts.imports_stack_chk) {
        (_, true) => Check::new(NAME, Status::Good, "uses __stack_chk_fail (MinGW)"),
        (Some(true), _) => Check::new(NAME, Status::Good, "security cookie in the load config"),
        (Some(false), _) => Check::new(NAME, Status::Weak, "no security cookie in the load config"),
        (None, _) => Check::new(
            NAME,
            Status::Weak,
            "no load config: likely built without /GS",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::survey::binary::inspect;

    const PE_OFFSET: usize = 0x40;
    const FILE_SIZE: usize = 0x400;
    const DLL_CHARACTERISTICS_AT: usize = PE_OFFSET + 4 + 20 + 70;

    /// A minimal 64-bit PE executable with one empty `.text` section.
    fn pe_file(dll_characteristics: u16) -> Vec<u8> {
        let mut out = vec![0u8; FILE_SIZE];
        out[..2].copy_from_slice(b"MZ");
        out[0x3c..0x40].copy_from_slice(&(PE_OFFSET as u32).to_le_bytes());
        let mut at = PE_OFFSET;
        let mut put = |bytes: &[u8]| {
            out[at..at + bytes.len()].copy_from_slice(bytes);
            at += bytes.len();
        };
        put(b"PE\0\0");
        // COFF: x86-64, 1 section, no symbols, 240-byte optional header.
        put(&0x8664u16.to_le_bytes());
        put(&1u16.to_le_bytes());
        put(&[0; 12]);
        put(&240u16.to_le_bytes());
        put(&0x0022u16.to_le_bytes());
        // Optional header (PE32+).
        put(&0x20bu16.to_le_bytes());
        put(&[0; 22]);
        put(&0x1_4000_0000u64.to_le_bytes()); // image base
        put(&0x1000u32.to_le_bytes()); // section alignment
        put(&0x200u32.to_le_bytes()); // file alignment
        put(&[0, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0]);
        put(&0x2000u32.to_le_bytes()); // size of image
        put(&0x200u32.to_le_bytes()); // size of headers
        put(&0u32.to_le_bytes()); // checksum
        put(&3u16.to_le_bytes()); // console subsystem
        put(&dll_characteristics.to_le_bytes());
        put(&[0; 36]);
        put(&16u32.to_le_bytes());
        put(&[0; 128]);
        // Section table.
        put(b".text\0\0\0");
        for value in [0x10u32, 0x1000, 0x200, 0x200, 0, 0, 0] {
            put(&value.to_le_bytes());
        }
        put(&0x6000_0020u32.to_le_bytes());
        out
    }

    fn only_report(bytes: &[u8]) -> BinaryReport {
        match inspect("app.exe", bytes) {
            Inspection::Binaries(mut reports) if reports.len() == 1 => reports.remove(0),
            other => panic!("expected one binary, got {other:?}"),
        }
    }

    fn statuses(report: &BinaryReport) -> Vec<(&str, Status)> {
        report.checks.iter().map(|c| (c.name, c.status)).collect()
    }

    #[test]
    fn the_builder_puts_dll_characteristics_where_the_parser_reads_them() {
        let bytes = pe_file(0x8160);
        assert_eq!(
            &bytes[DLL_CHARACTERISTICS_AT..DLL_CHARACTERISTICS_AT + 2],
            &[0x60, 0x81]
        );
    }

    #[test]
    fn a_hardened_executable_passes() {
        let all = IMAGE_DLLCHARACTERISTICS_DYNAMIC_BASE
            | IMAGE_DLLCHARACTERISTICS_HIGH_ENTROPY_VA
            | IMAGE_DLLCHARACTERISTICS_NX_COMPAT
            | IMAGE_DLLCHARACTERISTICS_GUARD_CF;
        let report = only_report(&pe_file(all));
        assert_eq!(
            (
                report.format.as_str(),
                report.arch.as_str(),
                report.kind.as_str()
            ),
            ("PE32+", "X86_64", "executable")
        );
        assert_eq!(
            statuses(&report),
            vec![
                ("ASLR (DYNAMICBASE)", Status::Good),
                ("DEP (NX)", Status::Good),
                ("Control Flow Guard", Status::Good),
                ("SEH", Status::Good),
                ("Stack cookies (/GS)", Status::Weak),
                ("Signature", Status::Weak),
            ]
        );
        assert!(report.is_stripped);
    }

    #[test]
    fn an_unhardened_executable_fails() {
        let report = only_report(&pe_file(0));
        let failing: Vec<&str> = report
            .checks
            .iter()
            .filter(|c| c.status == Status::Bad)
            .map(|c| c.name)
            .collect();
        assert_eq!(failing, vec!["ASLR (DYNAMICBASE)", "DEP (NX)"]);
    }

    #[test]
    fn thirty_two_bit_seh_and_cookies_are_judged() {
        let facts = PeFacts {
            is_64: false,
            is_dotnet: false,
            dll_characteristics: IMAGE_DLLCHARACTERISTICS_DYNAMIC_BASE,
            are_relocations_stripped: false,
            has_security_cookie: Some(true),
            imports_stack_chk: false,
            has_safe_seh_table: false,
            is_signed: true,
        };
        assert_eq!(seh_check(&facts).status, Status::Bad);
        assert_eq!(cookie_check(&facts).status, Status::Good);
        assert_eq!(aslr_check(&facts).status, Status::Good);
        let stripped_relocs = PeFacts {
            are_relocations_stripped: true,
            ..facts
        };
        assert_eq!(aslr_check(&stripped_relocs).status, Status::Bad);
    }
}

//! Mach-O executables and libraries (macOS, iOS), including each slice of
//! a universal binary.

use goblin::mach::constants::cputype::get_arch_name_from_types;
use goblin::mach::header::{
    filetype_to_str, MH_ALLOW_STACK_EXECUTION, MH_BUNDLE, MH_DYLIB, MH_EXECUTE, MH_OBJECT, MH_PIE,
};
use goblin::mach::load_command::CommandVariant;
use goblin::mach::symbols::{N_EXT, N_SECT, N_STAB, N_TYPE};
use goblin::mach::{Mach, MachO, SingleArch};

use super::{fortified, fortify_check, BinaryReport, Check, Inspection, Status};
use crate::analysis::capabilities::classify_imports;

const CANARY_IMPORTS: &[&str] = &["__stack_chk_fail", "__stack_chk_guard"];

/// What the mitigation checks need to know about one Mach-O image.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MachFacts {
    filetype: u32,
    flags: u32,
    has_canary: bool,
    is_signed: bool,
    is_encrypted: bool,
    fortified: Vec<String>,
    local_symbols: usize,
    debug_symbols: usize,
}

impl MachFacts {
    fn of(macho: &MachO, imports: &[String]) -> Self {
        let (mut local_symbols, mut debug_symbols) = (0, 0);
        for (_, nlist) in macho.symbols().flatten() {
            if nlist.n_type & N_STAB != 0 {
                debug_symbols += 1;
            } else if nlist.n_type & N_EXT == 0 && nlist.n_type & N_TYPE == N_SECT {
                local_symbols += 1;
            }
        }
        let commands = macho.load_commands.iter().map(|c| &c.command);
        Self {
            filetype: macho.header.filetype,
            flags: macho.header.flags,
            has_canary: imports.iter().any(|i| CANARY_IMPORTS.contains(&i.as_str())),
            is_signed: commands
                .clone()
                .any(|c| matches!(c, CommandVariant::CodeSignature(_))),
            is_encrypted: commands.into_iter().any(|c| match c {
                CommandVariant::EncryptionInfo32(info) => info.cryptid != 0,
                CommandVariant::EncryptionInfo64(info) => info.cryptid != 0,
                _ => false,
            }),
            fortified: fortified(imports.iter().map(String::as_str)),
            local_symbols,
            debug_symbols,
        }
    }
}

pub(super) fn report(path: &str, size: u64, mach: Mach) -> Inspection {
    match mach {
        Mach::Binary(macho) => image_report(path, size, &macho),
        Mach::Fat(multi) => {
            let mut reports = Vec::new();
            for arch in &multi {
                match arch {
                    Ok(SingleArch::MachO(macho)) => {
                        if let Inspection::Binaries(found) = image_report(path, size, &macho) {
                            reports.extend(found.into_iter().map(|r| BinaryReport {
                                path: format!("{} [{}]", r.path, r.arch),
                                ..r
                            }));
                        }
                    }
                    Ok(SingleArch::Archive(_)) => {}
                    Err(error) => return Inspection::Unreadable(error.to_string()),
                }
            }
            if reports.is_empty() {
                Inspection::Object
            } else {
                Inspection::Binaries(reports)
            }
        }
    }
}

fn image_report(path: &str, size: u64, macho: &MachO) -> Inspection {
    if macho.header.filetype == MH_OBJECT {
        return Inspection::Object;
    }
    // Mach-O symbol names carry a leading `_` (C's `socket` is `_socket`).
    let imports: Vec<String> = macho
        .imports()
        .unwrap_or_default()
        .iter()
        .map(|i| i.name.strip_prefix('_').unwrap_or(i.name).to_string())
        .collect();
    let facts = MachFacts::of(macho, &imports);
    let arch = get_arch_name_from_types(macho.header.cputype, macho.header.cpusubtype).map_or_else(
        || format!("CPU type {:#x}", macho.header.cputype),
        str::to_string,
    );
    let bits = if macho.is_64 { 64 } else { 32 };
    Inspection::Binaries(vec![BinaryReport {
        path: path.to_string(),
        size,
        format: format!("Mach-O {bits}-bit"),
        arch,
        kind: kind_label(&facts),
        is_stripped: facts.local_symbols == 0 && facts.debug_symbols == 0,
        symbols: symbols_label(&facts),
        checks: checks(&facts),
        libraries: macho
            .libs
            .iter()
            .filter(|l| **l != "self")
            .map(|l| l.to_string())
            .collect(),
        capabilities: classify_imports(imports.iter().map(String::as_str)),
    }])
}

fn kind_label(facts: &MachFacts) -> String {
    match facts.filetype {
        MH_EXECUTE if facts.flags & MH_PIE != 0 => "PIE executable".into(),
        MH_EXECUTE => "executable".into(),
        MH_DYLIB => "dynamic library".into(),
        MH_BUNDLE => "bundle".into(),
        other => filetype_to_str(other).to_ascii_lowercase(),
    }
}

fn symbols_label(facts: &MachFacts) -> String {
    match (facts.local_symbols, facts.debug_symbols) {
        (_, d) if d > 0 => format!("not stripped: {d} debug (stab) symbols"),
        (0, _) => "stripped: only exported and imported symbols".into(),
        (n, _) => format!("not stripped: {n} local symbols"),
    }
}

fn checks(facts: &MachFacts) -> Vec<Check> {
    let mut checks = vec![
        aslr_check(facts),
        if facts.flags & MH_ALLOW_STACK_EXECUTION != 0 {
            Check::new("NX stack", Status::Bad, "MH_ALLOW_STACK_EXECUTION is set")
        } else {
            Check::new("NX stack", Status::Good, "the stack is not executable")
        },
        if facts.has_canary {
            Check::new("Stack canary", Status::Good, "uses __stack_chk_fail")
        } else {
            Check::new("Stack canary", Status::Bad, "no __stack_chk_fail import")
        },
        fortify_check(&facts.fortified),
        if facts.is_signed {
            Check::new("Code signature", Status::Good, "signed (not verified)")
        } else {
            Check::new("Code signature", Status::Weak, "unsigned")
        },
    ];
    if facts.is_encrypted {
        checks.push(Check::new(
            "Encryption",
            Status::Unknown,
            "encrypted (FairPlay): the code can't be read from the file",
        ));
    }
    checks
}

fn aslr_check(facts: &MachFacts) -> Check {
    const NAME: &str = "ASLR (PIE)";
    match facts.filetype {
        MH_EXECUTE if facts.flags & MH_PIE != 0 => Check::new(
            NAME,
            Status::Good,
            "position-independent: loaded at a random address",
        ),
        MH_EXECUTE => Check::new(NAME, Status::Bad, "no MH_PIE: loaded at a fixed address"),
        _ => Check::new(NAME, Status::Good, "library: position-independent"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::survey::binary::inspect;

    #[test]
    fn judges_mach_o_mitigations() {
        let facts = MachFacts {
            filetype: MH_EXECUTE,
            flags: MH_ALLOW_STACK_EXECUTION,
            has_canary: false,
            is_signed: false,
            is_encrypted: true,
            fortified: Vec::new(),
            local_symbols: 0,
            debug_symbols: 0,
        };
        let statuses: Vec<(&str, Status)> =
            checks(&facts).iter().map(|c| (c.name, c.status)).collect();
        assert_eq!(
            statuses,
            vec![
                ("ASLR (PIE)", Status::Bad),
                ("NX stack", Status::Bad),
                ("Stack canary", Status::Bad),
                ("FORTIFY_SOURCE", Status::Weak),
                ("Code signature", Status::Weak),
                ("Encryption", Status::Unknown),
            ]
        );
        assert_eq!(kind_label(&facts), "executable");
        assert_eq!(
            symbols_label(&facts),
            "stripped: only exported and imported symbols"
        );
    }

    /// The test program itself is a real Mach-O on macOS.
    #[cfg(target_os = "macos")]
    #[test]
    fn inspects_this_test_program() {
        let path = std::env::current_exe().expect("test binary");
        let bytes = std::fs::read(path).expect("read");
        let Inspection::Binaries(reports) = inspect("test", &bytes) else {
            panic!("not inspected");
        };
        let report = &reports[0];
        assert!(report.format.starts_with("Mach-O"));
        assert_eq!(report.kind, "PIE executable");
        assert!(!report.is_stripped, "{}", report.symbols);
        assert!(report.libraries.iter().any(|l| l.contains("libSystem")));
        let aslr = report
            .checks
            .iter()
            .find(|c| c.name == "ASLR (PIE)")
            .expect("aslr");
        assert_eq!(aslr.status, Status::Good);
    }
}

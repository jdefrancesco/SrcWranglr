//! The C library and POSIX: unbounded copies and formats, commands and
//! programs run, predictable temp files, races on paths, privileges not
//! dropped, weak random numbers and parsing. Microsoft's C runtime names
//! for the same functions are here too.

use super::Rule;
use crate::analysis::Severity;

pub(super) const RULES: &[Rule] = &[
    Rule {
        names: &["gets", "_getws", "_getts", "getwd", "_cgets", "_cgetws"],
        severity: Severity::High,
        cwe: 242,
        why: "Reads input with no length limit, so any long enough input overflows the buffer.",
        fix: "Use fgets (or getcwd with a size).",
    },
    Rule {
        names: &[
            "strcpy", "wcscpy", "stpcpy", "wcpcpy", "_tcscpy", "_mbscpy", "_ftcscpy", "_tccpy",
            "_mbccpy",
        ],
        severity: Severity::High,
        cwe: 120,
        why: "Copies without knowing the destination's size: overflows if the source is longer.",
        fix: "Use snprintf or strlcpy with sizeof(dest) (strscpy in the Linux kernel), or \
              check strlen(src) first.",
    },
    Rule {
        names: &["strcat", "wcscat", "_tcscat", "_mbscat", "_ftcscat"],
        severity: Severity::High,
        cwe: 120,
        why: "Appends without knowing the destination's size: overflows if the result is longer.",
        fix: "Use snprintf or strlcat with sizeof(dest).",
    },
    Rule {
        names: &[
            "sprintf",
            "vsprintf",
            "_stprintf",
            "_vstprintf",
            "_swprintf",
        ],
        severity: Severity::High,
        cwe: 120,
        why: "Formats into a buffer of unknown size: a long argument overflows it.",
        fix: "Use snprintf with sizeof(dest) and check the result (scnprintf or sysfs_emit in \
              the Linux kernel).",
    },
    Rule {
        names: &[
            "system", "_wsystem", "_tsystem", "popen", "_popen", "_wpopen", "_tpopen",
        ],
        severity: Severity::High,
        cwe: 78,
        why: "Runs a shell command: if any part comes from input, it can inject commands.",
        fix: "Use execv/posix_spawn with an argument list, or validate strictly.",
    },
    Rule {
        names: &[
            "execl",
            "execlp",
            "execle",
            "execv",
            "execvp",
            "execvpe",
            "execvP",
            "fexecve",
            "posix_spawn",
            "posix_spawnp",
            "_execl",
            "_execlp",
            "_execv",
            "_execvp",
            "_spawnl",
            "_spawnlp",
            "_spawnv",
            "_spawnvp",
            "_wspawnl",
            "_wspawnlp",
            "_wspawnv",
            "_wspawnvp",
        ],
        severity: Severity::Medium,
        cwe: 78,
        why: "Starts a program: a path or argument from input can run something else.",
        fix: "Use absolute paths and fixed arguments; never pass input unchecked.",
    },
    Rule {
        names: &["dlopen", "dlmopen"],
        severity: Severity::Medium,
        cwe: 114,
        why: "Loads and runs a library: a path from input, or a bare name looked up along the \
              library path, can load someone else's code.",
        fix: "Load only fixed, absolute paths in folders only you can write to.",
    },
    Rule {
        names: &[
            "scanf", "fscanf", "sscanf", "vscanf", "vfscanf", "vsscanf", "wscanf", "fwscanf",
            "swscanf", "vwscanf", "vfwscanf", "vswscanf", "_tscanf", "_ftscanf", "_stscanf",
        ],
        severity: Severity::Medium,
        cwe: 120,
        why: "%s or %[ without a field width writes past the destination.",
        fix: "Give every %s / %[ a width, e.g. %63s for char buf[64].",
    },
    Rule {
        names: &[
            "strncpy", "wcsncpy", "_tcsncpy", "_mbsncpy", "stpncpy", "wcpncpy",
        ],
        severity: Severity::Medium,
        cwe: 170,
        why: "Leaves the destination without a NUL terminator when the source fills it.",
        fix: "Set dest[size - 1] = '\\0' after the call, or use strlcpy / snprintf.",
    },
    Rule {
        names: &["strncat", "wcsncat", "_tcsncat", "_mbsncat"],
        severity: Severity::Medium,
        cwe: 193,
        why: "The size is the space left (minus the NUL), not the buffer size: a common overflow.",
        fix: "Pass sizeof(dest) - strlen(dest) - 1, or use strlcat / snprintf.",
    },
    Rule {
        names: &["realpath"],
        severity: Severity::Medium,
        cwe: 120,
        why: "Writes up to PATH_MAX bytes into its second argument: a smaller buffer overflows.",
        fix: "Pass NULL to get an allocated result, or a buffer of PATH_MAX bytes.",
    },
    Rule {
        names: &["alloca", "_alloca", "__builtin_alloca"],
        severity: Severity::Medium,
        cwe: 770,
        why: "Allocates on the stack with no limit: a large size overflows the stack.",
        fix: "Bound the size, or allocate on the heap.",
    },
    Rule {
        names: &[
            "mktemp",
            "_mktemp",
            "_wmktemp",
            "tmpnam",
            "tmpnam_r",
            "_wtmpnam",
            "tempnam",
            "_tempnam",
            "_wtempnam",
        ],
        severity: Severity::Medium,
        cwe: 377,
        why: "The temporary file name is predictable: another process can create or link it first.",
        fix: "Use mkstemp / tmpfile.",
    },
    Rule {
        names: &[
            "access",
            "_access",
            "_waccess",
            "faccessat",
            "euidaccess",
            "eaccess",
        ],
        severity: Severity::Medium,
        cwe: 367,
        why: "The file can change between this check and its use (TOCTOU).",
        fix: "Open the file and check the open handle (fstat) instead.",
    },
    Rule {
        names: &["chroot"],
        severity: Severity::Medium,
        cwe: 243,
        why: "Without chdir(\"/\") straight after, the process keeps a way out to files \
              outside the new root.",
        fix: "Call chdir(\"/\") right after chroot, then drop root privileges.",
    },
    Rule {
        names: &["chmod", "chown", "lchown", "_chmod", "_wchmod"],
        severity: Severity::Low,
        cwe: 367,
        why: "Works on a path: anyone who can write to its folder can swap in a link between \
              the program's checks and this call.",
        fix: "Use fchmod / fchown on a descriptor you opened.",
    },
    Rule {
        names: &[
            "setuid",
            "setgid",
            "seteuid",
            "setegid",
            "setreuid",
            "setregid",
            "setresuid",
            "setresgid",
            "setgroups",
            "initgroups",
        ],
        severity: Severity::Low,
        cwe: 273,
        why: "If dropping privileges fails and the result isn't checked, the program goes on \
              with the privileges it meant to give up.",
        fix: "Check every result and stop on failure; drop groups before the user ID.",
    },
    Rule {
        names: &["getlogin", "getlogin_r", "cuserid"],
        severity: Severity::Low,
        cwe: 807,
        why: "The name comes from the login session, which the user can influence: it's not \
              a safe basis for an access decision.",
        fix: "Use getuid() / geteuid() and look the user up with getpwuid.",
    },
    Rule {
        names: &[
            "memcpy",
            "memmove",
            "wmemcpy",
            "wmemmove",
            "bcopy",
            "memccpy",
            "mempcpy",
            "__builtin_memcpy",
            "__builtin_memmove",
        ],
        severity: Severity::Low,
        cwe: 120,
        why: "Copies a byte count the destination may not hold.",
        fix: "Make sure the size can't exceed the destination's size.",
    },
    Rule {
        names: &[
            "rand", "rand_r", "random", "srand", "srandom", "drand48", "erand48", "lrand48",
            "nrand48", "mrand48", "jrand48", "srand48", "seed48",
        ],
        severity: Severity::Low,
        cwe: 338,
        why: "Predictable: not suitable for keys, tokens, nonces, or anything security-relevant.",
        fix: "Use getrandom / arc4random / a CSPRNG.",
    },
    Rule {
        names: &[
            "atoi", "atol", "atoll", "atof", "_wtoi", "_wtol", "_ttoi", "_atoi64", "_wtoi64",
        ],
        severity: Severity::Low,
        cwe: 190,
        why: "Can't report bad input, and out-of-range values are undefined behavior.",
        fix: "Use strtol / strtoul and check errno and the end pointer.",
    },
    Rule {
        names: &["strtok", "wcstok", "_tcstok", "_mbstok"],
        severity: Severity::Low,
        cwe: 676,
        why: "Keeps hidden state: breaks across threads or nested tokenizing loops.",
        fix: "Use strtok_r.",
    },
    Rule {
        names: &["asctime", "ctime"],
        severity: Severity::Low,
        cwe: 676,
        why: "Writes into one shared static buffer, which isn't thread-safe and which some \
              dates (years past 9999) overflow.",
        fix: "Use strftime into a buffer you size.",
    },
    Rule {
        names: &["getpass"],
        severity: Severity::Low,
        cwe: 676,
        why: "Obsolete: returns a static buffer and may silently cut a long password short.",
        fix: "Turn off the terminal's echo (tcsetattr) and read into your own buffer.",
    },
];

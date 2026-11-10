//! The Windows API and Microsoft's C runtime: unbounded string functions,
//! formats that don't terminate, programs and libraries started by search
//! path, buffer sizes counted in the wrong unit, and impersonation that
//! fails silently.

use super::Rule;
use crate::analysis::Severity;

pub(super) const RULES: &[Rule] = &[
    Rule {
        names: &[
            "lstrcpy",
            "lstrcpyA",
            "lstrcpyW",
            "StrCpy",
            "StrCpyA",
            "StrCpyW",
            "CharToOem",
            "CharToOemA",
            "CharToOemW",
            "OemToChar",
            "OemToCharA",
            "OemToCharW",
        ],
        severity: Severity::High,
        cwe: 120,
        why: "Copies without knowing the destination's size: overflows if the source is longer.",
        fix: "Use StringCchCopy with the destination's size.",
    },
    Rule {
        names: &[
            "lstrcat", "lstrcatA", "lstrcatW", "StrCat", "StrCatA", "StrCatW",
        ],
        severity: Severity::High,
        cwe: 120,
        why: "Appends without knowing the destination's size: overflows if the result is longer.",
        fix: "Use StringCchCat with the destination's size.",
    },
    Rule {
        names: &[
            "StrNCat", "StrNCatA", "StrNCatW", "StrCatN", "StrCatNA", "StrCatNW",
        ],
        severity: Severity::Medium,
        cwe: 193,
        why: "The count is how much to append, not the buffer's size: passing the buffer's size \
              overflows it.",
        fix: "Use StringCchCat with the destination's size.",
    },
    Rule {
        names: &[
            "wsprintf",
            "wsprintfA",
            "wsprintfW",
            "wvsprintf",
            "wvsprintfA",
            "wvsprintfW",
        ],
        severity: Severity::High,
        cwe: 120,
        why: "Formats up to 1024 characters into a buffer whose size it doesn't know: a long \
              argument overflows a smaller one.",
        fix: "Use StringCchPrintf with the destination's size.",
    },
    Rule {
        names: &[
            "_snprintf",
            "_snwprintf",
            "_sntprintf",
            "_vsnprintf",
            "_vsnwprintf",
            "_vsntprintf",
        ],
        severity: Severity::Medium,
        cwe: 170,
        why: "Unlike C99 snprintf, leaves the buffer without a NUL terminator when the output \
              fills it.",
        fix: "Use snprintf or _snprintf_s with _TRUNCATE, or terminate the buffer yourself.",
    },
    Rule {
        names: &[
            "WinExec",
            "ShellExecute",
            "ShellExecuteA",
            "ShellExecuteW",
            "ShellExecuteEx",
            "ShellExecuteExA",
            "ShellExecuteExW",
        ],
        severity: Severity::High,
        cwe: 78,
        why: "Runs a command line or opens a file through the shell: a name from input can \
              start another program.",
        fix: "Use CreateProcess with the program's full path and quoted arguments.",
    },
    Rule {
        names: &[
            "CreateProcess",
            "CreateProcessA",
            "CreateProcessW",
            "CreateProcessAsUser",
            "CreateProcessAsUserA",
            "CreateProcessAsUserW",
            "CreateProcessWithLogonW",
            "CreateProcessWithTokenW",
        ],
        severity: Severity::Medium,
        cwe: 428,
        why: "Without an application name, an unquoted path with spaces in the command line \
              runs the first program the split finds (C:\\Program.exe).",
        fix: "Pass the full program path as the application name, and quote it in the \
              command line.",
    },
    Rule {
        names: &[
            "LoadLibrary",
            "LoadLibraryA",
            "LoadLibraryW",
            "LoadLibraryEx",
            "LoadLibraryExA",
            "LoadLibraryExW",
        ],
        severity: Severity::Medium,
        cwe: 427,
        why: "A bare or relative name is looked up along the DLL search path, where a planted \
              DLL can be found first.",
        fix: "Pass a full path, or LOAD_LIBRARY_SEARCH_SYSTEM32 to LoadLibraryEx.",
    },
    Rule {
        names: &["MultiByteToWideChar"],
        severity: Severity::Medium,
        cwe: 120,
        why: "The output size is counted in wide characters, not bytes: passing sizeof(buffer) \
              lets it write twice the room there is.",
        fix: "Pass sizeof(buffer) / sizeof(WCHAR), or _countof(buffer).",
    },
    Rule {
        names: &[
            "ImpersonateLoggedOnUser",
            "ImpersonateNamedPipeClient",
            "ImpersonateSelf",
            "ImpersonateAnonymousToken",
            "DdeImpersonateClient",
            "RpcImpersonateClient",
            "CoImpersonateClient",
            "SetThreadToken",
        ],
        severity: Severity::Medium,
        cwe: 252,
        why: "If it fails and the result isn't checked, the code that follows runs with the \
              caller's own rights, usually higher ones.",
        fix: "Check the result and stop on failure.",
    },
    Rule {
        names: &["CopyMemory", "MoveMemory", "RtlCopyMemory", "RtlMoveMemory"],
        severity: Severity::Low,
        cwe: 120,
        why: "Copies a byte count the destination may not hold.",
        fix: "Make sure the size can't exceed the destination's size.",
    },
    Rule {
        names: &[
            "IsBadReadPtr",
            "IsBadWritePtr",
            "IsBadHugeReadPtr",
            "IsBadHugeWritePtr",
            "IsBadCodePtr",
            "IsBadStringPtr",
            "IsBadStringPtrA",
            "IsBadStringPtrW",
        ],
        severity: Severity::Low,
        cwe: 676,
        why: "Unreliable: the memory can change right after the check, and probing it can \
              trip guard pages that the stack needs to grow.",
        fix: "Remove the check and make sure by design that the pointer is valid.",
    },
];

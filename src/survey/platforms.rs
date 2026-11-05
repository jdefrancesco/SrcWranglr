//! Which operating systems a C/C++ codebase targets, from the platform
//! headers it includes and the platform macros it checks.

/// Platforms in display order, each with the include paths and macros that
/// point to it.
const PLATFORMS: &[(&str, &[&str], &[&str])] = &[
    (
        "Windows",
        &[
            "windows.h",
            "winsock2.h",
            "ws2tcpip.h",
            "winternl.h",
            "ntddk.h",
            "wdm.h",
        ],
        &["_WIN32", "_WIN64", "_MSC_VER", "__MINGW32__", "__CYGWIN__"],
    ),
    (
        "Linux",
        &[
            "linux/",
            "sys/epoll.h",
            "sys/inotify.h",
            "sys/prctl.h",
            "sys/signalfd.h",
        ],
        &["__linux__", "__linux"],
    ),
    ("Android", &["android/", "jni.h"], &["__ANDROID__"]),
    (
        "macOS / iOS",
        &[
            "mach/",
            "mach-o/",
            "CoreFoundation/",
            "dispatch/dispatch.h",
            "TargetConditionals.h",
        ],
        &["__APPLE__", "__MACH__", "TARGET_OS_IPHONE"],
    ),
    (
        "BSD",
        &["sys/event.h", "sys/sysctl.h"],
        &["__FreeBSD__", "__OpenBSD__", "__NetBSD__", "__DragonFly__"],
    ),
    (
        "POSIX",
        &[
            "unistd.h",
            "pthread.h",
            "sys/socket.h",
            "sys/mman.h",
            "fcntl.h",
        ],
        &["__unix__", "__unix", "_POSIX_VERSION"],
    ),
    (
        "WebAssembly",
        &["emscripten"],
        &["__EMSCRIPTEN__", "__wasm__"],
    ),
    (
        "Embedded / RTOS",
        &["FreeRTOS.h", "zephyr/", "cmsis", "stm32", "esp_system.h"],
        &["CONFIG_ZEPHYR", "ESP_PLATFORM", "configUSE_PREEMPTION"],
    ),
];

/// The platforms `text` points to, in display order.
pub fn platforms_in(text: &str) -> Vec<&'static str> {
    let includes: Vec<&str> = text
        .lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix('#')?.trim_start();
            Some(rest.strip_prefix("include")?.trim())
        })
        .collect();
    PLATFORMS
        .iter()
        .filter(|(_, headers, macros)| {
            let is_included = includes
                .iter()
                .any(|inc| headers.iter().any(|h| inc.contains(h)));
            is_included || macros.iter().any(|m| has_word(text, m))
        })
        .map(|(name, _, _)| *name)
        .collect()
}

/// Whether `word` appears in `text` as a whole identifier.
fn has_word(text: &str, word: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// The platform names in display order, for sorting counts.
pub fn order_of(platform: &str) -> usize {
    PLATFORMS
        .iter()
        .position(|(name, _, _)| *name == platform)
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_platforms_from_includes_and_macros() {
        let text = "#include <windows.h>\n# include <sys/epoll.h>\n#ifdef __APPLE__\n#endif\n";
        assert_eq!(platforms_in(text), vec!["Windows", "Linux", "macOS / iOS"]);
    }

    #[test]
    fn macros_must_be_whole_words() {
        assert!(platforms_in("int my__linux__var;").is_empty());
        assert_eq!(platforms_in("#if defined(__linux__)"), vec!["Linux"]);
    }

    #[test]
    fn mentions_outside_includes_do_not_count_as_headers() {
        assert!(platforms_in("// see windows.h for details\n").is_empty());
    }

    #[test]
    fn orders_platforms_as_listed() {
        assert!(order_of("Windows") < order_of("POSIX"));
        assert_eq!(order_of("Plan 9"), usize::MAX);
    }
}

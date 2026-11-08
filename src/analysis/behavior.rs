//! What each function does, told by the library functions it calls: it
//! reads input, talks over the network, copies memory, parses numbers,
//! runs programs… So functions can be found by behavior ("reads network
//! input and copies memory") however their code is written.
//!
//! A function has a behavior directly when it makes such a call, and
//! through its calls when a project function it calls (a few calls deep)
//! has it.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use super::capabilities;
use crate::model::{CodeGraph, EdgeKind, SymbolId};

/// Calls followed when a function's behaviors include what it calls.
pub const MAX_CALL_DEPTH: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Behavior {
    ReadsInput,
    Network,
    WritesOutput,
    Files,
    CopiesMemory,
    CopiesStrings,
    FormatsText,
    ParsesText,
    Allocates,
    Frees,
    RunsPrograms,
    LoadsCode,
    Environment,
    Privileges,
    Ipc,
    Threads,
    Signals,
    Randomness,
    Crypto,
}

impl Behavior {
    pub const ALL: [Behavior; 19] = [
        Behavior::ReadsInput,
        Behavior::Network,
        Behavior::WritesOutput,
        Behavior::Files,
        Behavior::CopiesMemory,
        Behavior::CopiesStrings,
        Behavior::FormatsText,
        Behavior::ParsesText,
        Behavior::Allocates,
        Behavior::Frees,
        Behavior::RunsPrograms,
        Behavior::LoadsCode,
        Behavior::Environment,
        Behavior::Privileges,
        Behavior::Ipc,
        Behavior::Threads,
        Behavior::Signals,
        Behavior::Randomness,
        Behavior::Crypto,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Behavior::ReadsInput => "reads input",
            Behavior::Network => "network",
            Behavior::WritesOutput => "writes output",
            Behavior::Files => "opens or changes files",
            Behavior::CopiesMemory => "copies memory",
            Behavior::CopiesStrings => "copies strings",
            Behavior::FormatsText => "formats text",
            Behavior::ParsesText => "parses text or numbers",
            Behavior::Allocates => "allocates",
            Behavior::Frees => "frees",
            Behavior::RunsPrograms => "runs programs",
            Behavior::LoadsCode => "loads code",
            Behavior::Environment => "environment",
            Behavior::Privileges => "changes privileges",
            Behavior::Ipc => "talks to other processes",
            Behavior::Threads => "threads and locks",
            Behavior::Signals => "signals",
            Behavior::Randomness => "randomness",
            Behavior::Crypto => "crypto",
        }
    }
}

const READS_INPUT: &str =
    "read pread pread64 readv preadv fread fgets gets getline getdelim fgetc getc getchar \
     scanf fscanf vscanf vfscanf fgetws recv recvfrom recvmsg recvmmsg SSL_read ReadFile \
     ReadConsoleA ReadConsoleW WSARecv WSARecvFrom BIO_read";
const NETWORK: &str =
    "socket connect accept accept4 bind listen recv recvfrom recvmsg recvmmsg send sendto \
     sendmsg sendmmsg getaddrinfo gethostbyname gethostbyaddr getnameinfo inet_pton \
     inet_ntop SSL_read SSL_write SSL_connect SSL_accept WSARecv WSASend WSARecvFrom \
     WSASendTo curl_easy_perform";
const WRITES_OUTPUT: &str =
    "write pwrite pwrite64 writev fwrite fputs puts fputc putc putchar send sendto \
     sendmsg SSL_write WriteFile BIO_write";
const FILES: &str =
    "open openat open64 creat fopen fopen64 freopen fdopen opendir fdopendir stat lstat \
     fstatat access faccessat unlink unlinkat rename renameat remove mkdir mkdirat rmdir \
     chmod fchmod chown lchown fchown link symlink readlink realpath truncate mkstemp \
     mkdtemp tmpnam tempnam tmpfile CreateFileA CreateFileW DeleteFileA DeleteFileW \
     MoveFileA MoveFileW";
const COPIES_MEMORY: &str =
    "memcpy memmove mempcpy bcopy wmemcpy wmemmove memccpy CopyMemory RtlCopyMemory \
     MoveMemory RtlMoveMemory memcpy_s memmove_s";
const COPIES_STRINGS: &str =
    "strcpy strncpy strcat strncat stpcpy stpncpy strlcpy strlcat wcscpy wcsncpy wcscat \
     wcsncat lstrcpyA lstrcpyW lstrcatA lstrcatW strdup strndup wcsdup strcpy_s strcat_s \
     strncpy_s strncat_s";
const FORMATS_TEXT: &str =
    "sprintf snprintf vsprintf vsnprintf asprintf vasprintf swprintf vswprintf printf \
     vprintf fprintf vfprintf dprintf syslog wsprintfA wsprintfW _snprintf";
const PARSES_TEXT: &str =
    "sscanf vsscanf swscanf strtol strtoul strtoll strtoull strtoimax strtoumax strtod \
     strtof strtold atoi atol atoll atof strtok strtok_r strsep wcstol wcstoul";
const ALLOCATES: &str =
    "malloc calloc realloc reallocarray aligned_alloc posix_memalign memalign valloc \
     pvalloc alloca strdup strndup HeapAlloc VirtualAlloc LocalAlloc GlobalAlloc mmap";
const FREES: &str = "\
    free cfree HeapFree VirtualFree LocalFree GlobalFree munmap";
const RUNS_PROGRAMS: &str =
    "system popen execl execlp execle execv execvp execvpe execve fexecve fork vfork \
     posix_spawn posix_spawnp CreateProcessA CreateProcessW ShellExecuteA ShellExecuteW \
     ShellExecuteExA ShellExecuteExW WinExec _popen _wsystem";
const LOADS_CODE: &str =
    "dlopen dlsym dlmopen LoadLibraryA LoadLibraryW LoadLibraryExA LoadLibraryExW \
     GetProcAddress";
const ENVIRONMENT: &str =
    "getenv secure_getenv setenv putenv unsetenv clearenv GetEnvironmentVariableA \
     GetEnvironmentVariableW SetEnvironmentVariableA SetEnvironmentVariableW";
const PRIVILEGES: &str =
    "setuid setgid seteuid setegid setreuid setregid setresuid setresgid setgroups \
     initgroups chroot capset cap_set_proc prctl AdjustTokenPrivileges \
     ImpersonateLoggedOnUser RevertToSelf";
const THREADS: &str = "\
    pthread_create pthread_join pthread_mutex_lock pthread_mutex_unlock \
    pthread_rwlock_rdlock pthread_rwlock_wrlock pthread_cond_wait pthread_cond_signal \
    CreateThread _beginthreadex EnterCriticalSection WaitForSingleObject thrd_create \
    mtx_lock";
const SIGNALS: &str = "\
    signal sigaction kill raise sigprocmask pthread_kill";
const RANDOMNESS: &str =
    "rand random srand srandom drand48 lrand48 arc4random arc4random_buf getrandom \
     getentropy RAND_bytes BCryptGenRandom CryptGenRandom rand_r";
/// Prefixes of crypto library functions.
const CRYPTO_PREFIXES: &str =
    "EVP_ SSL_CTX_ RSA_ AES_ HMAC SHA1 SHA256 SHA512 MD5 DES_ mbedtls_ wc_ gcry_ crypto_ \
     BCrypt CryptEncrypt CryptDecrypt";

/// What a call to `name` (as written: `std::` and `::` prefixes are fine)
/// says its caller does.
pub fn behaviors_of_call(name: &str) -> Vec<Behavior> {
    let name = name.rsplit("::").next().unwrap_or(name);
    let lists: [(&str, Behavior); 17] = [
        (READS_INPUT, Behavior::ReadsInput),
        (NETWORK, Behavior::Network),
        (WRITES_OUTPUT, Behavior::WritesOutput),
        (FILES, Behavior::Files),
        (COPIES_MEMORY, Behavior::CopiesMemory),
        (COPIES_STRINGS, Behavior::CopiesStrings),
        (FORMATS_TEXT, Behavior::FormatsText),
        (PARSES_TEXT, Behavior::ParsesText),
        (ALLOCATES, Behavior::Allocates),
        (FREES, Behavior::Frees),
        (RUNS_PROGRAMS, Behavior::RunsPrograms),
        (LOADS_CODE, Behavior::LoadsCode),
        (ENVIRONMENT, Behavior::Environment),
        (PRIVILEGES, Behavior::Privileges),
        (THREADS, Behavior::Threads),
        (SIGNALS, Behavior::Signals),
        (RANDOMNESS, Behavior::Randomness),
    ];
    let mut found: Vec<Behavior> = lists
        .iter()
        .filter(|(names, _)| names.split_whitespace().any(|n| n == name))
        .map(|(_, behavior)| *behavior)
        .collect();
    if CRYPTO_PREFIXES
        .split_whitespace()
        .any(|p| name.starts_with(p))
    {
        found.push(Behavior::Crypto);
    }
    if capabilities::classify(name, None).is_some_and(capabilities::Category::is_ipc) {
        found.push(Behavior::Ipc);
    }
    found
}

/// Each function's behaviors, and the library calls that show each, from
/// the calls it makes itself.
pub type DirectBehaviors = HashMap<SymbolId, BTreeMap<Behavior, Vec<String>>>;

/// The behaviors of every function in `graph`, from the calls each makes.
pub fn direct_behaviors(graph: &CodeGraph) -> DirectBehaviors {
    let mut found: DirectBehaviors = HashMap::new();
    for call in graph.external_calls.values().flatten() {
        for behavior in behaviors_of_call(&call.name) {
            let calls = found
                .entry(call.caller)
                .or_default()
                .entry(behavior)
                .or_default();
            if !calls.contains(&call.name) {
                calls.push(call.name.clone());
            }
        }
    }
    found
}

/// For each behavior, the functions that have it within `MAX_CALL_DEPTH`
/// calls: itself, or a project function it calls (or that one calls…).
pub fn reached_behaviors(
    graph: &CodeGraph,
    direct: &DirectBehaviors,
) -> HashMap<Behavior, HashSet<SymbolId>> {
    let mut callers: HashMap<SymbolId, Vec<SymbolId>> = HashMap::new();
    for edge in graph.edges.iter().filter(|e| e.kind == EdgeKind::Calls) {
        callers.entry(edge.to).or_default().push(edge.from);
    }
    Behavior::ALL
        .into_iter()
        .map(|behavior| {
            let having = direct
                .iter()
                .filter(|(_, behaviors)| behaviors.contains_key(&behavior))
                .map(|(id, _)| *id);
            (behavior, callers_within(&callers, having))
        })
        .collect()
}

/// `start` and every function calling one of them within `MAX_CALL_DEPTH`
/// calls.
fn callers_within(
    callers: &HashMap<SymbolId, Vec<SymbolId>>,
    start: impl Iterator<Item = SymbolId>,
) -> HashSet<SymbolId> {
    let mut seen: HashSet<SymbolId> = HashSet::new();
    let mut queue: VecDeque<(SymbolId, usize)> = VecDeque::new();
    for id in start {
        if seen.insert(id) {
            queue.push_back((id, 0));
        }
    }
    while let Some((id, depth)) = queue.pop_front() {
        if depth == MAX_CALL_DEPTH {
            continue;
        }
        for &caller in callers.get(&id).map_or(&[][..], Vec::as_slice) {
            if seen.insert(caller) {
                queue.push_back((caller, depth + 1));
            }
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::graph_of_files;

    fn id(graph: &CodeGraph, name: &str) -> SymbolId {
        graph
            .symbols
            .values()
            .find(|s| s.name == name)
            .map(|s| s.id)
            .unwrap_or_else(|| panic!("no {name}"))
    }

    const SERVER: &str = "\
    void copy_in(char *d, const char *s, int n) { memcpy(d, s, n); }\n void handle(int fd) \
    {\n char buf[64], out[64];\n int n = recv(fd, buf, sizeof buf, 0);\n copy_in(out, buf, \
    n);\n }\n void serve(void) { handle(socket(2, 1, 0)); }\n void far(void) { serve(); }\n \
    void farther(void) { far(); }\n";

    #[test]
    fn a_call_can_say_more_than_one_thing() {
        let recv = behaviors_of_call("recv");
        assert!(recv.contains(&Behavior::ReadsInput));
        assert!(recv.contains(&Behavior::Network));
        assert_eq!(behaviors_of_call("std::memcpy"), [Behavior::CopiesMemory]);
        assert_eq!(behaviors_of_call("EVP_EncryptUpdate"), [Behavior::Crypto]);
        assert!(behaviors_of_call("pipe").contains(&Behavior::Ipc));
        assert!(behaviors_of_call("parse_header").is_empty());
    }

    #[test]
    fn functions_have_the_behaviors_of_their_own_calls() {
        let graph = graph_of_files(&[("s.c", SERVER)]);
        let direct = direct_behaviors(&graph);
        let handle = &direct[&id(&graph, "handle")];
        assert_eq!(handle[&Behavior::ReadsInput], ["recv"]);
        assert!(
            !handle.contains_key(&Behavior::CopiesMemory),
            "its callee's, not its own"
        );
        assert!(direct[&id(&graph, "copy_in")].contains_key(&Behavior::CopiesMemory));
    }

    #[test]
    fn through_calls_a_few_deep_a_function_has_its_callees_behaviors() {
        let graph = graph_of_files(&[("s.c", SERVER)]);
        let reached = reached_behaviors(&graph, &direct_behaviors(&graph));
        let copies = &reached[&Behavior::CopiesMemory];
        for name in ["copy_in", "handle", "serve", "far"] {
            assert!(copies.contains(&id(&graph, name)), "{name}");
        }
        assert!(
            !copies.contains(&id(&graph, "farther")),
            "past {MAX_CALL_DEPTH} calls"
        );
    }
}

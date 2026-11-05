//! What a program can do, read from the library functions it calls: which
//! kinds of IPC (pipes, sockets, shared memory, message queues, D-Bus, …)
//! it uses, and related reach such as starting processes or loading code.
//! Works on source calls (with their arguments) and on binary imports.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use crate::model::{CodeGraph, SymbolId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Pipes,
    UnixSockets,
    Netlink,
    NetworkSockets,
    Sockets,
    SharedMemory,
    MessageQueues,
    Semaphores,
    Signals,
    DBus,
    MachXpc,
    WindowMessages,
    Com,
    Rpc,
    DeviceIo,
    MessagingLibraries,
    NetworkLibraries,
    ProcessCreation,
    DynamicLoading,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Pipes => "Pipes, FIFOs & mailslots",
            Category::UnixSockets => "Unix domain sockets",
            Category::Netlink => "Netlink sockets",
            Category::NetworkSockets => "Network sockets & name lookup",
            Category::Sockets => "Socket I/O (any family)",
            Category::SharedMemory => "Shared memory",
            Category::MessageQueues => "Message queues",
            Category::Semaphores => "Named semaphores, mutexes & events",
            Category::Signals => "Signals",
            Category::DBus => "D-Bus",
            Category::MachXpc => "Mach ports & XPC",
            Category::WindowMessages => "Window messages",
            Category::Com => "COM / DCOM",
            Category::Rpc => "RPC (MS-RPC, ALPC, Sun RPC)",
            Category::DeviceIo => "Device & driver I/O",
            Category::MessagingLibraries => "Messaging & RPC libraries",
            Category::NetworkLibraries => "HTTP & TLS libraries",
            Category::ProcessCreation => "Process creation",
            Category::DynamicLoading => "Dynamic code loading",
        }
    }

    /// Whether this is a way for processes to talk to each other (as
    /// opposed to related reach, like starting processes).
    pub fn is_ipc(self) -> bool {
        !matches!(
            self,
            Category::ProcessCreation | Category::DynamicLoading | Category::NetworkLibraries
        )
    }
}

/// Functions by exact name. Windows `…A`/`…W` variants are listed once,
/// without the suffix.
const NAMES: &[(Category, &[&str])] = &[
    (
        Category::Pipes,
        &[
            "pipe",
            "pipe2",
            "popen",
            "_popen",
            "_pipe",
            "mkfifo",
            "mkfifoat",
            "CreatePipe",
            "CreateNamedPipe",
            "ConnectNamedPipe",
            "CallNamedPipe",
            "PeekNamedPipe",
            "TransactNamedPipe",
            "WaitNamedPipe",
            "DisconnectNamedPipe",
            "ImpersonateNamedPipeClient",
            "CreateMailslot",
        ],
    ),
    (Category::UnixSockets, &["socketpair"]),
    (
        Category::NetworkSockets,
        &[
            "getaddrinfo",
            "gethostbyname",
            "gethostbyname2",
            "gethostbyname_r",
            "gethostbyaddr",
            "getnameinfo",
            "inet_pton",
            "inet_ntop",
            "inet_addr",
            "inet_aton",
            "inet_ntoa",
            "WSAStartup",
            "WSASocket",
            "WSAConnect",
            "WSAAccept",
            "WSARecv",
            "WSASend",
            "WSARecvFrom",
            "WSASendTo",
            "WSAIoctl",
            "closesocket",
            "ioctlsocket",
        ],
    ),
    (
        Category::Sockets,
        &[
            "socket",
            "bind",
            "listen",
            "accept",
            "accept4",
            "connect",
            "send",
            "recv",
            "sendto",
            "recvfrom",
            "sendmsg",
            "recvmsg",
            "sendmmsg",
            "recvmmsg",
            "shutdown",
            "setsockopt",
            "getsockopt",
            "getsockname",
            "getpeername",
        ],
    ),
    (
        Category::SharedMemory,
        &[
            "shm_open",
            "shm_unlink",
            "shmget",
            "shmat",
            "shmdt",
            "shmctl",
            "memfd_create",
            "CreateFileMapping",
            "OpenFileMapping",
            "MapViewOfFile",
            "MapViewOfFileEx",
            "MapViewOfFile3",
            "ashmem_create_region",
            "ASharedMemory_create",
        ],
    ),
    (
        Category::MessageQueues,
        &[
            "mq_open",
            "mq_close",
            "mq_unlink",
            "mq_send",
            "mq_receive",
            "mq_timedsend",
            "mq_timedreceive",
            "mq_notify",
            "mq_getattr",
            "mq_setattr",
            "msgget",
            "msgsnd",
            "msgrcv",
            "msgctl",
        ],
    ),
    (
        Category::Semaphores,
        &[
            "sem_open",
            "sem_unlink",
            "semget",
            "semop",
            "semtimedop",
            "semctl",
            "CreateSemaphore",
            "OpenSemaphore",
            "CreateMutex",
            "OpenMutex",
            "CreateEvent",
            "OpenEvent",
        ],
    ),
    (
        Category::Signals,
        &[
            "kill",
            "killpg",
            "sigqueue",
            "tgkill",
            "tkill",
            "pthread_kill",
            "signal",
            "sigaction",
            "bsd_signal",
            "sigwait",
            "sigwaitinfo",
            "sigtimedwait",
            "signalfd",
        ],
    ),
    (
        Category::MachXpc,
        &[
            "mach_msg",
            "mach_msg_overwrite",
            "mach_port_allocate",
            "mach_port_insert_right",
            "bootstrap_look_up",
            "bootstrap_check_in",
            "bootstrap_register",
            "task_for_pid",
        ],
    ),
    (
        Category::WindowMessages,
        &[
            "SendMessage",
            "PostMessage",
            "PostThreadMessage",
            "SendMessageTimeout",
            "SendNotifyMessage",
            "BroadcastSystemMessage",
            "RegisterWindowMessage",
        ],
    ),
    (
        Category::Com,
        &[
            "CoCreateInstance",
            "CoCreateInstanceEx",
            "CoGetClassObject",
            "CoInitialize",
            "CoInitializeEx",
            "CoInitializeSecurity",
            "CoRegisterClassObject",
            "CoMarshalInterface",
            "CoUnmarshalInterface",
            "OleInitialize",
        ],
    ),
    (
        Category::Rpc,
        &[
            "RpcServerListen",
            "RpcServerRegisterIf",
            "RpcServerRegisterIf2",
            "RpcServerRegisterIfEx",
            "RpcServerUseProtseqEp",
            "RpcStringBindingCompose",
            "RpcBindingFromStringBinding",
            "NdrClientCall",
            "NdrClientCall2",
            "NdrClientCall3",
            "NdrServerCall2",
            "NtCreatePort",
            "NtConnectPort",
            "NtRequestWaitReplyPort",
            "clnt_create",
            "clnt_call",
            "svc_register",
            "svc_run",
            "svcudp_create",
            "svctcp_create",
        ],
    ),
    (
        Category::DeviceIo,
        &["ioctl", "DeviceIoControl", "NtDeviceIoControlFile"],
    ),
    (
        Category::NetworkLibraries,
        &[
            "InternetOpen",
            "InternetOpenUrl",
            "HttpSendRequest",
            "URLDownloadToFile",
        ],
    ),
    (
        Category::ProcessCreation,
        &[
            "fork",
            "vfork",
            "execl",
            "execle",
            "execlp",
            "execv",
            "execve",
            "execvp",
            "execvpe",
            "fexecve",
            "posix_spawn",
            "posix_spawnp",
            "system",
            "CreateProcess",
            "CreateProcessAsUser",
            "CreateProcessWithLogon",
            "CreateProcessWithToken",
            "ShellExecute",
            "ShellExecuteEx",
            "WinExec",
        ],
    ),
    (
        Category::DynamicLoading,
        &[
            "dlopen",
            "dlmopen",
            "dlsym",
            "dlvsym",
            "LoadLibrary",
            "LoadLibraryEx",
            "GetProcAddress",
            "LdrLoadDll",
        ],
    ),
];

/// Library APIs recognized by their name prefix.
const PREFIXES: &[(Category, &[&str])] = &[
    (Category::DBus, &["dbus_", "g_dbus_", "sd_bus_"]),
    (Category::MachXpc, &["xpc_", "CFMessagePort"]),
    (Category::Rpc, &["NtAlpc", "ZwAlpc"]),
    (
        Category::MessagingLibraries,
        &[
            "zmq_",
            "nng_",
            "grpc_",
            "mosquitto_",
            "amqp_",
            "rd_kafka_",
            "MQTTClient_",
            "MQTTAsync_",
        ],
    ),
    (
        Category::NetworkLibraries,
        &[
            "curl_",
            "lws_",
            "SSL_",
            "mbedtls_ssl_",
            "gnutls_",
            "WinHttp",
        ],
    ),
];

const UNIX_FAMILIES: &[&str] = &["AF_UNIX", "AF_LOCAL", "PF_UNIX", "PF_LOCAL"];
const NETLINK_FAMILIES: &[&str] = &["AF_NETLINK", "PF_NETLINK"];
const NETWORK_FAMILIES: &[&str] = &["AF_INET", "PF_INET", "AF_PACKET", "AF_BLUETOOTH"];

fn by_name() -> &'static HashMap<&'static str, Category> {
    static TABLE: OnceLock<HashMap<&'static str, Category>> = OnceLock::new();
    TABLE.get_or_init(|| {
        NAMES
            .iter()
            .flat_map(|(category, names)| names.iter().map(move |name| (*name, *category)))
            .collect()
    })
}

/// The category of a call to `name` (as written: `std::`/`::` prefixes are
/// fine), using its arguments when known to tell socket families apart and
/// shared from private `mmap`s.
pub fn classify(name: &str, args: Option<&[String]>) -> Option<Category> {
    let name = name.rsplit("::").next().unwrap_or(name);
    match name {
        "socket" | "socketpair" => return Some(socket_category(name, args)),
        "mmap" | "mmap64" => {
            let flags = args.and_then(|a| a.get(3))?;
            return flags
                .contains("MAP_SHARED")
                .then_some(Category::SharedMemory);
        }
        _ => {}
    }
    let table = by_name();
    table
        .get(name)
        .or_else(|| windows_base_name(name).and_then(|base| table.get(base)))
        .copied()
        .or_else(|| {
            PREFIXES
                .iter()
                .find(|(_, prefixes)| prefixes.iter().any(|p| name.starts_with(p)))
                .map(|(category, _)| *category)
        })
}

fn socket_category(name: &str, args: Option<&[String]>) -> Category {
    let family = args.and_then(|a| a.first()).map_or("", String::as_str);
    let has = |families: &[&str]| families.iter().any(|f| family.contains(f));
    // socketpair() only makes local (Unix domain) pairs in practice.
    if has(UNIX_FAMILIES) || name == "socketpair" {
        Category::UnixSockets
    } else if has(NETLINK_FAMILIES) {
        Category::Netlink
    } else if has(NETWORK_FAMILIES) {
        Category::NetworkSockets
    } else {
        Category::Sockets
    }
}

/// `CreateNamedPipeW` → `CreateNamedPipe`: Win32 ANSI/Unicode variants.
fn windows_base_name(name: &str) -> Option<&str> {
    let base = name.strip_suffix('A').or_else(|| name.strip_suffix('W'))?;
    base.chars()
        .last()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .then_some(base)
}

/// One call in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPlace {
    pub file: String,
    pub line: u32,
    pub caller: SymbolId,
}

/// Every call to one function within a category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionUse {
    pub name: String,
    pub calls: Vec<CallPlace>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityUse {
    pub category: Category,
    /// Sorted by name.
    pub functions: Vec<FunctionUse>,
}

impl CapabilityUse {
    pub fn call_count(&self) -> usize {
        self.functions.iter().map(|f| f.calls.len()).sum()
    }
}

/// The categories the project's library calls fall in, in category order.
pub fn capabilities_in(graph: &CodeGraph) -> Vec<CapabilityUse> {
    let mut grouped: BTreeMap<Category, BTreeMap<String, Vec<CallPlace>>> = BTreeMap::new();
    let mut files: Vec<&String> = graph.external_calls.keys().collect();
    files.sort();
    for file in files {
        for call in &graph.external_calls[file] {
            let Some(category) = classify(&call.name, Some(&call.args)) else {
                continue;
            };
            let name = call.name.rsplit("::").next().unwrap_or(&call.name);
            grouped
                .entry(category)
                .or_default()
                .entry(name.to_string())
                .or_default()
                .push(CallPlace {
                    file: file.clone(),
                    line: call.span.line,
                    caller: call.caller,
                });
        }
    }
    grouped
        .into_iter()
        .map(|(category, functions)| CapabilityUse {
            category,
            functions: functions
                .into_iter()
                .map(|(name, calls)| FunctionUse { name, calls })
                .collect(),
        })
        .collect()
}

/// Imported function names grouped by category (for binaries), each list
/// sorted and without duplicates.
pub fn classify_imports<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> Vec<(Category, Vec<String>)> {
    let mut grouped: BTreeMap<Category, Vec<String>> = BTreeMap::new();
    for name in names {
        if let Some(category) = classify(name, None) {
            grouped.entry(category).or_default().push(name.to_string());
        }
    }
    grouped
        .into_iter()
        .map(|(category, mut names)| {
            names.sort();
            names.dedup();
            (category, names)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ExternalCall, Span};

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn classifies_by_name_and_namespace() {
        assert_eq!(classify("pipe", None), Some(Category::Pipes));
        assert_eq!(classify("::shm_open", None), Some(Category::SharedMemory));
        assert_eq!(
            classify("std::system", None),
            Some(Category::ProcessCreation)
        );
        assert_eq!(classify("strcpy", None), None);
    }

    #[test]
    fn classifies_windows_ansi_and_unicode_variants() {
        assert_eq!(classify("CreateNamedPipeW", None), Some(Category::Pipes));
        assert_eq!(classify("CreateNamedPipeA", None), Some(Category::Pipes));
        assert_eq!(
            classify("CreateProcessWithLogonW", None),
            Some(Category::ProcessCreation)
        );
        // An uppercase letter before the A/W is part of the name.
        assert_eq!(classify("WSA", None), None);
    }

    #[test]
    fn classifies_library_apis_by_prefix() {
        assert_eq!(classify("dbus_connection_send", None), Some(Category::DBus));
        assert_eq!(
            classify("xpc_connection_create", None),
            Some(Category::MachXpc)
        );
        assert_eq!(
            classify("zmq_send", None),
            Some(Category::MessagingLibraries)
        );
        assert_eq!(
            classify("NtAlpcSendWaitReceivePort", None),
            Some(Category::Rpc)
        );
    }

    #[test]
    fn tells_socket_families_apart_by_argument() {
        let socket = |family: &str| classify("socket", Some(&args(&[family, "SOCK_STREAM", "0"])));
        assert_eq!(socket("AF_UNIX"), Some(Category::UnixSockets));
        assert_eq!(socket("PF_LOCAL"), Some(Category::UnixSockets));
        assert_eq!(socket("AF_NETLINK"), Some(Category::Netlink));
        assert_eq!(socket("AF_INET6"), Some(Category::NetworkSockets));
        assert_eq!(socket("family"), Some(Category::Sockets));
        assert_eq!(classify("socket", None), Some(Category::Sockets));
        assert_eq!(classify("socketpair", None), Some(Category::UnixSockets));
    }

    #[test]
    fn only_shared_mappings_count_as_shared_memory() {
        let mmap = |flags: &str| classify("mmap", Some(&args(&["0", "n", "PROT_READ", flags])));
        assert_eq!(mmap("MAP_SHARED"), Some(Category::SharedMemory));
        assert_eq!(mmap("MAP_PRIVATE | MAP_ANONYMOUS"), None);
        assert_eq!(classify("mmap", None), None);
    }

    #[test]
    fn groups_a_projects_calls_by_category_and_function() {
        let call = |name: &str, line: u32, call_args: &[&str]| ExternalCall {
            caller: 1,
            name: name.into(),
            span: Span {
                line,
                start: 0,
                end: 1,
            },
            extent: Span {
                line,
                start: 0,
                end: 1,
            },
            args: args(call_args),
        };
        let mut graph = CodeGraph::default();
        graph.external_calls.insert(
            "ipc.c".into(),
            vec![
                call("pipe", 3, &["fds"]),
                call("socket", 4, &["AF_UNIX", "SOCK_STREAM", "0"]),
                call("pipe", 9, &["more"]),
                call("strlen", 10, &["s"]),
            ],
        );
        let uses = capabilities_in(&graph);
        let summary: Vec<(Category, Vec<(&str, usize)>)> = uses
            .iter()
            .map(|u| {
                let functions = u.functions.iter().map(|f| (f.name.as_str(), f.calls.len()));
                (u.category, functions.collect())
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (Category::Pipes, vec![("pipe", 2)]),
                (Category::UnixSockets, vec![("socket", 1)]),
            ]
        );
        assert_eq!(uses[0].call_count(), 2);
        assert_eq!(uses[0].functions[0].calls[1].line, 9);
    }

    #[test]
    fn finds_shared_mappings_written_with_a_global_qualifier() {
        use crate::index::{build_graph, Extractor, SourceFile};
        let source = "class Env {\n  void* Map(int fd, long n) {\n    void* base =\n        ::mmap(/*addr=*/nullptr, n, PROT_READ, MAP_SHARED, fd, 0);\n    return base;\n  }\n};\n";
        let facts = Extractor::new()
            .expect("grammar")
            .extract(source)
            .expect("parse");
        let graph = build_graph(vec![SourceFile {
            path: "env.cc".into(),
            contents: source.into(),
            facts,
        }]);
        let uses = capabilities_in(&graph);
        assert_eq!(uses.len(), 1, "{:?}", graph.external_calls);
        assert_eq!(uses[0].category, Category::SharedMemory);
    }

    #[test]
    fn groups_binary_imports_without_duplicates() {
        let grouped = classify_imports(["connect", "socket", "connect", "dlopen", "malloc"]);
        assert_eq!(
            grouped,
            vec![
                (
                    Category::Sockets,
                    vec!["connect".to_string(), "socket".to_string()]
                ),
                (Category::DynamicLoading, vec!["dlopen".to_string()]),
            ]
        );
        assert!(Category::Pipes.is_ipc() && !Category::DynamicLoading.is_ipc());
    }
}

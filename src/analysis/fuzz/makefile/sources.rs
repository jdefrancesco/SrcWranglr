//! What a fuzzer for one function needs built: the source files defining
//! the functions it calls (all the way down), and the folders `-I` must
//! name for their `#include`s to be found.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use crate::analysis::includes::{folder_of, included, normalized, shared_folders, Resolver};
use crate::model::{CodeGraph, EdgeKind, FunctionMetrics, SymbolId};

/// Source files listed at most (nearest first, by calls).
pub const MAX_SOURCES: usize = 300;
/// Functions followed at most.
const MAX_FUNCTIONS: usize = 50_000;
/// A name defined in more files than this is taken for a macro written
/// like a function, and not followed.
const MAX_DEFINITIONS: usize = 2;
/// Files of only globals added at most.
const MAX_DATA_FILES: usize = 20;
/// Files whose `#include`s are read at most.
const MAX_SCANNED: usize = 2_000;
/// Extensions of files compiled on their own.
const SOURCE_EXTENSIONS: &[&str] = &["c", "cc", "cpp", "cxx", "c++"];

/// What to build.
pub struct Needed {
    /// Source files, sorted.
    pub sources: Vec<String>,
    /// Files needed whose paths can't be written into a Makefile safely.
    pub unwritable: Vec<String>,
    /// Files left out because they are for another platform.
    pub other_platform: Vec<String>,
    /// Functions (not `main`) defined in two of the sources: name, files.
    pub clashes: Vec<(String, String, String)>,
    /// Whether more than `MAX_SOURCES` were needed.
    pub is_cut: bool,
}

/// The files to build for `function`, leaving out its own file when the
/// harness `#include`s it.
pub fn needed<'a>(
    graph: &'a CodeGraph,
    function: &'a FunctionMetrics,
    is_own_file_included: bool,
) -> Needed {
    let index = Index::new(graph);
    let (mut files, other_platform) = reached_files(&index, function);
    let data = data_only_beside(graph, &index, &files);
    files.extend(data);
    if is_own_file_included {
        files.retain(|f| *f != function.file);
    }
    let is_cut = files.len() > MAX_SOURCES;
    files.truncate(MAX_SOURCES);
    let (mut sources, mut unwritable): (Vec<String>, Vec<String>) = files
        .into_iter()
        .map(str::to_string)
        .partition(|f| is_make_safe(f));
    sources.sort();
    unwritable.sort();
    let clashes = clashes(graph, &index, &sources);
    Needed {
        sources,
        unwritable,
        other_platform: other_platform.into_iter().map(str::to_string).collect(),
        clashes,
        is_cut,
    }
}

/// The project's functions and calls, indexed once for the walk.
struct Index<'a> {
    /// The files defining each function: more than one for a version per
    /// architecture, or for a macro written like a function.
    files_of: HashMap<SymbolId, Vec<&'a str>>,
    /// The functions each file defines.
    in_file: HashMap<&'a str, Vec<SymbolId>>,
    /// What each function calls or refers to (`.read = parse_x`).
    next: HashMap<SymbolId, Vec<SymbolId>>,
}

impl<'a> Index<'a> {
    fn new(graph: &'a CodeGraph) -> Self {
        let mut files_of: HashMap<SymbolId, Vec<&str>> = HashMap::new();
        let mut in_file: HashMap<&str, Vec<SymbolId>> = HashMap::new();
        for f in &graph.functions {
            let files = files_of.entry(f.symbol).or_default();
            if !files.contains(&f.file.as_str()) {
                files.push(f.file.as_str());
            }
            in_file.entry(f.file.as_str()).or_default().push(f.symbol);
        }
        let mut next: HashMap<SymbolId, Vec<SymbolId>> = HashMap::new();
        for edge in &graph.edges {
            if matches!(edge.kind, EdgeKind::Calls | EdgeKind::Uses) {
                next.entry(edge.from).or_default().push(edge.to);
            }
        }
        Self {
            files_of,
            in_file,
            next,
        }
    }

    /// Whether `id` is "defined" in more than `MAX_DEFINITIONS` files: a
    /// macro written like a function (`CMD_FUNC(cmd_join) { … }`), not
    /// one function. Following it would tie every file using the macro
    /// together. Versions under `#if` in one file count once.
    fn is_macro_made(&self, id: &SymbolId) -> bool {
        self.files_of
            .get(id)
            .is_some_and(|files| files.len() > MAX_DEFINITIONS)
    }

    /// The file defining `id`: of several (a version per architecture),
    /// the one sharing the most folders with `near`, then the shortest.
    fn file_of(&self, id: &SymbolId, near: &str) -> Option<&'a str> {
        self.files_of
            .get(id)?
            .iter()
            .copied()
            .max_by_key(|file| rank(file, near))
    }
}

/// The source files `function` needs linked, nearest first: the files
/// defining the functions it calls or refers to, and what every function
/// in those files calls in turn, since a file is linked whole. Also the
/// files left out for another platform.
fn reached_files<'a>(
    index: &Index<'a>,
    function: &'a FunctionMetrics,
) -> (Vec<&'a str>, Vec<&'a str>) {
    let (mut files, mut other_platform) = (Vec::new(), Vec::new());
    let mut seen_files: HashSet<&str> = HashSet::new();
    let mut seen = HashSet::from([function.symbol]);
    let mut queue = VecDeque::from([function.symbol]);
    while let Some(id) = queue.pop_front() {
        let file = match id == function.symbol {
            true => Some(function.file.as_str()),
            false => index.file_of(&id, &function.file),
        };
        let file = file.filter(|f| is_source(f));
        if let Some(file) = file.filter(|f| is_for_other_platform(f)) {
            if seen_files.insert(file) {
                other_platform.push(file);
            }
            continue;
        }
        let whole_file = match file.filter(|f| seen_files.insert(f)) {
            Some(file) => {
                files.push(file);
                index
                    .in_file
                    .get(file)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
            }
            None => &[],
        };
        let calls = index.next.get(&id).map(Vec::as_slice).unwrap_or_default();
        for to in calls.iter().chain(whole_file) {
            if !index.is_macro_made(to) && seen.len() < MAX_FUNCTIONS && seen.insert(*to) {
                queue.push_back(*to);
            }
        }
    }
    (files, other_platform)
}

/// Files of only globals (like `ircd_vars.c`) in the folders of `files`:
/// nothing calls into them, but the code may use their variables, which
/// the index doesn't follow. Only files defining data (`int loop = 0;`,
/// `x[] = {…}`) that no listed file `#include`s (an X-macro list), and at
/// most `MAX_DATA_FILES`.
fn data_only_beside<'a>(graph: &'a CodeGraph, index: &Index, files: &[&str]) -> Vec<&'a str> {
    let folders: HashSet<&str> = files.iter().map(|f| folder_of(f)).collect();
    let listed: HashSet<&str> = files.iter().copied().collect();
    let included_names: HashSet<&str> = files
        .iter()
        .filter_map(|f| graph.file_contents.get(*f))
        .flat_map(|text| text.lines().filter_map(included))
        .map(|name| name.rsplit('/').next().unwrap_or(name))
        .collect();
    let mut data: Vec<&str> = graph
        .file_contents
        .iter()
        .filter(|(f, _)| is_source(f) && !is_for_other_platform(f))
        .filter(|(f, _)| folders.contains(folder_of(f)) && !listed.contains(f.as_str()))
        .filter(|(f, _)| !index.in_file.contains_key(f.as_str()))
        .filter(|(f, _)| !included_names.contains(f.rsplit('/').next().unwrap_or(f)))
        .filter(|(_, text)| defines_data(text))
        .map(|(f, _)| f.as_str())
        .collect();
    data.sort_unstable();
    data.truncate(MAX_DATA_FILES);
    data
}

/// Whether `text` looks like it defines a variable: a line (not a
/// preprocessor line or comment) with an `=` or a `[`.
fn defines_data(text: &str) -> bool {
    text.lines()
        .map(str::trim_start)
        .any(|line| !line.starts_with('#') && !line.starts_with("//") && line.contains(['=', '[']))
}

/// Whether `path` is in a folder for another platform than the ones
/// AFL++ runs on (`src/windows/gui.c`).
pub fn is_for_other_platform(path: &str) -> bool {
    path.split('/').any(|segment| {
        matches!(
            segment.to_ascii_lowercase().as_str(),
            "windows" | "win32" | "win64" | "msvc"
        )
    })
}

/// C functions other than `main` (which is renamed) defined, not
/// `static`, in two of `sources`: the link would find both. C++ files
/// aren't checked: two functions of one name there are overloads.
fn clashes(graph: &CodeGraph, index: &Index, sources: &[String]) -> Vec<(String, String, String)> {
    let listed: HashSet<&str> = sources
        .iter()
        .map(String::as_str)
        .filter(|f| f.ends_with(".c"))
        .collect();
    let mut files_of: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for f in &graph.functions {
        if f.signature.is_static
            || index.is_macro_made(&f.symbol)
            || !listed.contains(f.file.as_str())
        {
            continue;
        }
        let Some(symbol) = graph.symbols.get(&f.symbol) else {
            continue;
        };
        if symbol.qualified_name != "main" {
            files_of
                .entry(symbol.qualified_name.as_str())
                .or_default()
                .insert(f.file.as_str());
        }
    }
    files_of
        .into_iter()
        .filter_map(|(name, files)| {
            let mut files = files.into_iter();
            let (first, second) = (files.next()?, files.next()?);
            Some((name.to_string(), first.to_string(), second.to_string()))
        })
        .collect()
}

/// How near `file` is to `to`: folders shared, then a shorter path, then
/// the first by name.
fn rank<'a>(file: &'a str, to: &str) -> (usize, Reverse<usize>, Reverse<&'a str>) {
    (shared_folders(file, to), Reverse(file.len()), Reverse(file))
}

pub fn is_source(file: &str) -> bool {
    file.rsplit_once('.').is_some_and(|(_, extension)| {
        SOURCE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
    })
}

/// Whether `path` can go into a Makefile as is: a relative path inside
/// the project, of letters, digits and `/._-+` only (no spaces, `$`, `%`,
/// `#`, `:` or `\` that make would read as syntax).
pub fn is_make_safe(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && path.split('/').all(|segment| segment != "..")
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
}

/// The folders (other than the project's top) `-I` must name.
pub struct Folders {
    pub usable: Vec<String>,
    /// Those that can't be written into a Makefile safely.
    pub unwritable: Vec<String>,
    /// Those left out because they are for another platform.
    pub other_platform: Vec<String>,
}

/// The folders `-I` must name for the `#include`s of `files`, and of the
/// headers they include, to be found.
pub fn include_folders(graph: &CodeGraph, files: &[&str]) -> Folders {
    let resolver = Resolver::new(graph);
    let mut queue: VecDeque<String> = files.iter().map(|f| f.to_string()).collect();
    let mut seen: HashSet<String> = queue.iter().cloned().collect();
    let mut folders = BTreeSet::new();
    let mut scanned = 0;
    while let Some(file) = queue.pop_front() {
        if scanned >= MAX_SCANNED {
            break;
        }
        scanned += 1;
        let Some(text) = graph.file_contents.get(&file) else {
            continue;
        };
        for name in text.lines().filter_map(included) {
            let Some(found) = resolver.resolve(&file, name) else {
                continue;
            };
            folders.extend(search_folder(&file, name, &found));
            if seen.insert(found.clone()) {
                queue.push_back(found);
            }
        }
    }
    let (other_platform, folders): (Vec<String>, Vec<String>) =
        folders.into_iter().partition(|f| is_for_other_platform(f));
    let (usable, unwritable) = folders.into_iter().partition(|f| is_make_safe(f));
    Folders {
        usable,
        unwritable,
        other_platform,
    }
}

/// The folder `-I` must name for `name`, written in `from`, to be
/// `found`; `None` when it is found beside `from`, or at the top.
fn search_folder(from: &str, name: &str, found: &str) -> Option<String> {
    let name = name.trim_start_matches("./");
    let beside = match folder_of(from) {
        "" => normalized(name),
        folder => normalized(&format!("{folder}/{name}")),
    };
    if beside.as_deref() == Some(found) {
        return None;
    }
    let name = normalized(name)?;
    let folder = found.strip_suffix(&name)?.strip_suffix('/')?;
    (!folder.is_empty()).then(|| folder.to_string())
}

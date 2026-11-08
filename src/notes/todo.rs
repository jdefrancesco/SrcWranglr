//! The project's TODO list, in `<project>/.srcwranglr/todo.json` next to
//! its notes. A TODO is a line of text, done or not, and may point at a
//! line of a file.
//!
//! The file sits in a source tree that may be untrusted, so loading checks
//! everything and neither loading nor saving follows symlinks.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::store::{check_target, NOTES_DIR};
use super::NoteTarget;
use crate::safe_fs::{read_regular_file, write_atomically};

pub const TODO_FILE: &str = "todo.json";
const FORMAT_VERSION: u32 = 1;
/// The longest TODO accepted, in bytes.
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_TODOS: usize = 2000;
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    pub text: String,
    #[serde(default, rename = "done")]
    pub is_done: bool,
    /// The line it is about, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

impl Todo {
    pub fn new(text: &str, place: Option<(String, u32)>) -> Self {
        let (file, line) = place.map_or((None, None), |(f, l)| (Some(f), Some(l)));
        Self {
            text: text.trim().to_string(),
            is_done: false,
            file,
            line,
        }
    }

    /// The line it points at.
    pub fn place(&self) -> Option<(&str, u32)> {
        self.file.as_deref().zip(self.line)
    }
}

/// The list, in the order written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Todos {
    items: Vec<Todo>,
}

impl Todos {
    pub fn items(&self) -> &[Todo] {
        &self.items
    }

    pub fn open_count(&self) -> usize {
        self.items.iter().filter(|t| !t.is_done).count()
    }

    /// The list with `todo` added at the end.
    pub fn with_added(&self, todo: Todo) -> Result<Self, String> {
        check_todo(&todo)?;
        if self.items.len() >= MAX_TODOS {
            return Err(format!("{MAX_TODOS} TODOs is the most; clear some first"));
        }
        let items = self.items.iter().cloned().chain([todo]).collect();
        Ok(Self { items })
    }

    /// The list with `todos` added at the end, if it can hold them all.
    pub fn with_all_added(&self, todos: Vec<Todo>) -> Result<Self, String> {
        todos.iter().try_for_each(check_todo)?;
        if self.items.len() + todos.len() > MAX_TODOS {
            return Err(format!("{MAX_TODOS} TODOs is the most; clear some first"));
        }
        let items = self.items.iter().cloned().chain(todos).collect();
        Ok(Self { items })
    }

    /// The list with TODO `index` done (or not).
    pub fn with_done(&self, index: usize, is_done: bool) -> Self {
        self.with_changed(index, |todo| Todo { is_done, ..todo })
    }

    /// The list with TODO `index` saying `text`.
    pub fn with_text(&self, index: usize, text: &str) -> Result<Self, String> {
        let text = text.trim().to_string();
        let changed = self.with_changed(index, |todo| Todo {
            text: text.clone(),
            ..todo
        });
        changed.items.iter().try_for_each(check_todo)?;
        Ok(changed)
    }

    /// The list without TODO `index`.
    pub fn without(&self, index: usize) -> Self {
        let items = self
            .items
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, t)| t.clone())
            .collect();
        Self { items }
    }

    /// The list without the ones done.
    pub fn without_done(&self) -> Self {
        let items = self.items.iter().filter(|t| !t.is_done).cloned().collect();
        Self { items }
    }

    fn with_changed(&self, index: usize, change: impl Fn(Todo) -> Todo) -> Self {
        let items = self
            .items
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if i == index {
                    change(t.clone())
                } else {
                    t.clone()
                }
            })
            .collect();
        Self { items }
    }
}

/// What the file holds.
#[derive(Debug, Serialize, Deserialize)]
struct TodoFile {
    version: u32,
    todos: Vec<Todo>,
}

/// The project's TODOs (none without a file).
pub fn load(root: &Path) -> Result<Todos, String> {
    let path = root.join(NOTES_DIR).join(TODO_FILE);
    let invalid = |message: String| format!("{}: {message}", path.display());
    let Some(json) =
        read_regular_file(&path, MAX_FILE_BYTES).map_err(|e| invalid(e.to_string()))?
    else {
        return Ok(Todos::default());
    };
    let file: TodoFile = serde_json::from_str(&json).map_err(|e| invalid(e.to_string()))?;
    if file.version != FORMAT_VERSION {
        return Err(invalid(format!("unsupported version {}", file.version)));
    }
    if file.todos.len() > MAX_TODOS {
        return Err(invalid(format!("more than {MAX_TODOS} TODOs")));
    }
    for (i, todo) in file.todos.iter().enumerate() {
        check_todo(todo).map_err(|e| invalid(format!("TODO {}: {e}", i + 1)))?;
    }
    Ok(Todos { items: file.todos })
}

/// Saves the TODOs atomically; writes nothing for none when there is no
/// file yet, so browsing a project never adds files to it.
pub fn save(root: &Path, todos: &Todos) -> Result<(), String> {
    let dir = root.join(NOTES_DIR);
    if todos.items.is_empty() && !dir.join(TODO_FILE).exists() {
        return Ok(());
    }
    todos.items.iter().try_for_each(check_todo)?;
    let file = TodoFile {
        version: FORMAT_VERSION,
        todos: todos.items.clone(),
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    if json.len() >= MAX_FILE_BYTES {
        return Err("Too many TODOs to save; clear some first".into());
    }
    write_atomically(&dir, TODO_FILE, &format!("{json}\n"))
        .map_err(|e| format!("Could not save {}: {e}", dir.join(TODO_FILE).display()))
}

/// Control characters, and the ones that flip the direction text is shown
/// in (which can make `evil.c` show as something else).
fn is_disallowed(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200E}' | '\u{200F}' | '\u{061C}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

/// What `load` accepts: one line of text, and a line of a project file.
/// What the TODO file accepts.
pub fn check_todo(todo: &Todo) -> Result<(), String> {
    if todo.text.is_empty() {
        return Err("a TODO needs some text".into());
    }
    if todo.text.len() > MAX_TEXT_BYTES {
        return Err(format!("a TODO is longer than {MAX_TEXT_BYTES} bytes"));
    }
    if todo.text.chars().any(is_disallowed) {
        return Err("a TODO is one line, with no control or text-direction characters".into());
    }
    match (&todo.file, todo.line) {
        (None, None) => Ok(()),
        (Some(file), _) if file.chars().any(is_disallowed) => {
            Err("a TODO's file has control or text-direction characters in it".into())
        }
        (Some(file), Some(line)) => check_target(&NoteTarget::Line {
            file: file.clone(),
            line,
        }),
        _ => Err("a TODO's place needs both a file and a line".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Todos {
        let place = Some(("net/parse.c".to_string(), 42));
        Todos::default()
            .with_added(Todo::new("check the length in parse_header", place))
            .and_then(|t| t.with_added(Todo::new("fuzz the TLV reader", None)))
            .expect("valid")
    }

    #[test]
    fn edits_return_a_new_list() {
        let todos = sample();
        let done = todos.with_done(0, true);
        assert!(!todos.items()[0].is_done && done.items()[0].is_done);
        assert_eq!(done.open_count(), 1);
        assert_eq!(done.without_done().items().len(), 1);
        assert_eq!(todos.without(0).items()[0].text, "fuzz the TLV reader");
        let renamed = todos.with_text(1, "  fuzz it  ").expect("valid");
        assert_eq!(renamed.items()[1].text, "fuzz it");
        assert!(todos.with_text(1, " ").is_err());
        assert_eq!(todos.items()[0].place(), Some(("net/parse.c", 42)));
    }

    #[test]
    fn saves_and_loads_and_writes_nothing_for_nothing() {
        let dir = tempfile::tempdir().expect("dir");
        save(dir.path(), &Todos::default()).expect("nothing to save");
        assert!(!dir.path().join(NOTES_DIR).exists());
        save(dir.path(), &sample()).expect("saved");
        assert_eq!(load(dir.path()), Ok(sample()));
        let json = std::fs::read_to_string(dir.path().join(NOTES_DIR).join(TODO_FILE));
        assert!(json.is_ok_and(|j| j.contains("\"done\": false")));
    }

    #[test]
    fn refuses_bad_todos_and_hostile_files() {
        assert!(Todos::default().with_added(Todo::new("", None)).is_err());
        assert!(Todos::default()
            .with_added(Todo::new("two\nlines", None))
            .is_err());
        assert!(Todos::default()
            .with_added(Todo::new("x", Some(("../outside.c".into(), 1))))
            .is_err());
        for file in ["a\u{202E}c.doc", "a\nb.c"] {
            let todo = Todo::new("x", Some((file.into(), 1)));
            assert!(Todos::default().with_added(todo).is_err(), "{file:?}");
        }
        assert!(Todos::default()
            .with_added(Todo::new("sneaky \u{2067}text", None))
            .is_err());
        let dir = tempfile::tempdir().expect("dir");
        let folder = dir.path().join(NOTES_DIR);
        std::fs::create_dir(&folder).expect("mkdir");
        for json in [
            r#"{"version": 2, "todos": []}"#,
            r#"{"version": 1, "todos": [{"text": "x", "file": "a.c"}]}"#,
            r#"{"version": 1, "todos": [{"text": "x", "file": "/etc/passwd", "line": 1}]}"#,
        ] {
            std::fs::write(folder.join(TODO_FILE), json).expect("write");
            assert!(load(dir.path()).is_err(), "{json}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_a_symlinked_file() {
        let dir = tempfile::tempdir().expect("dir");
        let folder = dir.path().join(NOTES_DIR);
        std::fs::create_dir(&folder).expect("mkdir");
        let elsewhere = dir.path().join("elsewhere.json");
        std::fs::write(&elsewhere, r#"{"version": 1, "todos": []}"#).expect("write");
        std::os::unix::fs::symlink(&elsewhere, folder.join(TODO_FILE)).expect("symlink");
        assert!(load(dir.path()).is_err());
        assert!(save(dir.path(), &sample()).is_err());
    }
}

//! Sessions: where you were in a project — the selected symbol, the line
//! the source showed, the graph view, which panes were open, the last
//! trace. One is kept for each project you open (in the app's own storage)
//! and restored when you open it again; Save Session As… writes one to a
//! file, with the project's folder, to come back to or hand to someone.
//!
//! Session files can come from anyone, so reading one checks everything:
//! sizes, a project folder that is a full path, file names inside the
//! project.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::notes::store::check_target;
use crate::notes::NoteTarget;

/// Extension of session files.
pub const SESSION_EXTENSION: &str = "srcwranglr-session";
const FORMAT_VERSION: u32 = 1;
/// A session file larger than this isn't read.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Projects whose sessions are kept, most recent first.
const MAX_KEPT: usize = 50;

/// Where you were in one project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// The selected symbol's qualified name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// The line the source showed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<Place>,
    #[serde(default)]
    pub view: View,
    #[serde(default)]
    pub panes: Panes,
    /// The trace in the Trace pane, to run again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<TraceStart>,
}

/// A line of a project file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub file: String,
    pub line: u32,
}

/// The graph view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    #[default]
    Trail,
    CallHierarchy,
}

/// Which side panes are open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Panes {
    #[serde(default)]
    pub notes: bool,
    #[serde(default)]
    pub findings: bool,
    #[serde(default)]
    pub todos: bool,
}

/// What a trace was started from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum TraceStart {
    /// A risky call (a finding's Trace Back).
    Call {
        file: String,
        line: u32,
        column: u32,
    },
    /// A variable picked in the source.
    Variable {
        file: String,
        line: u32,
        column: u32,
    },
    /// A variable picked in the source, traced forward to where it goes.
    Forward {
        file: String,
        line: u32,
        column: u32,
    },
}

impl TraceStart {
    fn place(&self) -> (&str, u32) {
        match self {
            Self::Call { file, line, .. }
            | Self::Variable { file, line, .. }
            | Self::Forward { file, line, .. } => (file, *line),
        }
    }
}

impl Session {
    /// Whether everything in it is something a project could have: names
    /// and files that notes accept, lines from 1.
    pub fn check(&self) -> Result<(), String> {
        if let Some(symbol) = &self.symbol {
            check_target(&NoteTarget::Symbol(symbol.clone()))?;
        }
        let places = self
            .place
            .iter()
            .map(|p| (p.file.as_str(), p.line))
            .chain(self.trace.iter().map(TraceStart::place));
        for (file, line) in places {
            check_target(&NoteTarget::Line {
                file: file.to_string(),
                line,
            })?;
            if line == 0 {
                return Err("lines start at 1".into());
            }
        }
        Ok(())
    }
}

/// The session of each project opened, most recent first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sessions {
    projects: Vec<(PathBuf, Session)>,
}

impl Sessions {
    /// Where they are kept in eframe's storage.
    pub const STORAGE_KEY: &'static str = "sessions";

    /// The sessions saved in `storage`, the ones that don't check out
    /// left out.
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        let saved: Self = storage
            .and_then(|s| eframe::get_value(s, Self::STORAGE_KEY))
            .unwrap_or_default();
        let projects = saved
            .projects
            .into_iter()
            .filter(|(root, session)| root.is_absolute() && session.check().is_ok())
            .take(MAX_KEPT)
            .collect();
        Self { projects }
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, Self::STORAGE_KEY, self);
    }

    /// The sessions with `root`'s replaced by `session`, first.
    pub fn with(&self, root: &Path, session: Session) -> Self {
        let projects = std::iter::once((root.to_path_buf(), session))
            .chain(self.projects.iter().filter(|(r, _)| r != root).cloned())
            .take(MAX_KEPT)
            .collect();
        Self { projects }
    }

    pub fn get(&self, root: &Path) -> Option<&Session> {
        self.projects
            .iter()
            .find(|(r, _)| r == root)
            .map(|(_, session)| session)
    }
}

/// A session file: the project's folder and where you were in it.
#[derive(Debug, Serialize, Deserialize)]
struct SessionFile {
    srcwranglr_session: u32,
    project: PathBuf,
    session: Session,
}

/// A session file's text for `session` in the project at `root`.
pub fn to_file(root: &Path, session: &Session) -> Result<String, String> {
    let file = SessionFile {
        srcwranglr_session: FORMAT_VERSION,
        project: root.to_path_buf(),
        session: session.clone(),
    };
    serde_json::to_string_pretty(&file)
        .map(|json| format!("{json}\n"))
        .map_err(|e| format!("Could not write the session: {e}"))
}

/// The project folder and session in a session file's text.
pub fn from_file(text: &str) -> Result<(PathBuf, Session), String> {
    let file: SessionFile =
        serde_json::from_str(text).map_err(|e| format!("not a session file: {e}"))?;
    if file.srcwranglr_session != FORMAT_VERSION {
        return Err(format!(
            "session file version {} isn't supported",
            file.srcwranglr_session
        ));
    }
    if !file.project.is_absolute() {
        return Err("the project folder in it isn't a full path".into());
    }
    file.session.check()?;
    Ok((file.project, file.session))
}

/// Reads the session file at `path`.
pub fn read_file(path: &Path) -> Result<(PathBuf, Session), String> {
    let shown = path.display();
    let metadata = std::fs::metadata(path).map_err(|e| format!("{shown}: {e}"))?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err(format!(
            "{shown} isn't a session file of at most {MAX_FILE_BYTES} bytes"
        ));
    }
    // Capped as it is read, too: the file may grow after the check.
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(MAX_FILE_BYTES + 1).read_to_string(&mut text))
        .map_err(|e| format!("{shown}: {e}"))?;
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("{shown} is larger than {MAX_FILE_BYTES} bytes"));
    }
    from_file(&text).map_err(|e| format!("{shown}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Session {
        Session {
            symbol: Some("leveldb::DBImpl::Write".into()),
            place: Some(Place {
                file: "db/db_impl.cc".into(),
                line: 1200,
            }),
            view: View::CallHierarchy,
            panes: Panes {
                notes: true,
                findings: false,
                todos: true,
            },
            trace: Some(TraceStart::Variable {
                file: "db/db_impl.cc".into(),
                line: 1210,
                column: 8,
            }),
        }
    }

    #[test]
    fn a_session_file_round_trips() {
        let root = Path::new("/src/leveldb");
        let text = to_file(root, &sample()).expect("written");
        assert!(text.contains("\"from\": \"variable\""), "{text}");
        assert_eq!(from_file(&text), Ok((root.to_path_buf(), sample())));
    }

    #[test]
    fn a_session_file_from_elsewhere_is_checked() {
        let with = |project: &str, session: &str| {
            format!(
                "{{\"srcwranglr_session\": 1, \"project\": {project:?}, \"session\": {session}}}"
            )
        };
        for (text, says) in [
            (with("relative/dir", "{}"), "full path"),
            (
                with(
                    "/p",
                    r#"{"place": {"file": "../../etc/passwd", "line": 1}}"#,
                ),
                "",
            ),
            (
                with("/p", r#"{"place": {"file": "a.c", "line": 0}}"#),
                "start at 1",
            ),
            (
                "{\"srcwranglr_session\": 9, \"project\": \"/p\", \"session\": {}}".into(),
                "version",
            ),
            ("not json".into(), "not a session file"),
        ] {
            let error = from_file(&text).expect_err(&text);
            assert!(error.contains(says), "{text}: {error}");
        }
        let minimal = with("/p", "{}");
        assert_eq!(from_file(&minimal), Ok(("/p".into(), Session::default())));
    }

    #[test]
    fn the_latest_session_of_each_project_is_kept_first() {
        let (a, b) = (Path::new("/a"), Path::new("/b"));
        let sessions = Sessions::default()
            .with(a, Session::default())
            .with(b, Session::default())
            .with(a, sample());
        assert_eq!(sessions.get(a), Some(&sample()));
        assert_eq!(sessions.projects.len(), 2);
        assert_eq!(sessions.projects[0].0, a);
        let many = (0..MAX_KEPT + 5).fold(Sessions::default(), |s, i| {
            s.with(&PathBuf::from(format!("/p{i}")), Session::default())
        });
        assert_eq!(many.projects.len(), MAX_KEPT);
    }

    #[test]
    fn reads_a_file_and_refuses_a_huge_one() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join(format!("s.{SESSION_EXTENSION}"));
        std::fs::write(&path, to_file(Path::new("/p"), &sample()).expect("text")).expect("write");
        assert_eq!(read_file(&path).map(|(_, s)| s), Ok(sample()));
        let huge = dir.path().join("huge");
        std::fs::write(&huge, vec![b' '; MAX_FILE_BYTES as usize + 1]).expect("write");
        assert!(read_file(&huge).is_err_and(|e| e.contains("at most")));
    }
}

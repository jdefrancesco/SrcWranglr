//! SrcWranglr's own diagnostics log: an in-memory ring buffer the Log
//! window shows, plus a human-readable file in the config folder that
//! outlives a crash. A panic hook records the panic (message, place and
//! backtrace) to both before the usual stderr print, so a crash that takes
//! the window down still leaves a trace to read next launch.
//!
//! This logs SrcWranglr's *own* activity and troubles — analyzer runs,
//! panics, background-job failures, internal errors — never the project's
//! source.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use super::dangerous_lists::config_dir;

/// The log file's name in the config folder.
const LOG_FILE: &str = "srcwranglr.log";
/// At most this many lines are kept in memory (and seeded from the file).
const MAX_LINES: usize = 2000;
/// The file is rotated once it passes this size, so it can't grow forever.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// How serious a log line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
    Panic,
}

impl Level {
    pub fn tag(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
            Level::Panic => "PANIC",
        }
    }

    fn from_tag(tag: &str) -> Option<Self> {
        Level::ALL.into_iter().find(|l| l.tag() == tag)
    }

    const ALL: [Level; 4] = [Level::Info, Level::Warn, Level::Error, Level::Panic];
}

/// One logged line: when (unix seconds), how serious, and the text (which
/// may be several lines, e.g. a backtrace).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub when: u64,
    pub level: Level,
    pub text: String,
}

/// The buffer the Log window reads, and the file it is mirrored to.
struct Logger {
    buffer: Mutex<VecDeque<Line>>,
    file: Option<PathBuf>,
}

impl Logger {
    fn new(file: Option<PathBuf>) -> Self {
        Self {
            buffer: Mutex::new(VecDeque::new()),
            file,
        }
    }

    /// Fills the buffer from the file (the previous sessions), bounded.
    fn load_file(&self) {
        let Some(path) = &self.file else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let mut buffer = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        for line in text.lines().filter_map(parse_line) {
            buffer.push_back(line);
        }
        while buffer.len() > MAX_LINES {
            buffer.pop_front();
        }
    }

    /// If the file has grown past the cap, keeps the latest half, so the log
    /// can't grow without bound.
    fn rotate_if_large(&self) {
        let Some(path) = &self.file else {
            return;
        };
        let too_big = std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_FILE_BYTES);
        if !too_big {
            return;
        }
        if let Ok(text) = std::fs::read_to_string(path) {
            let kept: Vec<&str> = text.lines().collect();
            let half = kept.len() / 2;
            let tail = kept[half..].join("\n");
            let _ = super::dangerous_lists::write_user_file(path, &format!("{tail}\n"));
        }
    }

    fn record(&self, line: Line) {
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.push_back(line.clone());
            while buffer.len() > MAX_LINES {
                buffer.pop_front();
            }
        }
        self.append(&line);
    }

    /// Appends one line to the file (best effort; logging never panics).
    fn append(&self, line: &Line) {
        let Some(path) = &self.file else {
            return;
        };
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        else {
            return;
        };
        let _ = file.write_all(encode_line(line).as_bytes());
    }

    fn snapshot(&self) -> Vec<Line> {
        self.buffer
            .lock()
            .map(|b| b.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn clear(&self) {
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.clear();
        }
        if let Some(path) = &self.file {
            let _ = super::dangerous_lists::write_user_file(path, "");
        }
    }
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Sets up the log (reading the previous session's file) and installs the
/// panic hook. Called once at startup; a second call does nothing.
pub(crate) fn install() {
    let path = config_dir().map(|dir| dir.join(LOG_FILE));
    if let Some(dir) = path.as_ref().and_then(|p| p.parent()) {
        let _ = std::fs::create_dir_all(dir);
    }
    let logger = Logger::new(path);
    logger.rotate_if_large();
    logger.load_file();
    if LOGGER.set(logger).is_err() {
        return;
    }
    record(
        Level::Info,
        format!("SrcWranglr {} started.", crate::version::version_line()),
    );
    install_panic_hook();
}

/// Records a panic to the log (with its place and a backtrace), then lets the
/// usual hook run (which prints to stderr).
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record(Level::Panic, format_panic(info));
        previous(info);
    }));
}

/// A panic's thread, place, message and backtrace, as one block of text.
fn format_panic(info: &std::panic::PanicHookInfo<'_>) -> String {
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("unnamed");
    let place = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "unknown location".to_string());
    let message = panic_message(info);
    let backtrace = std::backtrace::Backtrace::force_capture();
    format!("panic in thread '{name}' at {place}:\n{message}\n{backtrace}")
}

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Adds a line to the log, if it has been set up.
pub(crate) fn record(level: Level, text: impl Into<String>) {
    if let Some(logger) = LOGGER.get() {
        logger.record(Line {
            when: now(),
            level,
            text: text.into(),
        });
    }
}

/// The lines in the log, oldest first.
pub(crate) fn lines() -> Vec<Line> {
    LOGGER.get().map(Logger::snapshot).unwrap_or_default()
}

/// Empties the log (and its file).
pub(crate) fn clear() {
    if let Some(logger) = LOGGER.get() {
        logger.clear();
    }
}

/// Where the log file is, if there is one.
pub(crate) fn file_path() -> Option<PathBuf> {
    LOGGER.get().and_then(|l| l.file.clone())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// One line for the file: `when<TAB>LEVEL<TAB>text`, the text's newlines and
/// backslashes escaped so a multi-line entry stays one physical line.
fn encode_line(line: &Line) -> String {
    let escaped = line.text.replace('\\', "\\\\").replace('\n', "\\n");
    format!("{}\t{}\t{escaped}\n", line.when, line.level.tag())
}

/// A line read back from the file, or `None` if it isn't one.
fn parse_line(raw: &str) -> Option<Line> {
    let mut parts = raw.splitn(3, '\t');
    let when: u64 = parts.next()?.parse().ok()?;
    let level = Level::from_tag(parts.next()?)?;
    let text = unescape(parts.next()?);
    Some(Line { when, level, text })
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(level: Level, text: &str) -> Line {
        Line {
            when: 1_700_000_000,
            level,
            text: text.into(),
        }
    }

    #[test]
    fn a_line_with_newlines_round_trips_through_the_file_form() {
        let original = line(Level::Panic, "boom at x.rs:9\n  frame 1\n  frame 2");
        let encoded = encode_line(&original);
        assert!(encoded.ends_with('\n'), "one physical line");
        assert_eq!(encoded.matches('\n').count(), 1, "no raw newlines inside");
        let parsed = parse_line(encoded.trim_end_matches('\n')).expect("parsed");
        assert_eq!(parsed, original);
    }

    #[test]
    fn a_line_that_is_not_the_log_form_is_skipped() {
        assert!(parse_line("just some text").is_none());
        assert!(parse_line("123\tNOTALEVEL\thi").is_none());
    }

    #[test]
    fn the_buffer_keeps_only_the_last_lines_and_mirrors_to_the_file() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("srcwranglr.log");
        let logger = Logger::new(Some(path.clone()));
        for i in 0..(MAX_LINES + 10) {
            logger.record(line(Level::Info, &format!("line {i}")));
        }
        let kept = logger.snapshot();
        assert_eq!(kept.len(), MAX_LINES, "ring is capped");
        assert_eq!(kept.last().unwrap().text, format!("line {}", MAX_LINES + 9));
        // The file has every line; a fresh logger reads the tail back.
        let reread = Logger::new(Some(path));
        reread.load_file();
        let seeded = reread.snapshot();
        assert_eq!(seeded.len(), MAX_LINES, "file tail is bounded too");
        assert_eq!(seeded.last().unwrap().level, Level::Info);
    }

    #[test]
    fn clearing_empties_the_buffer_and_file() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("srcwranglr.log");
        let logger = Logger::new(Some(path.clone()));
        logger.record(line(Level::Error, "a problem"));
        logger.clear();
        assert!(logger.snapshot().is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    }
}

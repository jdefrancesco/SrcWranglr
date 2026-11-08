//! Docs for a function: what the Docs window shows, and the man pages read
//! so far. Pages are read in the background, a few at a time, and kept
//! for the session, so hovering a call shows its summary once read.

use std::collections::HashMap;
use std::sync::Arc;

use super::background::BackgroundJob;
use crate::docs::doxygen::Doc;
use crate::docs::man::{lookup_name, man_page, ManPage};
use crate::docs::{doc_of, symbol_comment};
use crate::model::{CodeGraph, SymbolId};

/// Pages read at once; more wait for one of these to finish.
const MAX_READING: usize = 4;
/// Pages kept; past this the read ones are forgotten.
const MAX_KEPT: usize = 256;
/// A declaration line longer than this is cut (minified code).
const MAX_SIGNATURE_CHARS: usize = 300;

/// What a library function's page lookup has come to.
#[derive(Clone)]
pub enum PageState {
    /// Being read, or waiting to be.
    Reading,
    /// Read: the page, or `None` if this system has none.
    Read(Option<Arc<ManPage>>),
    Failed(String),
}

/// What the Docs window shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    /// A function the project defines, with the comment documenting it.
    Project(Box<ProjectDoc>),
    /// A library function's man page, by the name looked up.
    Library(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectDoc {
    pub symbol: SymbolId,
    pub name: String,
    pub kind: &'static str,
    pub file: String,
    pub line: u32,
    /// Its declaration's line, as written.
    pub signature: String,
    /// The comment's parts, where it is, and whether it is Doxygen's.
    pub comment: Option<(Doc, String, u32, bool)>,
}

impl ProjectDoc {
    pub fn of(graph: &CodeGraph, id: SymbolId) -> Option<Self> {
        let symbol = graph.symbols.get(&id)?;
        let signature = graph
            .file_contents
            .get(&symbol.file)
            .and_then(|source| source.lines().nth(symbol.line.saturating_sub(1) as usize))
            .map(|line| cut(line.trim(), MAX_SIGNATURE_CHARS))
            .unwrap_or_default();
        let comment = symbol_comment(graph, id).map(|(file, comment)| {
            let doc = doc_of(&comment.text, &symbol.name);
            (doc, file, comment.line, comment.is_doxygen)
        });
        Some(Self {
            symbol: id,
            name: symbol.qualified_name.clone(),
            kind: symbol.kind.label(),
            file: symbol.file.clone(),
            line: symbol.line,
            signature,
            comment,
        })
    }
}

/// `text` cut to `max` characters, ending in `…` when cut.
fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

#[derive(Default)]
pub struct Docs {
    /// What the window shows, while it is open.
    pub shown: Option<Shown>,
    pages: HashMap<String, PageState>,
    reading: Vec<(String, BackgroundJob<Option<ManPage>>)>,
    /// Names waiting for a free reader, oldest first.
    waiting: Vec<String>,
}

impl Docs {
    /// Starts reading the man page for the call `called`, unless it is
    /// read, being read, or not a C name. Returns the name looked up.
    pub fn request(&mut self, called: &str, ctx: &egui::Context) -> Option<String> {
        let name = lookup_name(called)?.to_string();
        if self.pages.contains_key(&name) {
            return Some(name);
        }
        if self.pages.len() >= MAX_KEPT {
            self.pages
                .retain(|_, page| matches!(page, PageState::Reading));
        }
        self.pages.insert(name.clone(), PageState::Reading);
        self.waiting.push(name.clone());
        self.start_waiting(ctx);
        Some(name)
    }

    fn start_waiting(&mut self, ctx: &egui::Context) {
        while self.reading.len() < MAX_READING && !self.waiting.is_empty() {
            let name = self.waiting.remove(0);
            let called = name.clone();
            let job = BackgroundJob::spawn(
                "Reading the man page",
                move |stop| man_page(&called, stop),
                ctx.clone(),
            );
            self.reading.push((name, job));
        }
    }

    /// Takes in the pages read since last time. Returns the names whose
    /// state changed.
    pub fn poll(&mut self, ctx: &egui::Context) -> Vec<String> {
        let mut done = Vec::new();
        let mut still = Vec::with_capacity(self.reading.len());
        for (name, job) in std::mem::take(&mut self.reading) {
            match job.poll() {
                Some(Ok(page)) => {
                    self.pages
                        .insert(name.clone(), PageState::Read(page.map(Arc::new)));
                    done.push(name);
                }
                Some(Err(error)) => {
                    self.pages.insert(name.clone(), PageState::Failed(error));
                    done.push(name);
                }
                None => still.push((name, job)),
            }
        }
        self.reading = still;
        self.start_waiting(ctx);
        done
    }

    /// `docs` with `page` read for `name`, as if looked up.
    #[cfg(test)]
    pub fn with_page(mut self, name: &str, page: Option<ManPage>) -> Self {
        self.pages
            .insert(name.to_string(), PageState::Read(page.map(Arc::new)));
        self
    }

    /// Where the lookup of `name` (as looked up) is, if it was asked for.
    pub fn page(&self, name: &str) -> Option<&PageState> {
        self.pages.get(name)
    }

    /// The one-line summary of `name`'s page: `Some(None)` if this system
    /// has no page for it, `None` while it isn't read.
    pub fn summary(&self, name: &str) -> Option<Option<String>> {
        match self.pages.get(name)? {
            PageState::Read(page) => Some(page.as_ref().and_then(|p| p.summary())),
            PageState::Failed(_) => Some(None),
            PageState::Reading => None,
        }
    }

    /// Forgets what the window shows and the pages being read (a project
    /// closed); pages read are kept, as they come from this system.
    pub fn reset(&mut self) {
        self.shown = None;
        self.reading.clear();
        for name in self.waiting.drain(..) {
            self.pages.remove(&name);
        }
        self.pages
            .retain(|_, page| !matches!(page, PageState::Reading));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::graph_of_files;

    #[test]
    fn a_project_function_s_doc_has_its_comment_parts() {
        let source = "/** @brief Frees it.\n @param p the buffer */\nvoid release(void *p) {\n}\n";
        let graph = graph_of_files(&[("r.c", source)]);
        let id = graph
            .symbols
            .values()
            .find(|s| s.name == "release")
            .map(|s| s.id)
            .expect("release");
        let doc = ProjectDoc::of(&graph, id).expect("a doc");
        assert_eq!(doc.signature, "void release(void *p) {");
        let (parts, file, line, is_doxygen) = doc.comment.expect("a comment");
        assert_eq!((file.as_str(), line, is_doxygen), ("r.c", 1, true));
        assert_eq!(parts.brief, "Frees it.");
        assert_eq!(parts.params[0].name, "p");
    }

    #[test]
    fn only_c_names_are_requested_and_each_once() {
        let ctx = egui::Context::default();
        let mut docs = Docs::default();
        assert_eq!(docs.request("--pager=sh", &ctx), None);
        assert_eq!(docs.request("obj.read", &ctx), None);
        assert_eq!(
            docs.request("std::no_such_fn_q", &ctx).as_deref(),
            Some("no_such_fn_q")
        );
        assert!(matches!(
            docs.page("no_such_fn_q"),
            Some(PageState::Reading)
        ));
        assert_eq!(docs.summary("no_such_fn_q"), None);
        docs.request("no_such_fn_q", &ctx);
        assert_eq!(docs.reading.len() + docs.waiting.len(), 1);
    }

    #[test]
    fn at_most_a_few_pages_are_read_at_once() {
        let ctx = egui::Context::default();
        let mut docs = Docs::default();
        for i in 0..10 {
            docs.request(&format!("no_such_fn_{i}"), &ctx);
        }
        assert_eq!(docs.reading.len(), MAX_READING);
        assert_eq!(docs.waiting.len(), 10 - MAX_READING);
        docs.reset();
        assert!(docs.reading.is_empty() && docs.waiting.is_empty());
        assert!(docs.page("no_such_fn_9").is_none());
    }
}

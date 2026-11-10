//! What the app does for a rule editor (in Find More Like This or the
//! Semgrep Rules window): testing a rule on the project, having semgrep
//! check it, saving it, going to what it matched; and the rest of what
//! the Semgrep Rules window asks.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::background::BackgroundJob;
use super::rule_editor::{rule_id_in, EditorAction, RuleEditor, SaveTarget};
use super::rule_files::{self, RuleHome};
use super::rule_library;
use super::rules_view::{RulesAction, RulesView};
use super::tool_job::ToolJob;
use super::trust;
use super::SrcWranglrApp;
use crate::analysis::wrappers::wrappers;
use crate::model::Span;

const UNTRUSTED: &str = "Trust this project's files to change its rules.";

/// Tells apart the rule files of tests started by this process.
static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Which window's editor an action came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditorOwner {
    Similar,
    Rules,
}

impl SrcWranglrApp {
    fn editor_of(&mut self, owner: EditorOwner) -> Option<&mut RuleEditor> {
        match owner {
            EditorOwner::Similar => self.similar_view.as_mut().map(|v| &mut v.editor),
            EditorOwner::Rules => self.rules_view.as_mut().map(|v| &mut v.editor),
        }
    }

    pub(super) fn on_editor_action(
        &mut self,
        owner: EditorOwner,
        action: EditorAction,
        ctx: &egui::Context,
    ) {
        match action {
            EditorAction::Test(yaml) => self.test_rule(owner, &yaml, ctx),
            EditorAction::Check(target, yaml) => self.check_rule(owner, target, yaml, ctx),
            EditorAction::Save(target, yaml) => self.save_rule(owner, &target, &yaml),
            EditorAction::GoTo(found) => match found.columns {
                Some((start, end)) => {
                    let span = Span {
                        line: found.line,
                        start,
                        end,
                    };
                    self.go_to_span(found.symbol, &found.file, span);
                }
                None => self.go_to_place(found.symbol, &found.file, found.line),
            },
        }
    }

    /// Runs `yaml`, alone, on the project in the background.
    pub(super) fn test_rule(&mut self, owner: EditorOwner, yaml: &str, ctx: &egui::Context) {
        let root = self.project.root.clone();
        let search = self.settings.search_path();
        let Some(editor) = self.editor_of(owner) else {
            return;
        };
        let Some(root) = root else {
            editor.test_failed(
                "Open a project folder to test rules on it; the demo isn't on disk.".into(),
            );
            return;
        };
        match write_temp_rule(yaml) {
            Ok(file) => {
                editor.start_test(ToolJob::spawn_rule_test(file, root, search, ctx.clone()))
            }
            Err(error) => editor.test_failed(error),
        }
    }

    /// Has semgrep check `yaml` in the background, to save it to `target`
    /// if it passes.
    fn check_rule(
        &mut self,
        owner: EditorOwner,
        target: SaveTarget,
        yaml: String,
        ctx: &egui::Context,
    ) {
        let search = self.settings.search_path();
        let Some(editor) = self.editor_of(owner) else {
            return;
        };
        let job = write_temp_rule(&yaml)
            .and_then(|file| ToolJob::spawn_rule_check(file, search, ctx.clone()));
        match job {
            Ok(job) => editor.start_check(job, target, yaml),
            Err(error) => editor.check_failed(target, yaml, error),
        }
    }

    pub(super) fn save_rule(&mut self, owner: EditorOwner, target: &SaveTarget, yaml: &str) {
        let root = self.project.root.clone();
        let home = target.home();
        let saved = match target {
            SaveTarget::New(home, fallback) => {
                let id = rule_id_in(yaml).unwrap_or(fallback);
                rule_files::save_rule(*home, root.as_deref(), id, yaml)
            }
            SaveTarget::Existing(RuleHome::Project, _) if !self.is_project_trusted() => {
                Err(UNTRUSTED.to_string())
            }
            SaveTarget::Existing(home, path) => {
                rule_files::overwrite_rule(*home, root.as_deref(), path, yaml)
            }
        };
        if saved.is_ok() && home == RuleHome::Project {
            self.trust_project_files_we_wrote();
        }
        // Semgrep runs ask the same when they start.
        let is_used = root.as_deref().is_some_and(trust::is_trusted);
        if let (Ok(path), Some(view)) = (&saved, self.rules_view.as_mut()) {
            // A rule saved from elsewhere is listed, but what the window
            // shows (maybe with edits) stays.
            let picked = (owner == EditorOwner::Rules).then_some(path.as_path());
            view.reload(rule_library::load(root.as_deref()), is_used, picked);
        }
        let message = saved.map(|path| saved_message(home, &path, root.as_deref(), is_used));
        if let Some(editor) = self.editor_of(owner) {
            editor.set_saved(message);
        }
    }

    pub(super) fn open_rules_view(&mut self) {
        let root = self.project.root.clone();
        let is_trusted = root.as_deref().is_some_and(trust::is_trusted);
        let library = rule_library::load(root.as_deref());
        self.rules_view = Some(RulesView::open(library, root.is_some(), is_trusted));
    }

    pub(super) fn show_rules_view(&mut self, ctx: &egui::Context) {
        let Some(view) = self.rules_view.as_mut() else {
            return;
        };
        match view.show(ctx, &self.project.graph) {
            None => self.rules_view = None,
            Some(Some(action)) => self.on_rules_action(action, ctx),
            Some(None) => {}
        }
    }

    fn on_rules_action(&mut self, action: RulesAction, ctx: &egui::Context) {
        match action {
            RulesAction::Editor(action) => self.on_editor_action(EditorOwner::Rules, action, ctx),
            RulesAction::Delete(home, path) => self.delete_rule(home, &path),
            RulesAction::Trust => {
                self.trust_project_files();
                self.reload_rules(None);
            }
            RulesAction::Reload => self.reload_rules(None),
            RulesAction::FindWrappers => {
                let graph = Arc::clone(&self.project.graph);
                let job = BackgroundJob::spawn(
                    "Looking for wrappers",
                    move |stop| wrappers(&graph, stop).ok_or_else(|| "Stopped.".to_string()),
                    ctx.clone(),
                );
                if let Some(view) = self.rules_view.as_mut() {
                    view.wrappers.start(job);
                }
            }
            RulesAction::GoTo { symbol, file, line } => self.go_to_place(symbol, &file, line),
        }
    }

    fn delete_rule(&mut self, home: RuleHome, path: &Path) {
        let root = self.project.root.clone();
        let deleted = match home == RuleHome::Project && !self.is_project_trusted() {
            true => Err(UNTRUSTED.to_string()),
            false => rule_files::delete_rule(home, root.as_deref(), path),
        };
        self.reload_rules(None);
        if let (Err(error), Some(view)) = (deleted, self.rules_view.as_mut()) {
            view.editor.set_saved(Err(error));
        }
    }

    fn is_project_trusted(&self) -> bool {
        self.project.root.as_deref().is_some_and(trust::is_trusted)
    }

    fn reload_rules(&mut self, path: Option<&Path>) {
        let root = self.project.root.clone();
        let is_trusted = root.as_deref().is_some_and(trust::is_trusted);
        if let Some(view) = self.rules_view.as_mut() {
            view.reload(rule_library::load(root.as_deref()), is_trusted, path);
        }
    }
}

/// Writes `yaml` to a fresh file of its own in the temp folder.
pub(super) fn write_temp_rule(yaml: &str) -> Result<PathBuf, String> {
    // Hard to guess, so another user of a shared temp folder can't take
    // the name first.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let file = std::env::temp_dir().join(format!(
        "srcwranglr-rule-{}-{}-{nanos}.yaml",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    create_private(&file, yaml)
        .map(|()| file)
        .map_err(|e| format!("Could not write the rule to run it: {e}"))
}

/// Creates `path` (failing if anything is there, even a symlink), only
/// you can read: it quotes the code being audited.
fn create_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(contents.as_bytes())
}

/// Where the rule went, and when semgrep runs use it.
fn saved_message(home: RuleHome, path: &Path, root: Option<&Path>, is_used: bool) -> String {
    let shown = root
        .and_then(|root| path.strip_prefix(root).ok())
        .unwrap_or(path)
        .display();
    match (home, is_used) {
        (RuleHome::Yours, _) => format!("Saved to {shown}. Every semgrep run uses it."),
        (RuleHome::Project, true) => {
            format!("Saved to {shown}. Analyze → Run semgrep uses it for this project.")
        }
        (RuleHome::Project, false) => format!(
            "Saved to {shown}. Semgrep runs use it once you trust this project's files \
             (Analyze → Trust This Project's Verdicts, List and Rules)."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::similar::Example;
    use crate::app::launch::LaunchOptions;
    use crate::app::project::Project;
    use crate::index::graph_of_files;

    fn app_with(source: &str, root: Option<PathBuf>) -> SrcWranglrApp {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let graph = graph_of_files(&[("a.c", source)]);
        app.project = Project::new("p", root, graph);
        app
    }

    #[test]
    fn a_project_rule_is_saved_and_says_when_it_is_used() {
        let dir = tempfile::tempdir().expect("dir");
        let source = "void f(char *b) {\n  gets(b);\n}\n";
        let mut app = app_with(source, Some(dir.path().to_path_buf()));
        app.are_project_files_trusted = false;
        app.find_similar(Example::line("a.c", 2));
        let target = SaveTarget::New(RuleHome::Project, "like-gets-2".into());
        app.save_rule(EditorOwner::Similar, &target, "rules: []\n");
        assert!(dir
            .path()
            .join(".srcwranglr/semgrep/like-gets-2.yaml")
            .is_file());
        let view = app.similar_view.as_ref().expect("open");
        let saved = view.editor.saved_text().unwrap_or_default();
        assert!(
            saved.contains(".srcwranglr/semgrep/like-gets-2.yaml"),
            "{saved}"
        );
        assert!(saved.contains("once you trust"), "{saved}");
    }

    #[test]
    fn a_saved_rule_is_picked_in_the_rules_window() {
        let dir = tempfile::tempdir().expect("dir");
        let mut app = app_with("int x;\n", Some(dir.path().to_path_buf()));
        app.open_rules_view();
        let target = SaveTarget::New(RuleHome::Project, "fallback".into());
        let yaml = "rules:\n  - id: mine\n    pattern: x\n";
        app.save_rule(EditorOwner::Rules, &target, yaml);
        let view = app.rules_view.as_ref().expect("open");
        assert_eq!(view.editor.yaml, yaml);
        let saved = view.editor.saved_text().unwrap_or_default();
        assert!(saved.contains("mine.yaml"), "{saved}");
    }

    #[test]
    fn testing_needs_a_project_on_disk() {
        let ctx = egui::Context::default();
        let mut app = app_with("int x;\n", None);
        app.find_similar(Example::line("a.c", 1));
        app.test_rule(EditorOwner::Similar, "rules: []\n", &ctx);
        let view = app.similar_view.as_ref().expect("open");
        assert!(view
            .editor
            .test_error()
            .unwrap_or_default()
            .contains("demo"));
    }

    #[cfg(unix)]
    #[test]
    fn a_rule_to_run_is_yours_alone() {
        use std::os::unix::fs::PermissionsExt;
        let file = write_temp_rule("a").expect("written");
        let mode = std::fs::metadata(&file).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::remove_file(file).expect("rm");
    }

    #[test]
    fn a_rule_to_run_gets_a_fresh_file() {
        let first = write_temp_rule("a").expect("written");
        let second = write_temp_rule("b").expect("written");
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(&first).expect("read"), "a");
        std::fs::remove_file(first).expect("rm");
        std::fs::remove_file(second).expect("rm");
    }
}

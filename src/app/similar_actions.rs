//! What the app does with the "Find More Like This" window: opening it on
//! a line, testing its rule on the project, saving it, and going to what
//! it matched.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::rule_files::{self, RuleHome};
use super::similar_view::{SimilarAction, SimilarView};
use super::tool_job::ToolJob;
use super::trust;
use super::SrcWranglrApp;
use crate::model::Span;
use crate::safe_fs;

/// Tells apart the rule files of tests started by this process.
static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

impl SrcWranglrApp {
    /// Opens the window with a rule drafted from `file:line`.
    pub(super) fn find_similar(&mut self, file: &str, line: u32) {
        self.similar_view = Some(SimilarView::open(&self.project.graph, file, line));
    }

    pub(super) fn show_similar_view(&mut self, ctx: &egui::Context) {
        let Some(view) = self.similar_view.as_mut() else {
            return;
        };
        match view.show(ctx, &self.project.graph) {
            None => self.similar_view = None,
            Some(Some(action)) => self.on_similar_action(action, ctx),
            Some(None) => {}
        }
    }

    fn on_similar_action(&mut self, action: SimilarAction, ctx: &egui::Context) {
        match action {
            SimilarAction::Test(yaml) => self.test_rule(&yaml, ctx),
            SimilarAction::Save(home, id, yaml) => self.save_rule(home, &id, &yaml),
            SimilarAction::GoTo(found) => match found.columns {
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
    fn test_rule(&mut self, yaml: &str, ctx: &egui::Context) {
        let Some(view) = self.similar_view.as_mut() else {
            return;
        };
        let Some(root) = self.project.root.clone() else {
            view.test_failed(
                "Open a project folder to test rules on it; the demo isn't on disk.".into(),
            );
            return;
        };
        match write_test_rule(yaml) {
            Ok(file) => {
                let search = self.settings.search_path();
                view.start_test(ToolJob::spawn_rule_test(file, root, search, ctx.clone()));
            }
            Err(error) => view.test_failed(error),
        }
    }

    fn save_rule(&mut self, home: RuleHome, id: &str, yaml: &str) {
        let root = self.project.root.clone();
        let saved = rule_files::save_rule(home, root.as_deref(), id, yaml);
        if saved.is_ok() && home == RuleHome::Project {
            self.trust_project_files_we_wrote();
        }
        // Semgrep runs ask the same when they start.
        let is_used = root.as_deref().is_some_and(trust::is_trusted);
        let message = saved.map(|path| saved_message(home, &path, root.as_deref(), is_used));
        if let Some(view) = self.similar_view.as_mut() {
            view.set_saved(message);
        }
    }
}

/// Writes `yaml` to a fresh file of its own in the temp folder.
fn write_test_rule(yaml: &str) -> Result<PathBuf, String> {
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
    safe_fs::write_new_file(&file, yaml)
        .map(|()| file)
        .map_err(|e| format!("Could not write the rule to test it: {e}"))
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
        app.find_similar("a.c", 2);
        app.save_rule(RuleHome::Project, "like-gets-2", "rules: []\n");
        assert!(dir
            .path()
            .join(".srcwranglr/semgrep/like-gets-2.yaml")
            .is_file());
        let view = app.similar_view.as_ref().expect("open");
        let saved = view_saved(view);
        assert!(
            saved.contains(".srcwranglr/semgrep/like-gets-2.yaml"),
            "{saved}"
        );
        assert!(saved.contains("once you trust"), "{saved}");
    }

    #[test]
    fn testing_needs_a_project_on_disk() {
        let ctx = egui::Context::default();
        let mut app = app_with("int x;\n", None);
        app.find_similar("a.c", 1);
        app.test_rule("rules: []\n", &ctx);
        let view = app.similar_view.as_ref().expect("open");
        assert!(view_test_error(view).contains("demo"));
    }

    #[test]
    fn a_test_rule_gets_a_fresh_file() {
        let first = write_test_rule("a").expect("written");
        let second = write_test_rule("b").expect("written");
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(&first).expect("read"), "a");
        std::fs::remove_file(first).expect("rm");
        std::fs::remove_file(second).expect("rm");
    }

    fn view_saved(view: &SimilarView) -> String {
        view.saved_text().unwrap_or_default()
    }

    fn view_test_error(view: &SimilarView) -> String {
        view.test_error().unwrap_or_default()
    }
}

//! A semgrep rule being written: its YAML to edit; Test, which runs it
//! alone on the project and lists every match; and Save, which has
//! semgrep check the rule first, so a broken one can't stop every later
//! semgrep run. Find More Like This and the Semgrep Rules window use it.

use std::path::PathBuf;

use egui::RichText;

use super::findings::line_text;
use super::rule_files::RuleHome;
use super::survey_view::ERROR_COLOR;
use super::tool_job::{with_symbols, ToolJob};
use super::visible;
use crate::analysis::Finding;
use crate::model::CodeGraph;

const RULE_ROWS: usize = 10;
/// A rule taller than this many rows scrolls in its box.
const MAX_RULE_ROWS: usize = 20;
/// Matches listed at most; the count says if there were more.
const MAX_LISTED: usize = 2_000;

/// Where a rule is saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveTarget {
    /// A new file in `home`, named after the rule's id, or this one if
    /// the id can't name a file.
    New(RuleHome, String),
    /// Over this file of `home`.
    Existing(RuleHome, PathBuf),
}

impl SaveTarget {
    pub fn home(&self) -> RuleHome {
        match self {
            SaveTarget::New(home, _) | SaveTarget::Existing(home, _) => *home,
        }
    }
}

/// A save button: where it saves, and what it says.
pub struct SaveButton {
    pub target: SaveTarget,
    pub label: &'static str,
    pub hover: &'static str,
}

impl SaveButton {
    /// "Save for This Project" and "Save for All Projects", for a new file
    /// named after `id` if the rule's own can't name one.
    pub fn new_file(id: &str) -> [SaveButton; 2] {
        [
            SaveButton {
                target: SaveTarget::New(RuleHome::Project, id.to_string()),
                label: "Save for This Project",
                hover: "Into the project's .srcwranglr/semgrep/: Analyze → Run semgrep uses it \
                        once you trust this project's files",
            },
            SaveButton {
                target: SaveTarget::New(RuleHome::Yours, id.to_string()),
                label: "Save for All Projects",
                hover: "Into your semgrep rules: every semgrep run uses it",
            },
        ]
    }
}

/// What the user asked of the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorAction {
    /// Run the rule (this text) on the project.
    Test(String),
    /// Have semgrep check the rule, to save it there if it passes.
    Check(SaveTarget, String),
    /// Save the rule there.
    Save(SaveTarget, String),
    /// Show this match in the source.
    GoTo(Finding),
}

#[derive(Default)]
pub struct RuleEditor {
    pub yaml: String,
    is_read_only: bool,
    test: Option<ToolJob>,
    matches: Option<Result<Vec<Finding>, String>>,
    /// A check running, and where the rule goes if it passes.
    check: Option<(ToolJob, SaveTarget, String)>,
    /// A rule semgrep refused: where it was to go, the rule, and why.
    refused: Option<(SaveTarget, String, String)>,
    /// A rule that passed its check, to save on the next frame.
    passed: Option<(SaveTarget, String)>,
    /// What the last save did.
    saved: Option<Result<String, String>>,
}

impl RuleEditor {
    pub fn new(yaml: String) -> Self {
        Self {
            yaml,
            ..Self::default()
        }
    }

    /// `yaml` to look at and test, not to edit or save.
    pub fn read_only(yaml: String) -> Self {
        Self {
            yaml,
            is_read_only: true,
            ..Self::default()
        }
    }

    /// Puts `yaml` in place of the rule, forgetting the old one's test,
    /// check and what was said about it.
    pub fn set_yaml(&mut self, yaml: String) {
        *self = Self {
            yaml,
            is_read_only: self.is_read_only,
            ..Self::default()
        };
    }

    /// Whether semgrep is checking the rule, to save it.
    pub fn is_checking(&self) -> bool {
        self.check.is_some() || self.passed.is_some()
    }

    /// Starts showing the results of `job`, a run of the rule.
    pub fn start_test(&mut self, job: ToolJob) {
        self.test = Some(job);
        self.matches = None;
    }

    pub fn test_failed(&mut self, error: String) {
        self.matches = Some(Err(error));
    }

    /// Starts `job`, semgrep checking `yaml`, to save it to `target` if
    /// it passes.
    pub fn start_check(&mut self, job: ToolJob, target: SaveTarget, yaml: String) {
        self.check = Some((job, target, yaml));
        self.refused = None;
        self.saved = None;
    }

    /// The check couldn't start; saving anyway is up to the user.
    pub fn check_failed(&mut self, target: SaveTarget, yaml: String, error: String) {
        self.refused = Some((target, yaml, error));
    }

    pub fn set_saved(&mut self, saved: Result<String, String>) {
        self.saved = Some(saved);
    }

    /// What the last save said, either way.
    #[cfg(test)]
    pub fn saved_text(&self) -> Option<String> {
        self.saved
            .clone()
            .map(|saved| saved.unwrap_or_else(|error| error))
    }

    /// Why the last test failed, if it did.
    #[cfg(test)]
    pub fn test_error(&self) -> Option<String> {
        self.matches.clone()?.err()
    }

    #[cfg(test)]
    pub fn set_matches(&mut self, matches: Result<Vec<Finding>, String>) {
        self.matches = Some(matches);
    }

    /// Why semgrep refused the rule, if it did.
    #[cfg(test)]
    pub fn refusal(&self) -> Option<&str> {
        self.refused.as_ref().map(|(_, _, error)| error.as_str())
    }

    /// Draws the rule, its buttons (`saves` among them, unless it is read
    /// only), what was said and what it matched. What was asked, if
    /// anything.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        saves: &[SaveButton],
    ) -> Option<EditorAction> {
        self.poll(graph);
        let mut action = None;
        let title = ui.label(match self.is_read_only {
            true => "Rule (semgrep YAML; read only):",
            false => "Rule (semgrep YAML; edit it as you like):",
        });
        // A long rule (a suggestion for every wrapper) scrolls inside its
        // box, so the buttons below it stay in view.
        let max_height = ui.text_style_height(&egui::TextStyle::Monospace) * MAX_RULE_ROWS as f32;
        egui::ScrollArea::vertical()
            .id_salt("rule_yaml")
            .max_height(max_height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.yaml)
                        .code_editor()
                        .interactive(!self.is_read_only)
                        .desired_rows(RULE_ROWS)
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(title.id);
            });
        action = self.buttons(ui, saves).or(action);
        action = self.status(ui).or(action);
        action = action.or_else(|| self.match_list(ui, graph));
        // A rule that passed its check is saved on a frame with nothing
        // else asked, so neither is lost.
        action.or_else(|| {
            let (target, yaml) = self.passed.take()?;
            Some(EditorAction::Save(target, yaml))
        })
    }

    fn poll(&mut self, graph: &CodeGraph) {
        if let Some(result) = self.test.as_ref().and_then(ToolJob::poll) {
            self.test = None;
            self.matches = Some(result.map(|found| {
                let mut found = with_symbols(found, graph);
                found.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
                found
            }));
        }
        let Some(result) = self.check.as_ref().and_then(|(job, _, _)| job.poll()) else {
            return;
        };
        let Some((_, target, yaml)) = self.check.take() else {
            return;
        };
        match result {
            Ok(_) => self.passed = Some((target, yaml)),
            Err(error) => self.refused = Some((target, yaml, error)),
        }
    }

    fn buttons(&mut self, ui: &mut egui::Ui, saves: &[SaveButton]) -> Option<EditorAction> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            let test = ui.add_enabled(self.test.is_none(), egui::Button::new("Test"));
            if test
                .on_hover_text("Run this rule on the project and list what it matches")
                .clicked()
            {
                action = Some(EditorAction::Test(self.yaml.clone()));
            }
            if self.is_read_only {
                return;
            }
            let is_checking = self.check.is_some();
            for save in saves {
                let button = ui.add_enabled(!is_checking, egui::Button::new(save.label));
                if button.on_hover_text(save.hover).clicked() {
                    action = Some(EditorAction::Check(save.target.clone(), self.yaml.clone()));
                }
            }
        });
        action
    }

    fn status(&mut self, ui: &mut egui::Ui) -> Option<EditorAction> {
        let mut action = None;
        // Saved messages name files, which may be the project's own.
        match &self.saved {
            Some(Ok(message)) => {
                ui.label(visible::visible(message));
            }
            Some(Err(error)) => {
                ui.label(RichText::new(visible::visible(error)).color(ERROR_COLOR));
            }
            None => {}
        }
        if let Some((target, yaml, error)) = &self.refused {
            // semgrep's errors quote the rule, which may come from a project.
            let font = egui::TextStyle::Body.resolve(ui.style());
            ui.label(visible::job(error, font, ERROR_COLOR));
            let anyway = ui.button("Save Anyway").on_hover_text(
                "Save it as it is: until it is fixed, semgrep runs that use it stop with this \
                 error",
            );
            if anyway.clicked() {
                action = Some(EditorAction::Save(target.clone(), yaml.clone()));
            }
        }
        if action.is_some() {
            self.refused = None;
        }
        if self.check.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking the rule with semgrep…");
            });
        }
        if self.test.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Running semgrep…");
            });
        }
        action
    }

    /// The places the test matched; the one clicked.
    fn match_list(&self, ui: &mut egui::Ui, graph: &CodeGraph) -> Option<EditorAction> {
        let matches = match self.matches.as_ref()? {
            Ok(matches) => matches,
            Err(error) => {
                // A tool's error output may quote the project's files.
                let font = egui::TextStyle::Body.resolve(ui.style());
                ui.label(visible::job(error, font, ERROR_COLOR));
                return None;
            }
        };
        ui.separator();
        ui.label(match matches.len() {
            0 => "No matches. Loosen the pattern, or try Any values.".to_string(),
            1 => "1 match:".to_string(),
            n if n > MAX_LISTED => format!("{n} matches; the first {MAX_LISTED}:"),
            n => format!("{n} matches:"),
        });
        let row_height = ui.spacing().interact_size.y;
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let color = ui.visuals().text_color();
        let shown = &matches[..matches.len().min(MAX_LISTED)];
        egui::ScrollArea::vertical()
            .id_salt("rule_matches")
            .auto_shrink([false, false])
            .show_rows(ui, row_height, shown.len(), |ui, range| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                let mut picked = None;
                for found in &shown[range] {
                    let place = format!("{}:{}  ", found.file, found.line);
                    let mut label = visible::job(&place, font.clone(), color);
                    let code = line_text(graph, &found.file, found.line).trim();
                    visible::append(&mut label, code, font.clone(), color);
                    if ui.selectable_label(false, label).clicked() {
                        picked = Some(EditorAction::GoTo(found.clone()));
                    }
                }
                picked
            })
            .inner
    }
}

/// The `id:` of the rule in `yaml`, if it has just one and that can name
/// a file.
pub fn rule_id_in(yaml: &str) -> Option<&str> {
    let mut ids = yaml.lines().filter_map(|line| {
        line.trim_start()
            .trim_start_matches("- ")
            .strip_prefix("id:")
    });
    let id = ids.next()?;
    if ids.next().is_some() {
        // Several rules: the file isn't named after the first.
        return None;
    }
    let id = id.trim().trim_matches(|c| c == '"' || c == '\'');
    let is_plain = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    is_plain.then_some(id)
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;
    use crate::index::graph_of_files;

    struct State {
        editor: RuleEditor,
        graph: CodeGraph,
        actions: Vec<EditorAction>,
    }

    fn harness(editor: RuleEditor) -> Harness<'static, State> {
        let graph = graph_of_files(&[("a.c", "int x;\n")]);
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
            |ui, state: &mut State| {
                let saves = SaveButton::new_file("fallback");
                if let Some(action) = state.editor.show(ui, &state.graph, &saves) {
                    state.actions.push(action);
                }
            },
            State {
                editor,
                graph,
                actions: Vec::new(),
            },
        );
        harness.run();
        harness
    }

    #[test]
    fn saving_asks_for_a_check_first() {
        let mut harness = harness(RuleEditor::new("rules: []\n".into()));
        harness.get_by_label("Save for All Projects").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            [EditorAction::Check(
                SaveTarget::New(RuleHome::Yours, "fallback".into()),
                "rules: []\n".into()
            )]
        );
    }

    #[test]
    fn a_refused_rule_says_why_and_can_be_saved_anyway() {
        let mut harness = harness(RuleEditor::new("bad".into()));
        let target = SaveTarget::New(RuleHome::Project, "x".into());
        harness.state_mut().editor.check_failed(
            target.clone(),
            "bad".into(),
            "semgrep couldn't use the rules: Invalid rule schema".into(),
        );
        harness.run();
        harness.get_by_label("semgrep couldn't use the rules: Invalid rule schema");
        harness.get_by_label("Save Anyway").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            [EditorAction::Save(target, "bad".into())]
        );
        assert!(harness.state().editor.refusal().is_none());
    }

    #[test]
    fn a_read_only_rule_can_be_tested_not_saved() {
        let mut harness = harness(RuleEditor::read_only("rules: []\n".into()));
        assert!(harness.query_by_label("Save for This Project").is_none());
        harness.get_by_label("Test").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            [EditorAction::Test("rules: []\n".into())]
        );
    }

    #[test]
    fn a_new_rule_in_place_forgets_the_old_ones_matches() {
        let mut editor = RuleEditor::new("a".into());
        editor.set_matches(Ok(Vec::new()));
        editor.set_saved(Ok("Saved".into()));
        editor.set_yaml("b".into());
        assert!(editor.test_error().is_none());
        assert!(editor.matches.is_none());
        assert!(editor.saved_text().is_none());
    }

    #[test]
    fn the_rules_id_names_its_file_if_it_can() {
        assert_eq!(rule_id_in("rules:\n  - id: my-rule\n"), Some("my-rule"));
        assert_eq!(rule_id_in("rules:\n  - id: \"q\"\n"), Some("q"));
        assert_eq!(rule_id_in("rules:\n  - id: ../../evil\n"), None);
        assert_eq!(rule_id_in("nothing"), None);
        // A file of several rules isn't named after the first.
        assert_eq!(rule_id_in("rules:\n  - id: a\n    x: 1\n  - id: b\n"), None);
    }
}

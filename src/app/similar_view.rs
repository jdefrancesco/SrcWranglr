//! The "Find More Like This" window: a semgrep rule drafted from a line of
//! code, to edit, test on the project, and save for the project or for all
//! of them. Testing lists every place the rule matches.

use egui::RichText;

use super::findings::line_text;
use super::rule_files::RuleHome;
use super::survey_view::ERROR_COLOR;
use super::tool_job::{with_symbols, ToolJob};
use super::visible::{self, visible};
use crate::analysis::similar::{drafts, Looseness, RuleDraft};
use crate::analysis::Finding;
use crate::model::CodeGraph;

const DEFAULT_SIZE: [f32; 2] = [620.0, 520.0];
const RULE_ROWS: usize = 8;
/// Matches listed at most; the count says if there were more.
const MAX_LISTED: usize = 2_000;

/// What the user asked of the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimilarAction {
    /// Run the rule (this text) on the project.
    Test(String),
    /// Save the rule (this id and text) there.
    Save(RuleHome, String, String),
    /// Show this match in the source.
    GoTo(Finding),
}

pub struct SimilarView {
    file: String,
    line: u32,
    looseness: Looseness,
    /// The drafts, in `Looseness` order (or why there are none). The
    /// drafted id names the file if the edited rule's can't.
    drafts: Result<[RuleDraft; 2], String>,
    /// The rule, as edited.
    yaml: String,
    test: Option<ToolJob>,
    matches: Option<Result<Vec<Finding>, String>>,
    /// What the last save did.
    saved: Option<Result<String, String>>,
}

impl SimilarView {
    /// The window for line `line` of `file`, with a rule drafted from it.
    pub fn open(graph: &CodeGraph, file: &str, line: u32) -> Self {
        let source = graph.file_contents.get(file).map_or("", String::as_str);
        let drafts = drafts(source, file, line);
        let yaml = drafts
            .as_ref()
            .map(|d| d[0].yaml.clone())
            .unwrap_or_default();
        Self {
            file: file.to_string(),
            line,
            looseness: Looseness::default(),
            drafts,
            yaml,
            test: None,
            matches: None,
            saved: None,
        }
    }

    /// The draft for the chosen looseness.
    fn drafted(&self) -> Option<&RuleDraft> {
        let drafts = self.drafts.as_ref().ok()?;
        Some(match self.looseness {
            Looseness::SameShape => &drafts[0],
            Looseness::AnyValues => &drafts[1],
        })
    }

    /// Starts showing the results of `job`, a run of the rule.
    pub fn start_test(&mut self, job: ToolJob) {
        self.test = Some(job);
        self.matches = None;
    }

    pub fn test_failed(&mut self, error: String) {
        self.matches = Some(Err(error));
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

    /// Draws the window; `None` once it is closed, else what was asked.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        graph: &CodeGraph,
    ) -> Option<Option<SimilarAction>> {
        self.poll_test(graph);
        let mut is_open = true;
        let mut action = None;
        egui::Window::new("Find More Like This")
            .id(egui::Id::unique("find_more_like_this"))
            .open(&mut is_open)
            .default_size(DEFAULT_SIZE)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| action = self.contents(ui, graph));
        is_open.then_some(action)
    }

    fn poll_test(&mut self, graph: &CodeGraph) {
        let Some(result) = self.test.as_ref().and_then(ToolJob::poll) else {
            return;
        };
        self.test = None;
        self.matches = Some(result.map(|found| {
            let mut found = with_symbols(found, graph);
            found.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
            found
        }));
    }

    fn contents(&mut self, ui: &mut egui::Ui, graph: &CodeGraph) -> Option<SimilarAction> {
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let code = line_text(graph, &self.file, self.line).trim();
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("From {}:{}", visible(&self.file), self.line));
            ui.label(visible::job(code, font, ui.visuals().text_color()));
        });
        if let Err(error) = &self.drafts {
            ui.label(RichText::new(error).color(ERROR_COLOR));
            return None;
        }
        self.looseness_picker(ui);
        let title = ui.label("Rule (semgrep YAML; edit it as you like):");
        ui.add(
            egui::TextEdit::multiline(&mut self.yaml)
                .code_editor()
                .desired_rows(RULE_ROWS)
                .desired_width(f32::INFINITY),
        )
        .labelled_by(title.id);
        let action = self.buttons(ui);
        self.status(ui);
        action.or_else(|| self.match_list(ui, graph))
    }

    fn looseness_picker(&mut self, ui: &mut egui::Ui) {
        let is_edited = self.drafted().is_some_and(|d| d.yaml != self.yaml);
        let before = self.looseness;
        ui.horizontal(|ui| {
            ui.label("Match:");
            for looseness in Looseness::ALL {
                ui.radio_value(&mut self.looseness, looseness, looseness.label());
            }
            if is_edited {
                ui.weak("(picking one replaces your edits)");
            }
        });
        if self.looseness != before {
            if let Some(yaml) = self.drafted().map(|d| d.yaml.clone()) {
                self.yaml = yaml;
            }
        }
    }

    fn buttons(&mut self, ui: &mut egui::Ui) -> Option<SimilarAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            let is_testing = self.test.is_some();
            let test = ui.add_enabled(!is_testing, egui::Button::new("Test"));
            if test
                .on_hover_text("Run this rule on the project and list what it matches")
                .clicked()
            {
                action = Some(SimilarAction::Test(self.yaml.clone()));
            }
            let id = rule_id_in(&self.yaml)
                .or(self.drafted().map(|d| d.id.as_str()))
                .unwrap_or_default()
                .to_string();
            let project = ui.button("Save for This Project").on_hover_text(
                "Into the project's .srcwranglr/semgrep/: Analyze → Run semgrep uses it once you \
                 trust this project's files",
            );
            if project.clicked() {
                action = Some(SimilarAction::Save(
                    RuleHome::Project,
                    id.clone(),
                    self.yaml.clone(),
                ));
            }
            let yours = ui
                .button("Save for All Projects")
                .on_hover_text("Into your semgrep rules: every semgrep run uses it");
            if yours.clicked() {
                action = Some(SimilarAction::Save(RuleHome::Yours, id, self.yaml.clone()));
            }
        });
        action
    }

    fn status(&self, ui: &mut egui::Ui) {
        match &self.saved {
            Some(Ok(message)) => {
                ui.label(message);
            }
            Some(Err(error)) => {
                ui.label(RichText::new(error).color(ERROR_COLOR));
            }
            None => {}
        }
        if self.test.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Running semgrep…");
            });
        }
    }

    /// The places the test matched; the one clicked.
    fn match_list(&self, ui: &mut egui::Ui, graph: &CodeGraph) -> Option<SimilarAction> {
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
            0 => "No matches. Try Any values, or loosen the pattern.".to_string(),
            1 => "1 match (the line you started from counts):".to_string(),
            n if n > MAX_LISTED => format!("{n} matches; the first {MAX_LISTED}:"),
            n => format!("{n} matches (the line you started from counts):"),
        });
        let row_height = ui.spacing().interact_size.y;
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let color = ui.visuals().text_color();
        let shown = &matches[..matches.len().min(MAX_LISTED)];
        egui::ScrollArea::vertical()
            .id_salt("similar_matches")
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
                        picked = Some(SimilarAction::GoTo(found.clone()));
                    }
                }
                picked
            })
            .inner
    }
}

/// The `id:` of the first rule in `yaml`, if it is a plain name.
fn rule_id_in(yaml: &str) -> Option<&str> {
    let id = yaml
        .lines()
        .find_map(|line| {
            line.trim_start()
                .trim_start_matches("- ")
                .strip_prefix("id:")
        })?
        .trim()
        .trim_matches(|c| c == '"' || c == '\'');
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
    use crate::analysis::Severity;
    use crate::index::graph_of_files;

    struct State {
        view: SimilarView,
        graph: CodeGraph,
        actions: Vec<SimilarAction>,
    }

    fn harness(source: &str, line: u32) -> Harness<'static, State> {
        let graph = graph_of_files(&[("src/net.c", source)]);
        let view = SimilarView::open(&graph, "src/net.c", line);
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
            |ui, state: &mut State| {
                let ctx = ui.ctx().clone();
                if let Some(Some(action)) = state.view.show(&ctx, &state.graph) {
                    state.actions.push(action);
                }
            },
            State {
                view,
                graph,
                actions: Vec::new(),
            },
        );
        harness.run();
        harness
    }

    const SOURCE: &str = "void f(char *b, char *p, int n) {\n  memcpy(b, p, ntohs(n));\n}\n";

    #[test]
    fn drafts_a_rule_from_the_line_and_loosens_it_on_request() {
        let mut harness = harness(SOURCE, 2);
        harness.get_by_label("From src/net.c:2");
        assert!(harness
            .state()
            .view
            .yaml
            .contains("memcpy($A, $B, ntohs($C))"));
        harness.get_by_label("Any values").click();
        harness.run();
        assert!(harness
            .state()
            .view
            .yaml
            .contains("memcpy($A, $B, ntohs(...))"));
    }

    #[test]
    fn test_and_save_hand_over_the_rule_as_edited() {
        let mut harness = harness(SOURCE, 2);
        harness.state_mut().view.yaml = harness
            .state()
            .view
            .yaml
            .replace("like-memcpy-2", "my-rule");
        harness.run();
        harness.get_by_label("Test").click();
        harness.run();
        harness.get_by_label("Save for This Project").click();
        harness.run();
        let actions = &harness.state().actions;
        assert!(matches!(&actions[0], SimilarAction::Test(yaml) if yaml.contains("my-rule")));
        assert!(matches!(
            &actions[1],
            SimilarAction::Save(RuleHome::Project, id, _) if id == "my-rule"
        ));
    }

    #[test]
    fn matches_are_listed_and_picking_one_goes_there() {
        let mut harness = harness(SOURCE, 2);
        let found = Finding {
            tool: "semgrep".into(),
            rule: "like-memcpy-2".into(),
            severity: Severity::Medium,
            message: String::new(),
            fix: None,
            cwe: None,
            file: "src/net.c".into(),
            line: 2,
            columns: Some((2, 24)),
            symbol: None,
        };
        harness.state_mut().view.matches = Some(Ok(vec![found.clone()]));
        harness.run();
        harness.get_by_label("1 match (the line you started from counts):");
        harness
            .get_by_label("src/net.c:2  memcpy(b, p, ntohs(n));")
            .click();
        harness.run();
        assert_eq!(harness.state().actions, [SimilarAction::GoTo(found)]);
    }

    #[test]
    fn a_line_without_code_says_so() {
        let harness = harness("int x;\n\n", 2);
        harness.get_by_label("No call or statement starts on this line.");
        assert!(harness.query_by_label("Test").is_none());
    }

    #[test]
    fn the_edited_rules_id_names_its_file_if_it_can() {
        assert_eq!(rule_id_in("rules:\n  - id: my-rule\n"), Some("my-rule"));
        assert_eq!(rule_id_in("rules:\n  - id: \"q\"\n"), Some("q"));
        assert_eq!(rule_id_in("rules:\n  - id: ../../evil\n"), None);
        assert_eq!(rule_id_in("nothing"), None);
    }
}

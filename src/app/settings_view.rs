//! The Settings window: which editor Open in Editor uses, the folders
//! tools are looked for in (with where each tool was found), the source
//! color scheme, and the symbol list's font. Each change is used and saved
//! as it is made; typed text once you leave its field.

use std::path::PathBuf;

use egui::{Id, RichText};

use super::dialogs::{self, DialogState, PendingDialog};
use super::fonts::{BarFont, SymbolFont};
use super::settings::{AgentSettings, Settings, DEFAULT_AGENT_BASE_URL};
use crate::analysis::tools::Tool;
use crate::code_view::scheme::{show_sample, Scheme};
use crate::programs::SearchPath;

/// Room the window's heading, footer and margins take, besides the
/// sections.
const MODAL_CHROME: f32 = 160.0;
/// The sections get at least this much room, however short the screen.
const MIN_SECTIONS_HEIGHT: f32 = 240.0;
const WIDTH: f32 = 520.0;
const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x6C, 0x6C);
const TOOLS: [Tool; 3] = [Tool::Semgrep, Tool::Cppcheck, Tool::Flawfinder];
/// Editors offered by name, with the command each is run as.
const EDITORS: &[(&str, &str)] = &[
    ("VS Code", "code"),
    ("Cursor", "cursor"),
    ("Zed", "zed"),
    ("Sublime Text", "subl"),
    ("Emacs", "emacs"),
    ("CLion", "clion"),
    ("Vim, in Terminal", "vim"),
    ("Neovim, in Terminal", "nvim"),
    ("Helix, in Terminal", "hx"),
];
const AUTOMATIC: &str = "Automatic: SRCWRANGLR_EDITOR, $VISUAL, $EDITOR, or VS Code";
const CUSTOM: &str = "Custom command…";
/// Shown in the font picked for the symbol list.
const FONT_SAMPLE: &str = "★ png_read_chunk  [function]   Parser::next_token  [method]";

/// What the user did in the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsAction {
    /// Use and save these now; the window stays open.
    Apply(Settings),
    /// Install this missing analyzer (its Install button was clicked).
    Install(Tool),
    /// The window was closed: the settings as left (to save if they
    /// changed), or why they can't be saved.
    Close(Result<Settings, String>),
}

/// How the editor is picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorChoice {
    Automatic,
    /// One of `EDITORS`, by index.
    Named(usize),
    Custom,
}

/// Settings being edited; each valid change is applied as it is made.
pub struct SettingsView {
    editor: EditorChoice,
    custom: String,
    /// One folder per line.
    tool_dirs: String,
    scheme: Scheme,
    symbol_font: SymbolFont,
    bar_font: BarFont,
    /// The model SrcWranglr's own agent uses, its API key, and endpoint.
    agent_model: String,
    agent_key: String,
    agent_base_url: String,
    /// The settings in use; edits that differ from them are applied.
    applied: Settings,
    /// Why the last change couldn't be saved, or a folder couldn't be
    /// picked.
    error: Option<String>,
    /// Whether a text field is being typed in: its edits wait until you
    /// leave it, so a half-typed folder isn't saved.
    is_typing: bool,
    folder_dialog: Option<PendingDialog<PathBuf>>,
    /// Where each tool was found, for the folders as last typed.
    found: Option<Found>,
    /// A tool whose Install button was clicked this frame, for `show` to
    /// return.
    install_requested: Option<Tool>,
    /// A tool being installed now (set by the app), so its row shows progress.
    installing: Option<Tool>,
}

/// Where the tools and the editor's program were found, for `key` (the
/// folders and the program as typed).
#[derive(Debug, Clone)]
struct Found {
    key: String,
    tools: Vec<Option<PathBuf>>,
    editor: Option<(String, Option<PathBuf>)>,
    managers: super::tool_install::Managers,
}

impl SettingsView {
    pub fn new(settings: &Settings) -> Self {
        let named = |command: &str| EDITORS.iter().position(|(_, c)| *c == command);
        let (editor, custom) = match settings.editor.as_deref() {
            None => (EditorChoice::Automatic, String::new()),
            Some(command) => match named(command) {
                Some(index) => (EditorChoice::Named(index), String::new()),
                None => (EditorChoice::Custom, command.to_string()),
            },
        };
        let tool_dirs = settings
            .tool_dirs
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        Self {
            editor,
            custom,
            tool_dirs,
            scheme: settings.scheme,
            symbol_font: settings.symbol_font,
            bar_font: settings.bar_font,
            agent_model: settings.agent.model.clone(),
            agent_key: settings.agent.api_key.clone(),
            agent_base_url: settings.agent.base_url.clone(),
            applied: settings.clone(),
            error: None,
            is_typing: false,
            folder_dialog: None,
            found: None,
            install_requested: None,
            installing: None,
        }
    }

    /// Shows why the last change couldn't be saved.
    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    /// Marks the tool being installed now, so its row shows progress.
    pub fn set_installing(&mut self, tool: Option<Tool>) {
        self.installing = tool;
    }

    /// Re-detects the tools next frame (after an install finishes).
    pub fn refresh_tools(&mut self) {
        self.found = None;
    }

    /// The settings as edited, if they are valid.
    fn draft(&self) -> Result<Settings, String> {
        let editor = match self.editor {
            EditorChoice::Automatic => None,
            EditorChoice::Named(index) => EDITORS.get(index).map(|(_, c)| c.to_string()),
            EditorChoice::Custom => Some(self.custom.trim().to_string()).filter(|c| !c.is_empty()),
        };
        let tool_dirs = self
            .tool_dirs
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect();
        let settings = Settings {
            editor,
            tool_dirs,
            scheme: self.scheme,
            symbol_font: self.symbol_font,
            bar_font: self.bar_font,
            agent: AgentSettings {
                base_url: match self.agent_base_url.trim() {
                    "" => DEFAULT_AGENT_BASE_URL.to_string(),
                    url => url.to_string(),
                },
                model: self.agent_model.trim().to_string(),
                api_key: self.agent_key.trim().to_string(),
            },
        };
        settings.check()?;
        Ok(settings)
    }

    /// The edits to apply now: valid, different from what is in use, and
    /// not still being typed.
    fn change_to_apply(&mut self) -> Option<Settings> {
        if self.is_typing {
            return None;
        }
        let settings = self.draft().ok().filter(|s| *s != self.applied)?;
        self.applied = settings.clone();
        self.error = None;
        Some(settings)
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<SettingsAction> {
        self.poll_folder_dialog();
        let response = egui::Modal::new(Id::unique("settings_view")).show(ctx, |ui| {
            ui.set_width(WIDTH);
            ui.heading("Settings");
            ui.separator();
            // The sections scroll on a short screen; Done stays in view.
            let room = (ctx.content_rect().height() - MODAL_CHROME).max(MIN_SECTIONS_HEIGHT);
            let is_typing = egui::ScrollArea::vertical()
                .max_height(room)
                .min_scrolled_height(room)
                .show(ui, |ui| {
                    let is_typing_command = self.editor_section(ui);
                    ui.separator();
                    let is_typing_folders = self.tools_section(ui, ctx);
                    ui.separator();
                    let is_typing_agent = self.agent_section(ui);
                    ui.separator();
                    self.colors_section(ui);
                    ui.separator();
                    self.font_section(ui);
                    is_typing_command || is_typing_folders || is_typing_agent
                })
                .inner;
            ui.separator();
            self.is_typing = is_typing;
            // Before the footer, so it doesn't show an error this clears.
            let change = self.change_to_apply();
            (change, self.footer(ui))
        });
        let should_close = response.should_close();
        let (change, is_done) = response.inner;
        if let Some(tool) = self.install_requested.take() {
            return Some(SettingsAction::Install(tool));
        }
        if is_done || should_close {
            return Some(SettingsAction::Close(self.draft()));
        }
        change.map(SettingsAction::Apply)
    }

    /// Returns whether the custom command is being typed.
    fn editor_section(&mut self, ui: &mut egui::Ui) -> bool {
        ui.strong("Editor");
        let caption = ui.label("Open in Editor uses:");
        let label = match self.editor {
            EditorChoice::Automatic => AUTOMATIC,
            EditorChoice::Named(index) => EDITORS.get(index).map_or(AUTOMATIC, |(l, _)| l),
            EditorChoice::Custom => CUSTOM,
        };
        egui::ComboBox::from_id_salt("settings_editor")
            .selected_text(label)
            .width(WIDTH - 40.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.editor, EditorChoice::Automatic, AUTOMATIC);
                for (index, (name, _)) in EDITORS.iter().enumerate() {
                    ui.selectable_value(&mut self.editor, EditorChoice::Named(index), *name);
                }
                ui.selectable_value(&mut self.editor, EditorChoice::Custom, CUSTOM);
            })
            .response
            .labelled_by(caption.id);
        if self.editor != EditorChoice::Custom {
            return false;
        }
        let field = egui::TextEdit::singleline(&mut self.custom)
            .hint_text("code -g {file}:{line}:{column}")
            .desired_width(f32::INFINITY);
        let is_typing = ui.add(field).has_focus();
        ui.weak(
            "{file}, {line} and {column} are filled in. Without {file} it is a program \
             and its options, and the file and line are added the way that editor takes them.",
        );
        is_typing
    }

    /// Returns whether the folders are being typed.
    fn tools_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
        ui.strong("Tools");
        ui.label(
            "Folders to look for semgrep, cppcheck, flawfinder and your editor in, one per line:",
        );
        let field = egui::TextEdit::multiline(&mut self.tool_dirs)
            .hint_text("/opt/homebrew/bin")
            .desired_rows(3)
            .desired_width(f32::INFINITY);
        let is_typing = ui.add(field).has_focus();
        ui.horizontal(|ui| {
            let can_pick = self.folder_dialog.is_none();
            if ui
                .add_enabled(can_pick, egui::Button::new("Add Folder…"))
                .clicked()
            {
                self.pick_folder(ctx);
            }
            ui.weak("Then PATH, then Homebrew's, ~/.local/bin and the other usual places.");
        });
        let Found {
            tools,
            editor,
            managers,
            ..
        } = self.found_tools();
        let installing = self.installing;
        let mut requested = None;
        egui::Grid::new("settings_tools")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for (tool, found) in TOOLS.iter().zip(tools) {
                    ui.label(tool.name());
                    match found {
                        Some(path) => {
                            ui.monospace(path.display().to_string());
                        }
                        None if installing == Some(*tool) => {
                            ui.add(egui::Spinner::new());
                        }
                        None => {
                            if let Some(tool) = missing_tool_row(ui, *tool, &managers) {
                                requested = Some(tool);
                            }
                        }
                    };
                    ui.end_row();
                }
                if let Some((program, found)) = editor {
                    ui.label(program);
                    match found {
                        Some(path) => ui.monospace(path.display().to_string()),
                        None => ui.label(RichText::new("not found").color(ERROR_COLOR)),
                    };
                    ui.end_row();
                }
            });
        if requested.is_some() {
            self.install_requested = requested;
        }
        is_typing
    }
}

/// A not-found tool's row: an **Install** button when SrcWranglr can run a
/// no-sudo package manager, else the command to run with a **Copy** button.
/// Returns the tool when its Install button is clicked.
fn missing_tool_row(
    ui: &mut egui::Ui,
    tool: Tool,
    managers: &super::tool_install::Managers,
) -> Option<Tool> {
    use super::tool_install::{plan, InstallPlan};
    let mut requested = None;
    ui.horizontal(|ui| match plan(tool, managers) {
        InstallPlan::Run { program, .. } => {
            ui.label(RichText::new("not found").color(ERROR_COLOR));
            if ui
                .button("Install")
                .on_hover_text(format!(
                    "Runs `{program} install {}` in the background",
                    tool.name()
                ))
                .clicked()
            {
                requested = Some(tool);
            }
        }
        InstallPlan::Manual(command) => {
            ui.label(RichText::new(format!("not found: {command}")).color(ERROR_COLOR));
            if ui
                .button("Copy")
                .on_hover_text("Copy the install command")
                .clicked()
            {
                ui.ctx().copy_text(command);
            }
        }
    });
    requested
}

impl SettingsView {
    /// Where each tool (and the editor's program) is, for the folders as
    /// typed; looked up again only when they change.
    fn found_tools(&mut self) -> Found {
        let program = match self.editor {
            EditorChoice::Automatic => None,
            EditorChoice::Named(index) => EDITORS.get(index).map(|(_, c)| c.to_string()),
            EditorChoice::Custom => self.custom.split_whitespace().next().map(String::from),
        };
        let key = format!(
            "{}\u{0}{}",
            self.tool_dirs,
            program.as_deref().unwrap_or("")
        );
        if let Some(found) = self.found.as_ref().filter(|f| f.key == key) {
            return found.clone();
        }
        let dirs: Vec<PathBuf> = self
            .tool_dirs
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect();
        let search = SearchPath::from_process(&dirs);
        let found = Found {
            key,
            tools: TOOLS.iter().map(|t| search.find(t.name())).collect(),
            editor: program.map(|p| {
                let path = search.find(&p);
                (p, path)
            }),
            managers: super::tool_install::Managers::found(&search),
        };
        self.found = Some(found.clone());
        found
    }

    /// The in-app agent's model, API key and endpoint, under a collapsing
    /// header. Returns whether a field is being typed in (so a half-typed
    /// key isn't saved yet).
    fn agent_section(&mut self, ui: &mut egui::Ui) -> bool {
        let field =
            |ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str, secret: bool| {
                let mut focused = false;
                ui.horizontal(|ui| {
                    ui.label(label);
                    let edit = egui::TextEdit::singleline(text)
                        .password(secret)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY);
                    focused = ui.add(edit).has_focus();
                });
                focused
            };
        egui::CollapsingHeader::new("Agent")
            .show(ui, |ui| {
                ui.label("The model SrcWranglr's own agent uses (any OpenAI-compatible endpoint).");
                let mut typing = false;
                typing |= field(ui, "Model:", &mut self.agent_model, "glm-5.3", false);
                typing |= field(ui, "API key:", &mut self.agent_key, "pasted here", true);
                typing |= field(
                    ui,
                    "Endpoint:",
                    &mut self.agent_base_url,
                    DEFAULT_AGENT_BASE_URL,
                    false,
                );
                ui.weak(
                    "The key is kept in plain text in settings.toml. Leave the endpoint blank \
                     for z.ai. Then start a run with 'Run in SrcWranglr' on the Agent pane's \
                     Assessment tab.",
                );
                typing
            })
            .body_returned
            .unwrap_or(false)
    }

    fn colors_section(&mut self, ui: &mut egui::Ui) {
        let caption = ui.strong("Source colors");
        egui::ComboBox::from_id_salt("settings_scheme")
            .selected_text(self.scheme.label())
            .show_ui(ui, |ui| {
                for scheme in Scheme::ALL {
                    ui.selectable_value(&mut self.scheme, scheme, scheme.label());
                }
            })
            .response
            .labelled_by(caption.id);
        show_sample(ui, &self.scheme.colors());
    }

    fn font_section(&mut self, ui: &mut egui::Ui) {
        let caption = ui.strong("Symbol list font");
        egui::ComboBox::from_id_salt("settings_symbol_font")
            .selected_text(self.symbol_font.label())
            .show_ui(ui, |ui| {
                for font in SymbolFont::ALL {
                    let label = RichText::new(font.label()).family(font.family());
                    ui.selectable_value(&mut self.symbol_font, font, label);
                }
            })
            .response
            .labelled_by(caption.id);
        ui.label(RichText::new(FONT_SAMPLE).family(self.symbol_font.family()));
        ui.add_space(6.0);
        let caption = ui.strong("Top bar font");
        egui::ComboBox::from_id_salt("settings_bar_font")
            .selected_text(self.bar_font.label())
            .show_ui(ui, |ui| {
                for font in BarFont::ALL {
                    ui.selectable_value(&mut self.bar_font, font, font.text(font.label()));
                }
            })
            .response
            .labelled_by(caption.id);
        ui.horizontal(|ui| {
            for title in ["File", "Go", "View", "Analyze"] {
                ui.label(self.bar_font.text(title));
            }
            ui.label(self.bar_font.control("Symbols"));
            ui.label(self.bar_font.control("Source"));
        });
    }

    /// Why the settings as edited can't be saved, and Done. Returns
    /// whether Done was clicked.
    fn footer(&mut self, ui: &mut egui::Ui) -> bool {
        let error = match self.draft() {
            Err(error) => Some(format!("Not saved: {error}")),
            Ok(_) => self.error.clone(),
        };
        if let Some(error) = error {
            ui.label(RichText::new(error).color(ERROR_COLOR));
        }
        ui.horizontal(|ui| {
            let is_done = ui.button("Done").clicked();
            ui.weak("Changes are saved as you make them.");
            is_done
        })
        .inner
    }

    fn pick_folder(&mut self, ctx: &egui::Context) {
        let dialog = dialogs::spawn(ctx, || {
            rfd::FileDialog::new()
                .set_title("A folder to look for tools in")
                .pick_folder()
        });
        match dialog {
            Ok(dialog) => self.folder_dialog = Some(dialog),
            Err(error) => self.error = Some(error),
        }
    }

    fn poll_folder_dialog(&mut self) {
        let Some(state) = self.folder_dialog.as_ref().map(PendingDialog::poll) else {
            return;
        };
        if state != DialogState::Open {
            self.folder_dialog = None;
        }
        match state {
            DialogState::Picked(folder) => {
                if !self.tool_dirs.trim().is_empty() {
                    self.tool_dirs.push('\n');
                }
                self.tool_dirs.push_str(&folder.to_string_lossy());
            }
            DialogState::Failed(error) => self.error = Some(error),
            DialogState::Open | DialogState::Cancelled => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;

    struct State {
        view: SettingsView,
        actions: Vec<SettingsAction>,
    }

    fn harness_for(view: SettingsView) -> Harness<'static, State> {
        // The fonts the app installs at launch, which the font picker uses.
        let mut harness = crate::app::fonts::test_harness()
            .with_size(egui::vec2(900.0, 1500.0))
            .build_ui_state(
                |ui, state: &mut State| {
                    let ctx = ui.ctx().clone();
                    if let Some(action) = state.view.show(&ctx) {
                        state.actions.push(action);
                    }
                },
                State {
                    view,
                    actions: Vec::new(),
                },
            );
        harness.run();
        harness
    }

    fn harness(settings: &Settings) -> Harness<'static, State> {
        harness_for(SettingsView::new(settings))
    }

    #[test]
    fn shows_each_section_and_where_the_tools_are() {
        let harness = harness(&Settings::default());
        harness.get_by_label("Editor");
        harness.get_by_label("Tools");
        harness.get_by_label("Source colors");
        harness.get_by_label("Symbol list font");
        for tool in ["semgrep", "cppcheck", "flawfinder"] {
            harness.get_by_label(tool);
        }
        harness.get_by_label("Open in Editor uses:");
    }

    #[test]
    fn a_missing_tool_offers_install_when_a_manager_is_present() {
        struct S {
            requested: Option<Tool>,
        }
        let managers = crate::app::tool_install::Managers {
            brew: true,
            pipx: false,
            pip: None,
        };
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
            move |ui, s: &mut S| {
                if let Some(tool) = missing_tool_row(ui, Tool::Semgrep, &managers) {
                    s.requested = Some(tool);
                }
            },
            S { requested: None },
        );
        harness.run();
        harness.get_by_label("Install").click();
        harness.run();
        assert_eq!(harness.state().requested, Some(Tool::Semgrep));
    }

    #[test]
    fn a_missing_tool_without_a_manager_shows_copy_not_install() {
        let managers = crate::app::tool_install::Managers {
            brew: false,
            pipx: false,
            pip: None,
        };
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
            move |ui, _s: &mut ()| {
                missing_tool_row(ui, Tool::Cppcheck, &managers);
            },
            (),
        );
        harness.run();
        assert!(harness.query_by_label("Install").is_none());
        assert!(harness.query_by_label("Copy").is_some());
    }

    #[test]
    fn nothing_is_applied_until_something_changes() {
        let mut harness = harness(&Settings::default());
        harness.run_steps(3);
        assert_eq!(harness.state().actions, []);
    }

    #[test]
    fn picking_a_font_applies_it_at_once() {
        let mut harness = harness(&Settings::default());
        harness
            .get_by_role_and_label(egui::Role::ComboBox, "Symbol list font")
            .click();
        harness.run();
        harness.get_by_label("Iosevka").click();
        harness.run();
        let expected = Settings {
            symbol_font: SymbolFont::Iosevka,
            ..Settings::default()
        };
        assert_eq!(harness.state().actions, [SettingsAction::Apply(expected)]);
    }

    #[test]
    fn typed_text_is_applied_once_you_leave_the_field() {
        let custom = Settings {
            editor: Some("subl".into()),
            ..Settings::default()
        };
        let mut view = SettingsView::new(&custom);
        view.editor = EditorChoice::Custom;
        view.custom = "subl".into();
        let mut harness = harness_for(view);
        harness.get_by_role(egui::Role::TextInput).focus();
        harness.run();
        harness
            .get_by_role(egui::Role::TextInput)
            .type_text(" --wait");
        harness.run();
        assert_eq!(harness.state().actions, [], "applied while typing");
        harness.key_press(egui::Key::Enter);
        harness.run();
        let expected = Settings {
            editor: Some("subl --wait".into()),
            ..Settings::default()
        };
        assert_eq!(harness.state().actions, [SettingsAction::Apply(expected)]);
    }

    #[test]
    fn done_closes_with_the_settings_as_left() {
        let settings = Settings {
            editor: Some("zed".into()),
            tool_dirs: vec!["/opt/tools".into()],
            scheme: Scheme::Monokai,
            symbol_font: SymbolFont::SourceCodePro,
            bar_font: BarFont::Iosevka,
            agent: crate::app::settings::AgentSettings::default(),
        };
        let mut harness = harness(&settings);
        harness.get_by_label("Done").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            [SettingsAction::Close(Ok(settings))]
        );
    }

    #[test]
    fn an_invalid_folder_is_shown_and_not_applied() {
        let view = SettingsView {
            tool_dirs: "/ok\nnot/absolute".into(),
            ..SettingsView::new(&Settings::default())
        };
        let mut harness = harness_for(view);
        harness.run_steps(2);
        assert_eq!(harness.state().actions, []);
        assert!(harness.query_by_label_contains("full path").is_some());
        harness.get_by_label("Done").click();
        harness.run();
        let closed = harness.state().actions.first().cloned();
        assert!(
            matches!(closed, Some(SettingsAction::Close(Err(ref e))) if e.contains("full path")),
            "{closed:?}"
        );
    }

    #[test]
    fn a_save_error_is_shown_until_the_next_change_applies() {
        let mut harness = harness(&Settings::default());
        harness
            .state_mut()
            .view
            .set_error("Could not save settings.toml: disk full".into());
        harness.run();
        assert!(harness.query_by_label_contains("disk full").is_some());
        harness.state_mut().view.scheme = Scheme::Monokai;
        harness.run();
        assert!(harness.query_by_label_contains("disk full").is_none());
    }

    #[test]
    fn a_custom_command_is_kept_and_a_relative_folder_refused() {
        let custom = Settings {
            editor: Some("subl --wait {file}:{line}".into()),
            ..Settings::default()
        };
        let view = SettingsView::new(&custom);
        assert_eq!(view.editor, EditorChoice::Custom);
        assert_eq!(view.draft(), Ok(custom));
        let relative = SettingsView {
            tool_dirs: "/ok\nnot/absolute".into(),
            ..SettingsView::new(&Settings::default())
        };
        assert!(relative.draft().is_err_and(|e| e.contains("full path")));
    }

    #[test]
    fn the_agent_fields_become_the_saved_provider() {
        let mut view = SettingsView::new(&Settings::default());
        view.agent_model = "glm-4.6".into();
        view.agent_key = "secret".into();
        let settings = view.draft().expect("valid");
        assert_eq!(settings.agent.model, "glm-4.6");
        assert_eq!(settings.agent.api_key, "secret");
        assert_eq!(settings.agent.base_url, DEFAULT_AGENT_BASE_URL);
        assert!(settings.agent.provider().is_some(), "ready to run");
    }
}

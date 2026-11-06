//! The Settings window: which editor Open in Editor uses, the folders
//! tools are looked for in (with where each tool was found), and the
//! source color scheme.

use std::path::PathBuf;

use egui::{Id, RichText};

use super::dialogs::{self, DialogState, PendingDialog};
use super::settings::Settings;
use crate::analysis::tools::Tool;
use crate::code_view::scheme::{show_sample, Scheme};
use crate::programs::SearchPath;

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

/// What the user did in the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsAction {
    Save(Settings),
    Cancel,
}

/// How the editor is picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorChoice {
    Automatic,
    /// One of `EDITORS`, by index.
    Named(usize),
    Custom,
}

/// Settings being edited; nothing changes until Save.
pub struct SettingsView {
    editor: EditorChoice,
    custom: String,
    /// One folder per line.
    tool_dirs: String,
    scheme: Scheme,
    error: Option<String>,
    folder_dialog: Option<PendingDialog<PathBuf>>,
    /// Where each tool was found, for the folders as last typed.
    found: Option<Found>,
}

/// Where the tools and the editor's program were found, for `key` (the
/// folders and the program as typed).
#[derive(Debug, Clone)]
struct Found {
    key: String,
    tools: Vec<Option<PathBuf>>,
    editor: Option<(String, Option<PathBuf>)>,
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
            error: None,
            folder_dialog: None,
            found: None,
        }
    }

    /// Shows why the last Save didn't go through.
    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
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
        };
        settings.check()?;
        Ok(settings)
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<SettingsAction> {
        self.poll_folder_dialog();
        let response = egui::Modal::new(Id::unique("settings_view")).show(ctx, |ui| {
            ui.set_width(WIDTH);
            ui.heading("Settings");
            ui.separator();
            self.editor_section(ui);
            ui.separator();
            self.tools_section(ui, ctx);
            ui.separator();
            self.colors_section(ui);
            ui.separator();
            self.buttons(ui)
        });
        let should_close = response.should_close();
        response
            .inner
            .or_else(|| should_close.then_some(SettingsAction::Cancel))
    }

    fn editor_section(&mut self, ui: &mut egui::Ui) {
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
        if self.editor == EditorChoice::Custom {
            let field = egui::TextEdit::singleline(&mut self.custom)
                .hint_text("code -g {file}:{line}:{column}")
                .desired_width(f32::INFINITY);
            ui.add(field);
            ui.weak(
                "{file}, {line} and {column} are filled in. Without {file} it is a program \
                 and its options, and the file and line are added the way that editor takes them.",
            );
        }
    }

    fn tools_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.strong("Tools");
        ui.label(
            "Folders to look for semgrep, cppcheck, flawfinder and your editor in, one per line:",
        );
        let field = egui::TextEdit::multiline(&mut self.tool_dirs)
            .hint_text("/opt/homebrew/bin")
            .desired_rows(3)
            .desired_width(f32::INFINITY);
        ui.add(field);
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
        let Found { tools, editor, .. } = self.found_tools();
        egui::Grid::new("settings_tools")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for (tool, found) in TOOLS.iter().zip(tools) {
                    ui.label(tool.name());
                    match found {
                        Some(path) => ui.monospace(path.display().to_string()),
                        None => ui.label(
                            RichText::new(format!("not found: {}", tool.install_hint()))
                                .color(ERROR_COLOR),
                        ),
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
    }

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
        };
        self.found = Some(found.clone());
        found
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

    fn buttons(&mut self, ui: &mut egui::Ui) -> Option<SettingsAction> {
        if let Some(error) = &self.error {
            ui.label(RichText::new(error).color(ERROR_COLOR));
        }
        let mut action = None;
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                match self.draft() {
                    Ok(settings) => action = Some(SettingsAction::Save(settings)),
                    Err(error) => self.error = Some(error),
                }
            }
            if ui.button("Cancel").clicked() {
                action = Some(SettingsAction::Cancel);
            }
        });
        action
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
        action: Option<SettingsAction>,
    }

    fn harness(settings: &Settings) -> Harness<'static, State> {
        let mut harness = Harness::new_ui_state(
            |ui, state: &mut State| {
                let ctx = ui.ctx().clone();
                if let Some(action) = state.view.show(&ctx) {
                    state.action = Some(action);
                }
            },
            State {
                view: SettingsView::new(settings),
                action: None,
            },
        );
        harness.run();
        harness
    }

    #[test]
    fn shows_each_section_and_where_the_tools_are() {
        let harness = harness(&Settings::default());
        harness.get_by_label("Editor");
        harness.get_by_label("Tools");
        harness.get_by_label("Source colors");
        for tool in ["semgrep", "cppcheck", "flawfinder"] {
            harness.get_by_label(tool);
        }
        harness.get_by_label("Open in Editor uses:");
    }

    #[test]
    fn saving_returns_the_settings_as_edited() {
        let settings = Settings {
            editor: Some("zed".into()),
            tool_dirs: vec!["/opt/tools".into()],
            scheme: Scheme::Monokai,
        };
        let mut harness = harness(&settings);
        harness.get_by_label("Save").click();
        harness.run();
        assert_eq!(harness.state().action, Some(SettingsAction::Save(settings)));
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
    fn cancel_changes_nothing() {
        let mut harness = harness(&Settings::default());
        harness.get_by_label("Cancel").click();
        harness.run();
        assert_eq!(harness.state().action, Some(SettingsAction::Cancel));
    }
}

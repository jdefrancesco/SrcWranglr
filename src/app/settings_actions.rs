//! What the app does with your settings: reading them at launch, and
//! using and saving each change made in the Settings window.

use super::settings::{settings_path, Settings};
use super::settings_view::{SettingsAction, SettingsView};
use super::tool_install::{self, InstallJob, InstallPlan, Managers};
use super::{SrcWranglrApp, Status};
use crate::analysis::tools::Tool;

impl SrcWranglrApp {
    /// Reads your settings, recovering what it can: a single bad key is
    /// degraded to its default (and reported) rather than reverting every
    /// setting — so one stale key can't quietly reset your colors. Only an
    /// unreadable file falls back to the defaults wholesale.
    pub(super) fn load_settings(&mut self) {
        let Some(path) = settings_path() else {
            return;
        };
        match Settings::load_recovering(&path) {
            Ok((settings, warnings)) => {
                self.use_settings(settings);
                if !warnings.is_empty() {
                    self.status = Status::Error(format!(
                        "Some settings weren't read ({}): {}",
                        path.display(),
                        warnings.join("; ")
                    ));
                }
            }
            Err(error) => {
                self.status = Status::Error(format!("Settings not read, using defaults: {error}"));
            }
        }
    }

    pub(super) fn open_settings(&mut self) {
        self.settings_view = Some(SettingsView::new(&self.settings));
    }

    pub(super) fn show_settings_view(&mut self, ctx: &egui::Context) {
        let Some(action) = self.settings_view.as_mut().and_then(|v| v.show(ctx)) else {
            return;
        };
        match action {
            SettingsAction::Apply(settings) => {
                if let Err(error) = self.save_settings(settings) {
                    if let Some(view) = self.settings_view.as_mut() {
                        view.set_error(error);
                    }
                }
            }
            SettingsAction::Install(tool) => self.install_tool(tool, ctx),
            SettingsAction::Close(left) => {
                self.settings_view = None;
                self.close_settings(left);
            }
        }
    }

    /// Starts installing a missing analyzer in the background, if one isn't
    /// already running and SrcWranglr can run a package manager for it.
    fn install_tool(&mut self, tool: Tool, ctx: &egui::Context) {
        if self.install_job.is_some() {
            self.status = Status::Info("An install is already running.".into());
            return;
        }
        let search = self.settings.search_path();
        match tool_install::plan(tool, &Managers::found(&search)) {
            plan @ InstallPlan::Run { .. } => {
                let job = InstallJob::spawn(tool, plan, &search, ctx);
                self.status = Status::Info(job.status_line());
                if let Some(view) = self.settings_view.as_mut() {
                    view.set_installing(Some(tool));
                }
                self.install_job = Some(job);
            }
            InstallPlan::Manual(command) => {
                self.status =
                    Status::Info(format!("Run this to install {}: {command}", tool.name()));
            }
        }
    }

    /// On opening a project, if analyzers are missing, adds a non-blocking
    /// note to the status bar pointing at Settings, where they can be
    /// installed. Never installs on its own.
    pub(super) fn offer_tool_install_on_open(&mut self) {
        let search = self.settings.search_path();
        let missing: Vec<&str> = [Tool::Semgrep, Tool::Cppcheck, Tool::Flawfinder]
            .into_iter()
            .filter(|t| search.find(t.name()).is_none())
            .map(|t| t.name())
            .collect();
        if missing.is_empty() {
            return;
        }
        let note = format!(
            " {} not found — ⌘, / Ctrl+, to install.",
            missing.join(", ")
        );
        if let Status::Info(message) = &mut self.status {
            message.push_str(&note);
        } else {
            self.status = Status::Info(note.trim_start().to_string());
        }
    }

    /// Advances a running install: keeps its progress in the status bar, and
    /// on finish reports the result and re-checks the tools.
    pub(super) fn poll_install(&mut self) {
        let Some(job) = self.install_job.as_ref() else {
            return;
        };
        let tool = job.tool();
        let Some(result) = job.poll() else {
            self.status = Status::Info(job.status_line());
            return;
        };
        self.install_job = None;
        if let Some(view) = self.settings_view.as_mut() {
            view.set_installing(None);
            view.refresh_tools();
        }
        self.status = match result {
            Ok(()) => Status::Info(format!("{} installed.", tool.name())),
            Err(error) => Status::Error(format!(
                "Couldn't install {}: {}",
                tool.name(),
                error.lines().next().unwrap_or(&error)
            )),
        };
    }

    /// Saves what the Settings window was closed with, if it wasn't yet
    /// (typed in a field, then closed), or says why it can't be.
    fn close_settings(&mut self, left: Result<Settings, String>) {
        match left {
            Ok(settings) if settings == self.settings => {}
            Ok(settings) => {
                if let Err(error) = self.save_settings(settings) {
                    self.status = Status::Error(error);
                }
            }
            Err(error) => {
                self.status = Status::Error(format!("Settings not saved: {error}"));
            }
        }
    }

    /// Saves `settings` to your settings file and uses them.
    fn save_settings(&mut self, settings: Settings) -> Result<(), String> {
        let path = settings_path().ok_or("No home folder to keep settings in ($HOME is unset)")?;
        let saved = settings.save_to(&path)?;
        self.use_settings(settings);
        self.status = Status::Info(format!("Settings saved to {}.", saved.display()));
        Ok(())
    }

    fn use_settings(&mut self, settings: Settings) {
        self.code_panel.set_scheme(settings.scheme);
        self.symbol_list.set_font(settings.symbol_font);
        self.bar_font = settings.bar_font;
        self.settings = settings;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::fonts::SymbolFont;
    use crate::app::launch::LaunchOptions;
    use crate::code_view::scheme::Scheme;

    fn app() -> SrcWranglrApp {
        SrcWranglrApp::new(&egui::Context::default(), None, LaunchOptions::default())
    }

    #[test]
    fn using_settings_recolors_the_source_sets_the_font_and_keeps_them() {
        let mut app = app();
        let settings = Settings {
            scheme: Scheme::PaperLight,
            symbol_font: SymbolFont::SourceCodePro,
            ..Settings::default()
        };
        app.use_settings(settings.clone());
        assert_eq!(app.settings, settings);
        assert_eq!(app.symbol_list.font(), SymbolFont::SourceCodePro);
        app.open_settings();
        assert!(app.settings_view.is_some());
    }

    #[test]
    fn chosen_colors_survive_a_restart() {
        let mut first = app();
        let settings = Settings {
            scheme: Scheme::Monokai,
            ..Settings::default()
        };
        first.save_settings(settings).expect("saved");
        // A fresh app reads the same config path (test config_dir is per-pid).
        let mut fresh = app();
        fresh.load_settings();
        assert_eq!(fresh.settings.scheme, Scheme::Monokai, "colors reverted");
        if let Status::Error(error) = &fresh.status {
            panic!("load errored: {error}");
        }
    }

    #[test]
    fn closing_with_nothing_new_saves_nothing() {
        let mut app = app();
        app.status = Status::Info("untouched".into());
        app.close_settings(Ok(app.settings.clone()));
        assert!(matches!(&app.status, Status::Info(text) if text == "untouched"));
    }

    #[test]
    fn closing_with_settings_that_cannot_be_saved_says_why() {
        let mut app = app();
        app.close_settings(Err("tool folder bin isn't a full path".into()));
        assert!(matches!(&app.status, Status::Error(text) if text.contains("Settings not saved")),);
    }
}

//! What the app does with your settings: reading them at launch, and
//! using and saving each change made in the Settings window.

use super::settings::{settings_path, Settings};
use super::settings_view::{SettingsAction, SettingsView};
use super::{SrcWranglrApp, Status};

impl SrcWranglrApp {
    /// Reads your settings; a bad file is reported and the defaults used.
    pub(super) fn load_settings(&mut self) {
        let loaded = settings_path().map(|path| Settings::load_from(&path));
        match loaded {
            Some(Ok(settings)) => self.use_settings(settings),
            Some(Err(error)) => {
                self.status = Status::Error(format!("Settings not read, using defaults: {error}"));
            }
            None => {}
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
            SettingsAction::Close(left) => {
                self.settings_view = None;
                self.close_settings(left);
            }
        }
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

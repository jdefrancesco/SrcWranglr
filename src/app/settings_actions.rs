//! What the app does with your settings: reading them at launch, the
//! Settings window, and applying what it saves.

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
            SettingsAction::Cancel => self.settings_view = None,
            SettingsAction::Save(settings) => match self.save_settings(settings) {
                Ok(()) => self.settings_view = None,
                Err(error) => {
                    if let Some(view) = self.settings_view.as_mut() {
                        view.set_error(error);
                    }
                }
            },
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
        self.settings = settings;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::launch::LaunchOptions;
    use crate::code_view::scheme::Scheme;

    #[test]
    fn using_settings_recolors_the_source_and_keeps_them() {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let settings = Settings {
            scheme: Scheme::PaperLight,
            ..Settings::default()
        };
        app.use_settings(settings.clone());
        assert_eq!(app.settings, settings);
        app.open_settings();
        assert!(app.settings_view.is_some());
    }
}

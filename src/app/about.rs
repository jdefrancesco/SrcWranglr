//! Help > About SrcWranglr: which version this is.

use egui::RichText;

use super::APP_NAME;
use crate::version::version_line;

/// Where the project lives, from Cargo.toml.
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const LICENSE: &str = env!("CARGO_PKG_LICENSE");
const NAME_SIZE: f32 = 24.0;
const GAP: f32 = 8.0;

/// Shows the About window while `is_open`; its close buttons clear it.
pub fn show(ctx: &egui::Context, is_open: &mut bool) {
    if !*is_open {
        return;
    }
    let mut is_shown = true;
    let close_clicked = egui::Window::new(format!("About {APP_NAME}"))
        .open(&mut is_shown)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(APP_NAME).size(NAME_SIZE).strong());
                ui.label(format!("Version {}", version_line()));
                ui.add_space(GAP);
                ui.label("Explore and audit C/C++ source code.");
                ui.hyperlink(REPOSITORY);
                ui.weak(format!("{LICENSE} License"));
                ui.add_space(GAP);
                ui.button("Close").clicked()
            })
            .inner
        })
        .and_then(|window| window.inner)
        .unwrap_or(false);
    *is_open = is_shown && !close_clicked;
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    fn window() -> egui_kittest::Harness<'static, bool> {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(800.0, 600.0))
            .build_ui_state(
                |ui, is_open: &mut bool| {
                    let ctx = ui.ctx().clone();
                    show(&ctx, is_open);
                },
                true,
            );
        harness.run();
        harness
    }

    #[test]
    fn shows_the_version_and_where_the_project_lives() {
        let harness = window();
        let version = crate::version::version_line();
        assert!(
            harness.query_by_label_contains(&version).is_some(),
            "{version}"
        );
        assert!(harness.query_by_label_contains(REPOSITORY).is_some());
    }

    #[test]
    fn closing_it_clears_the_flag() {
        let mut harness = window();
        harness.get_by_label("Close").click();
        harness.run();
        assert!(!*harness.state());
    }

    #[test]
    fn a_closed_window_shows_nothing() {
        let mut harness = window();
        *harness.state_mut() = false;
        harness.run();
        let version = crate::version::version_line();
        assert!(harness.query_by_label_contains(&version).is_none());
    }
}

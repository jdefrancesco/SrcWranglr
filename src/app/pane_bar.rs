//! Showing and hiding panes without shortcuts: a toggle per pane in the
//! menu bar, after the project's name, and a hide arrow in each pane's
//! heading. Side panes slide in and out, and dragging one shut hides it
//! too.

use egui::{Align, Layout, RichText};

use super::fonts;
use super::palette::Command;
use super::{SrcWranglrApp, FINDINGS_SHORTCUT, NOTES_PANEL_SHORTCUT};

/// Room between two toggles.
const TOGGLE_GAP: f32 = 6.0;
/// Room around a toggle's label, inside its pill.
const TOGGLE_PADDING: egui::Vec2 = egui::vec2(9.0, 2.0);
/// Enough to round a toggle's ends fully.
const TOGGLE_ROUNDING: u8 = 12;
/// A touch of tracking, so the toggles' labels read as buttons.
const TOGGLE_LETTER_SPACING: f32 = 0.4;

/// A pane that can be shown or hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Symbols,
    Source,
    Notes,
    Findings,
    Strings,
    History,
    Matching,
    Todos,
    Trace,
}

impl Pane {
    /// In the order the toggles are shown, left to right.
    pub const ALL: [Pane; 9] = [
        Pane::Symbols,
        Pane::Source,
        Pane::Notes,
        Pane::Findings,
        Pane::Strings,
        Pane::History,
        Pane::Matching,
        Pane::Todos,
        Pane::Trace,
    ];

    /// Its toggle's label.
    pub fn label(self) -> &'static str {
        match self {
            Pane::Symbols => "Symbols",
            Pane::Source => "Source",
            Pane::Notes => "Notes",
            Pane::Findings => "Findings",
            Pane::Strings => "Strings",
            Pane::History => "History",
            Pane::Matching => "Matching",
            Pane::Todos => "TODO",
            Pane::Trace => "Trace",
        }
    }

    /// What it shows, for menus, the palette and tooltips.
    pub fn title(self) -> &'static str {
        match self {
            Pane::Symbols => "Symbol List",
            Pane::Source => "Source Pane",
            Pane::Notes => "Notes & Bookmarks",
            Pane::Findings => "Findings (risky calls)",
            Pane::Strings => "Strings in the Source",
            Pane::History => "Git History (fix commits)",
            Pane::Matching => "Semantic Code Matching",
            Pane::Todos => "TODO List",
            Pane::Trace => "Trace Pane",
        }
    }
}

/// The window edge a pane sits at, which its hide arrow points to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Bottom,
}

impl Edge {
    fn arrow(self) -> &'static str {
        match self {
            Edge::Left => "◂",
            Edge::Right => "▸",
            Edge::Bottom => "▾",
        }
    }
}

/// A pane at the right of the window, `width` wide until dragged, to show
/// with `show_collapsible` so it slides in and out. Hidden, it leaves no
/// handle to drag it open: the hidden panes would share the window's edge,
/// so a drag there couldn't tell which one to open. Its toggle does.
pub fn right_pane(id_salt: impl egui::AsIdSalt, width: f32) -> egui::Panel {
    egui::Panel::right(id_salt)
        .resizable(true)
        .default_size(width)
        .drag_to_open(false)
}

/// A small arrow that hides the pane at `edge`. Returns whether it was
/// clicked.
pub fn hide_button(ui: &mut egui::Ui, edge: Edge) -> bool {
    ui.small_button(edge.arrow())
        .on_hover_text("Hide this pane (its button in the menu bar brings it back)")
        .clicked()
}

/// A pane's heading with its hide arrow at the right. Returns whether the
/// arrow was clicked.
pub fn heading(ui: &mut egui::Ui, title: impl Into<RichText>, edge: Edge) -> bool {
    ui.horizontal(|ui| {
        ui.heading(title);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            hide_button(ui, edge)
        })
        .inner
    })
    .inner
}

/// A pill per pane, left to right in `ui`, lit for the ones shown. `hint`
/// is each one's tooltip. Returns the one clicked.
pub fn toggles(
    ui: &mut egui::Ui,
    is_shown: impl Fn(Pane) -> bool,
    hint: impl Fn(Pane) -> String,
) -> Option<Pane> {
    ui.scope(|ui| {
        // Buttons as elsewhere in the window, not as menu titles: the menu
        // bar hides the background of buttons that aren't lit.
        ui.visuals_mut().widgets = ui.ctx().global_style().visuals.widgets.clone();
        let spacing = ui.spacing_mut();
        spacing.item_spacing.x = TOGGLE_GAP;
        spacing.button_padding = TOGGLE_PADDING;
        Pane::ALL.into_iter().fold(None, |clicked, pane| {
            let toggle = toggle(ui, pane, is_shown(pane)).on_hover_text(hint(pane));
            match toggle.clicked() {
                true => Some(pane),
                false => clicked,
            }
        })
    })
    .inner
}

/// One pane's pill: blue when the pane is shown, gray when hidden.
fn toggle(ui: &mut egui::Ui, pane: Pane, is_lit: bool) -> egui::Response {
    let label = RichText::new(pane.label())
        .family(fonts::medium())
        .extra_letter_spacing(TOGGLE_LETTER_SPACING);
    let pill = egui::Button::selectable(is_lit, label)
        .frame_when_inactive(true)
        .corner_radius(TOGGLE_ROUNDING);
    ui.add(pill)
}

impl SrcWranglrApp {
    pub(super) fn is_pane_shown(&self, pane: Pane) -> bool {
        match pane {
            Pane::Symbols => self.layout.is_symbols_shown,
            Pane::Source => self.layout.is_source_shown,
            Pane::Notes => self.notes_panel.is_open(),
            Pane::Findings => self.findings_panel.is_open(),
            Pane::Strings => self.strings_panel.is_open(),
            Pane::History => self.history_panel.is_open(),
            Pane::Matching => self.matching_panel.is_open(),
            Pane::Todos => self.todo_panel.is_open(),
            Pane::Trace => self.trace_panel.is_open(),
        }
    }

    /// Shows `pane` if hidden, hides it if shown.
    pub(super) fn toggle_pane(&mut self, pane: Pane) {
        let is_shown = !self.is_pane_shown(pane);
        match pane {
            Pane::Symbols => self.layout = self.layout.with_symbols_shown(is_shown),
            Pane::Source => self.layout = self.layout.with_source_shown(is_shown),
            Pane::Notes => self.notes_panel.set_open(is_shown),
            Pane::Findings => self.findings_panel.set_open(is_shown),
            Pane::Strings => self.strings_panel.set_open(is_shown),
            Pane::History => self.history_panel.set_open(is_shown),
            Pane::Matching => self.matching_panel.set_open(is_shown),
            Pane::Todos => self.todo_panel.set_open(is_shown),
            Pane::Trace => self.trace_panel.set_open(is_shown),
        }
    }

    /// The toggles in the menu bar, while a project is open.
    pub(super) fn show_pane_toggles(&self, ui: &mut egui::Ui) -> Option<Command> {
        if !self.is_project_open {
            return None;
        }
        let ctx = ui.ctx().clone();
        let hint = |pane: Pane| {
            let shortcut = match pane {
                Pane::Notes => Some(&NOTES_PANEL_SHORTCUT),
                Pane::Findings => Some(&FINDINGS_SHORTCUT),
                _ => None,
            };
            match shortcut {
                Some(keys) => format!(
                    "Show or hide: {} ({})",
                    pane.title(),
                    ctx.format_shortcut(keys)
                ),
                None => format!("Show or hide: {}", pane.title()),
            }
        };
        toggles(ui, |pane| self.is_pane_shown(pane), hint).map(Command::TogglePane)
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::{NodeT, Queryable};
    use egui_kittest::Harness;

    use super::*;
    use crate::app::launch::LaunchOptions;

    #[test]
    fn a_heading_reports_its_hide_arrow() {
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
            |ui, is_hidden: &mut bool| {
                if heading(ui, "Findings (3)", Edge::Right) {
                    *is_hidden = true;
                }
            },
            false,
        );
        harness.run();
        harness.get_by_label("Findings (3)");
        harness.get_by_label("▸").click();
        harness.run();
        assert!(*harness.state());
    }

    #[test]
    fn toggles_are_lit_for_shown_panes_and_report_clicks() {
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
            |ui, clicked: &mut Option<Pane>| {
                let shown = |pane| pane == Pane::Source;
                if let Some(pane) = toggles(ui, shown, |p| p.title().to_string()) {
                    *clicked = Some(pane);
                }
            },
            None,
        );
        harness.run();
        for pane in Pane::ALL {
            let toggle = harness.get_by_label(pane.label());
            let is_lit = toggle.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True);
            assert_eq!(is_lit, pane == Pane::Source, "{pane:?}");
        }
        harness.get_by_label("TODO").click();
        harness.run();
        assert_eq!(*harness.state(), Some(Pane::Todos));
    }

    fn demo_app() -> (SrcWranglrApp, egui::Context) {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        app.open_demo(&ctx);
        (app, ctx)
    }

    #[test]
    fn every_pane_can_be_shown_and_hidden_again() {
        let (mut app, ctx) = demo_app();
        for pane in Pane::ALL {
            let was_shown = app.is_pane_shown(pane);
            app.run_command(Command::TogglePane(pane), &ctx);
            assert_eq!(app.is_pane_shown(pane), !was_shown, "{pane:?}");
            app.run_command(Command::TogglePane(pane), &ctx);
            assert_eq!(app.is_pane_shown(pane), was_shown, "{pane:?}");
        }
    }

    #[test]
    fn the_symbol_list_and_source_start_shown_and_the_rest_hidden() {
        let (app, _) = demo_app();
        let shown: Vec<Pane> = Pane::ALL
            .into_iter()
            .filter(|&pane| app.is_pane_shown(pane))
            .collect();
        assert_eq!(shown, [Pane::Symbols, Pane::Source]);
    }

    #[test]
    fn going_to_a_line_brings_back_a_hidden_source_pane() {
        let (mut app, ctx) = demo_app();
        app.run_command(Command::TogglePane(Pane::Source), &ctx);
        assert!(!app.is_pane_shown(Pane::Source));
        app.go_to_place(None, "main.cpp", 8);
        assert!(app.is_pane_shown(Pane::Source));
    }

    #[test]
    fn hidden_side_panes_leave_no_drag_handle_at_the_window_edge() {
        let mut harness = window();
        // The last pixels of the 1400-wide window, margin included.
        for x in 1380..=1400 {
            harness.hover_at(egui::pos2(x as f32, 450.0));
            harness.run();
            let cursor = harness.output().platform_output.cursor_icon;
            let is_resize = matches!(
                cursor,
                egui::CursorIcon::ResizeHorizontal
                    | egui::CursorIcon::ResizeEast
                    | egui::CursorIcon::ResizeWest
                    | egui::CursorIcon::ResizeColumn
            );
            assert!(!is_resize, "a drag handle at x = {x}: {cursor:?}");
        }
    }

    /// The menu bar and panes of the demo project.
    fn window() -> Harness<'static, SrcWranglrApp> {
        let (app, _) = demo_app();
        let mut harness = crate::app::fonts::test_harness()
            .with_size(egui::vec2(1400.0, 900.0))
            .build_ui_state(
                |ui, app: &mut SrcWranglrApp| {
                    let ctx = ui.ctx().clone();
                    if let Some(command) = app.show_menu_bar(ui) {
                        app.run_command(command, &ctx);
                    }
                    app.show_project_panes(ui);
                },
                app,
            );
        harness.run();
        harness
    }

    #[test]
    fn the_todo_pane_opens_from_its_toggle_and_slides_shut_from_its_arrow() {
        let mut harness = window();
        assert!(harness.query_by_label_contains("TODO (").is_none());
        harness.get_by_label("TODO").click();
        harness.run();
        assert!(harness.state().todo_panel.is_open());
        let heading = harness.get_by_label_contains("TODO (").rect();
        // Its arrow is the one on the heading's row.
        let arrow = harness
            .query_all_by_label("▸")
            .find(|a| (a.rect().center().y - heading.center().y).abs() < heading.height())
            .expect("the TODO pane's hide arrow");
        arrow.click();
        harness.run();
        assert!(!harness.state().todo_panel.is_open());
        assert!(harness.query_by_label_contains("TODO (").is_none());
    }

    #[test]
    fn the_toggles_follow_the_project_name_left_to_right_with_room_between() {
        let harness = window();
        let name = harness.state().project.name.clone();
        let name_right = harness
            .query_all_by_label(&name)
            .map(|node| node.rect())
            .find(|rect| rect.top() < 30.0)
            .expect("the project's name in the menu bar")
            .right();
        let pills: Vec<egui::Rect> = Pane::ALL
            .into_iter()
            .map(|pane| harness.get_by_label(pane.label()).rect())
            .collect();
        assert!(
            pills[0].left() > name_right,
            "Symbols starts after the name"
        );
        for pair in pills.windows(2) {
            assert!(pair[1].left() - pair[0].right() >= TOGGLE_GAP - 0.5);
        }
        // Right after the menus and the name, not pushed to the far right.
        assert!(pills[0].left() - name_right < 40.0, "{:?}", pills[0]);
    }

    #[test]
    fn hiding_the_symbol_list_from_its_toggle_and_back() {
        let mut harness = window();
        harness.get_by_label_contains("Symbols (");
        harness.get_by_label("Symbols").click();
        harness.run();
        assert!(harness.query_by_label_contains("Symbols (").is_none());
        harness.get_by_label("Symbols").click();
        harness.run();
        harness.get_by_label_contains("Symbols (");
    }
}

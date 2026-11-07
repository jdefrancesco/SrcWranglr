//! The main window's layout: where the source pane sits (beside the graph
//! or below it) and how much of the room it takes, whether the symbol list
//! and source pane are shown, plus a way to reset pane sizes. Kept between
//! launches, along with the window's size and the other panes' sizes
//! (eframe and egui remember those).
//!
//! The source pane's size is a share of the room it has with the graph,
//! not a pixel size, so it follows the window (including one that is
//! maximized after the first frames) and always leaves the graph some room.

use serde::{Deserialize, Serialize};

/// The source pane's share of the room it has with the graph: where it
/// starts, and the bounds dragging it can't pass.
const DEFAULT_SHARE: f32 = 0.45;
const MIN_SHARE: f32 = 0.15;
const MAX_SHARE: f32 = 0.75;
/// The source pane is at least this big while the room allows.
const MIN_SOURCE_SIZE: f32 = 240.0;
/// A pane size this close to the one asked for wasn't dragged.
const DRAG_TOLERANCE: f32 = 0.5;
/// Layouts saved before this had the source pane below the graph by
/// default; they are moved beside it once.
const BESIDE_BY_DEFAULT_VERSION: u32 = 1;

/// Where the source pane sits relative to the graph.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourcePlacement {
    /// Under the graph, the full width of the room beside the symbol list.
    Below,
    /// To the right of the graph, the full height.
    #[default]
    Beside,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub source: SourcePlacement,
    /// The source pane's share of the room when below the graph.
    pub below_share: f32,
    /// Its share when beside the graph.
    pub beside_share: f32,
    /// Part of the other panes' ids: Reset Layout bumps it, so they forget
    /// the sizes they were dragged to and start at their defaults again.
    pub generation: u32,
    pub is_symbols_shown: bool,
    pub is_source_shown: bool,
    /// Which defaults the layout was saved under: 0 for one saved before
    /// layouts had a version (missing in the file).
    #[serde(default)]
    version: u32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            source: SourcePlacement::default(),
            below_share: DEFAULT_SHARE,
            beside_share: DEFAULT_SHARE,
            generation: 0,
            is_symbols_shown: true,
            is_source_shown: true,
            version: BESIDE_BY_DEFAULT_VERSION,
        }
    }
}

impl Layout {
    /// Where the layout is kept in eframe's storage.
    pub const STORAGE_KEY: &'static str = "layout";

    /// The layout saved in `storage` (its shares kept within bounds), or
    /// the default.
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        storage
            .and_then(|s| eframe::get_value::<Self>(s, Self::STORAGE_KEY))
            .map(Self::sanitized)
            .map(Self::migrated)
            .unwrap_or_default()
    }

    /// A layout saved when the source pane sat below the graph by default,
    /// moved beside it (the default now). Later layouts are kept as saved.
    fn migrated(self) -> Self {
        if self.version >= BESIDE_BY_DEFAULT_VERSION {
            return self;
        }
        Self {
            source: SourcePlacement::Beside,
            version: BESIDE_BY_DEFAULT_VERSION,
            ..self
        }
    }

    /// Keeps the layout in `storage` for the next launch.
    pub fn save(self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, Self::STORAGE_KEY, &self);
    }

    /// The same layout with the source pane on the other side.
    pub fn swapped(self) -> Self {
        let source = match self.source {
            SourcePlacement::Below => SourcePlacement::Beside,
            SourcePlacement::Beside => SourcePlacement::Below,
        };
        Self { source, ..self }
    }

    pub fn with_symbols_shown(self, is_symbols_shown: bool) -> Self {
        Self {
            is_symbols_shown,
            ..self
        }
    }

    pub fn with_source_shown(self, is_source_shown: bool) -> Self {
        Self {
            is_source_shown,
            ..self
        }
    }

    /// The default layout, with panes forgetting their sizes.
    pub fn reset(self) -> Self {
        Self {
            generation: self.generation.wrapping_add(1),
            ..Self::default()
        }
    }

    /// The id of the pane called `name` in this layout.
    pub fn pane_id(self, name: &'static str) -> (&'static str, u32) {
        (name, self.generation)
    }

    /// Shows the source pane below or beside the graph at its share of the
    /// room left in `ui`. Returns the layout with the share the pane was
    /// dragged to (unchanged if it wasn't), and what `add_contents` did.
    pub fn show_source_pane<R>(
        self,
        ui: &mut egui::Ui,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> (Self, R) {
        let (salt, room) = match self.source {
            SourcePlacement::Below => (self.pane_id("code_panel"), ui.available_height()),
            SourcePlacement::Beside => (self.pane_id("code_panel_beside"), ui.available_width()),
        };
        // egui would keep the pane at the pixel size it last had; forget
        // it, so the pane is sized from its share again every frame. (The
        // id is the one egui gives a panel with this salt.)
        let id = ui.scope_id().with_salt(egui::IdSalt::new(salt));
        ui.data_mut(|d| d.remove::<egui::PanelState>(id));
        let panel = match self.source {
            SourcePlacement::Below => egui::Panel::bottom(salt),
            SourcePlacement::Beside => egui::Panel::right(salt),
        };
        let asked = source_size(self.share(), room);
        let shown = panel
            .resizable(true)
            .size_range(source_size(MIN_SHARE, room)..=source_size(MAX_SHARE, room))
            .default_size(asked)
            .show(ui, add_contents);
        let size = match self.source {
            SourcePlacement::Below => shown.response.rect.height(),
            SourcePlacement::Beside => shown.response.rect.width(),
        };
        let is_dragged = (size - asked).abs() > DRAG_TOLERANCE && room > 0.0;
        let layout = if is_dragged {
            self.with_share(size / room)
        } else {
            self
        };
        (layout, shown.inner)
    }

    fn share(self) -> f32 {
        match self.source {
            SourcePlacement::Below => self.below_share,
            SourcePlacement::Beside => self.beside_share,
        }
    }

    fn with_share(self, share: f32) -> Self {
        let share = bounded_share(share);
        match self.source {
            SourcePlacement::Below => Self {
                below_share: share,
                ..self
            },
            SourcePlacement::Beside => Self {
                beside_share: share,
                ..self
            },
        }
    }

    /// Shares from storage, which may have been edited, kept within bounds.
    fn sanitized(self) -> Self {
        Self {
            below_share: bounded_share(self.below_share),
            beside_share: bounded_share(self.beside_share),
            ..self
        }
    }
}

fn bounded_share(share: f32) -> f32 {
    if share.is_finite() {
        share.clamp(MIN_SHARE, MAX_SHARE)
    } else {
        DEFAULT_SHARE
    }
}

/// The source pane's size for `share` of `room`: at least
/// `MIN_SOURCE_SIZE`, but never more than `MAX_SHARE` of the room, so the
/// graph keeps some.
pub fn source_size(share: f32, room: f32) -> f32 {
    (share * room)
        .max(MIN_SOURCE_SIZE)
        .min(room * MAX_SHARE)
        .max(0.0)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use eframe::Storage;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::app::SrcWranglrApp;

    #[derive(Default)]
    struct MemoryStorage(HashMap<String, String>);

    impl eframe::Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }

        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }

        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }

        fn flush(&mut self) {}
    }

    /// The layout with the source pane under the graph.
    fn below() -> Layout {
        Layout {
            source: SourcePlacement::Below,
            ..Layout::default()
        }
    }

    #[test]
    fn the_source_pane_starts_beside_the_graph() {
        assert_eq!(Layout::load(None).source, SourcePlacement::Beside);
    }

    #[test]
    fn swapping_moves_the_source_pane_and_back() {
        let layout = Layout::default();
        assert_eq!(layout.swapped().source, SourcePlacement::Below);
        assert_eq!(layout.swapped().swapped(), layout);
    }

    #[test]
    fn resetting_puts_it_back_beside_with_fresh_pane_ids() {
        let moved = Layout::default().swapped();
        let reset = moved.reset();
        assert_eq!(reset.source, SourcePlacement::Beside);
        assert_ne!(reset.pane_id("code_panel"), moved.pane_id("code_panel"));
    }

    #[test]
    fn a_layout_saved_when_below_was_the_default_moves_beside_once() {
        let mut storage = MemoryStorage::default();
        storage.set_string(
            Layout::STORAGE_KEY,
            "(source: Below, below_share: 0.5, beside_share: 0.45, generation: 2)".into(),
        );
        let moved = Layout::load(Some(&storage));
        assert_eq!(moved.source, SourcePlacement::Beside);
        assert_eq!((moved.below_share, moved.generation), (0.5, 2));
        // Moved back below on purpose, it stays there.
        moved.swapped().save(&mut storage);
        assert_eq!(Layout::load(Some(&storage)).source, SourcePlacement::Below);
    }

    #[test]
    fn the_symbol_list_and_source_pane_can_be_hidden_and_reset_shows_them() {
        let layout = Layout::default();
        assert!(layout.is_symbols_shown && layout.is_source_shown);
        let hidden = layout.with_symbols_shown(false).with_source_shown(false);
        assert!(!hidden.is_symbols_shown && !hidden.is_source_shown);
        let reset = hidden.reset();
        assert!(reset.is_symbols_shown && reset.is_source_shown);
    }

    #[test]
    fn hidden_panes_are_kept_hidden_between_launches() {
        let mut storage = MemoryStorage::default();
        let layout = Layout::default().with_source_shown(false);
        layout.save(&mut storage);
        assert_eq!(Layout::load(Some(&storage)), layout);
    }

    #[test]
    fn the_layout_is_kept_between_launches() {
        let mut storage = MemoryStorage::default();
        let layout = Layout::default().reset().swapped();
        layout.save(&mut storage);
        assert_eq!(Layout::load(Some(&storage)), layout);
    }

    #[test]
    fn an_unreadable_saved_layout_falls_back_to_the_default() {
        let mut storage = MemoryStorage::default();
        storage.set_string(Layout::STORAGE_KEY, "not a layout".into());
        assert_eq!(Layout::load(Some(&storage)), Layout::default());
    }

    #[test]
    fn the_source_pane_takes_its_share_of_the_room_but_never_too_little_or_too_much() {
        assert_eq!(source_size(DEFAULT_SHARE, 1000.0), 1000.0 * DEFAULT_SHARE);
        assert_eq!(source_size(0.01, 1000.0), MIN_SOURCE_SIZE);
        // However big the share, the graph keeps some room.
        assert_eq!(source_size(1.0, 1000.0), 1000.0 * MAX_SHARE);
        assert_eq!(source_size(DEFAULT_SHARE, 200.0), 200.0 * MAX_SHARE);
    }

    #[test]
    fn saved_shares_are_kept_within_bounds() {
        let mut storage = MemoryStorage::default();
        storage.set_string(
            Layout::STORAGE_KEY,
            "(source: Beside, below_share: 5.0, beside_share: -1.0, generation: 3)".into(),
        );
        let layout = Layout::load(Some(&storage));
        assert_eq!(layout.source, SourcePlacement::Beside);
        assert_eq!(
            (layout.below_share, layout.beside_share),
            (MAX_SHARE, MIN_SHARE)
        );
        storage.set_string(Layout::STORAGE_KEY, "(below_share: NaN)".into());
        assert_eq!(Layout::load(Some(&storage)).below_share, DEFAULT_SHARE);
    }

    #[test]
    fn resetting_restores_the_starting_shares() {
        let dragged = Layout {
            below_share: 0.7,
            beside_share: 0.2,
            ..Layout::default()
        };
        let reset = dragged.reset();
        assert_eq!(
            (reset.below_share, reset.beside_share),
            (DEFAULT_SHARE, DEFAULT_SHARE)
        );
    }

    /// The panes laid out as `layout` says in a window of `size`.
    fn panes(layout: Layout, size: egui::Vec2) -> egui_kittest::Harness<'static, SrcWranglrApp> {
        let mut app = SrcWranglrApp::new(
            &egui::Context::default(),
            None,
            crate::app::launch::LaunchOptions::default(),
        );
        app.layout = layout;
        let mut harness = crate::app::fonts::test_harness()
            .with_size(size)
            .build_ui_state(
                |ui, app: &mut SrcWranglrApp| {
                    app.show_symbol_list(ui);
                    app.show_code_panel(ui);
                    egui::CentralPanel::default().show(ui, |ui| ui.label("graph"));
                },
                app,
            );
        harness.run();
        harness
    }

    fn source_pane_rect(harness: &egui_kittest::Harness<'_, SrcWranglrApp>) -> egui::Rect {
        harness
            .get_by_label("Select a symbol to view its source.")
            .rect()
    }

    /// Where the source pane is drawn in a 1200×800 window.
    fn source_pane_center(layout: Layout) -> egui::Pos2 {
        source_pane_rect(&panes(layout, egui::vec2(1200.0, 800.0))).center()
    }

    #[test]
    fn the_source_pane_follows_the_window_size() {
        let mut harness = panes(below(), egui::vec2(1200.0, 800.0));
        let small = source_pane_rect(&harness).height();
        harness.set_size(egui::vec2(1200.0, 1400.0));
        harness.run();
        let big = source_pane_rect(&harness).height();
        assert!(big > small * 1.5, "{small} -> {big}");
    }

    /// Drags the source pane's top edge (below the graph, in a 1200×800
    /// window) up to `to_y`, and returns the share it ends up with.
    fn share_after_dragging_to(to_y: f32) -> f32 {
        let mut harness = panes(below(), egui::vec2(1200.0, 800.0));
        let edge = egui::pos2(700.0, 800.0 - source_size(DEFAULT_SHARE, 800.0));
        harness.hover_at(edge);
        harness.step();
        harness.drag_at(edge);
        harness.step();
        harness.hover_at(egui::pos2(edge.x, to_y));
        harness.step();
        harness.drop_at(egui::pos2(edge.x, to_y));
        harness.run();
        harness.state().layout.below_share
    }

    #[test]
    fn dragging_the_source_pane_changes_its_share() {
        let share = share_after_dragging_to(340.0);
        assert!((share - 460.0 / 800.0).abs() < 0.02, "{share}");
    }

    #[test]
    fn the_source_pane_cannot_be_dragged_over_the_graph() {
        assert_eq!(share_after_dragging_to(20.0), 0.75);
    }

    #[test]
    fn the_source_pane_keeps_its_share_until_dragged() {
        // Small enough that the pane's minimum size wins over its share.
        let mut harness = panes(Layout::default(), egui::vec2(960.0, 420.0));
        harness.run();
        harness.set_size(egui::vec2(1200.0, 900.0));
        harness.run();
        assert_eq!(harness.state().layout, Layout::default());
    }

    #[test]
    fn the_source_pane_sits_below_or_beside_the_graph() {
        let under = source_pane_center(below());
        assert!(under.y > 500.0, "{under:?}");
        let beside = source_pane_center(Layout::default());
        assert!(beside.x > 700.0 && beside.y < 500.0, "{beside:?}");
    }

    #[test]
    fn hidden_panes_are_not_drawn() {
        let layout = Layout::default()
            .with_symbols_shown(false)
            .with_source_shown(false);
        let harness = panes(layout, egui::vec2(1200.0, 800.0));
        assert!(harness
            .query_by_label("Select a symbol to view its source.")
            .is_none());
        assert!(harness.query_by_label_contains("Symbols (").is_none());
        harness.get_by_label("graph");
    }
}

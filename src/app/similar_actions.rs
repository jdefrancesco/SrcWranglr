//! What the app does with the "Find More Like This" window: opening it on
//! code, adding places and exceptions from lines of code, and handing its
//! rule editor's actions on (see `rule_actions`).

use super::rule_actions::EditorOwner;
use super::similar_view::SimilarView;
use super::{SrcWranglrApp, Status};
use crate::analysis::similar::Example;

impl SrcWranglrApp {
    /// Opens the window with a rule drafted from `place`.
    pub(super) fn find_similar(&mut self, place: Example) {
        self.similar_view = Some(SimilarView::open(&self.project.graph, place));
    }

    /// Opens the window with a rule drafted from what `places` share.
    pub(super) fn find_similar_in(&mut self, places: Vec<Example>) {
        self.similar_view = Some(SimilarView::open_on(&self.project.graph, places));
    }

    /// Adds `place` to the rule being drafted (or starts one from it).
    pub(super) fn add_rule_place(&mut self, place: Example) {
        match self.similar_view.as_mut() {
            Some(view) => view.add_place(&self.project.graph, place),
            None => self.find_similar(place),
        }
    }

    /// Adds an exception drafted from `file:line` to the rule being
    /// drafted.
    pub(super) fn add_rule_exception(&mut self, file: &str, line: u32) {
        match self.similar_view.as_mut() {
            Some(view) => view.add_exception_from(&self.project.graph, file, line),
            None => {
                self.status = Status::Info(
                    "Draft a rule first (right-click a line → Find More Like This…), then add \
                     exceptions to it."
                        .into(),
                );
            }
        }
    }

    /// Whether a rule is being drafted, so lines can add to it.
    pub(super) fn is_drafting_rule(&self) -> bool {
        self.similar_view.is_some()
    }

    pub(super) fn show_similar_view(&mut self, ctx: &egui::Context) {
        let Some(view) = self.similar_view.as_mut() else {
            return;
        };
        match view.show(ctx, &self.project.graph) {
            None => self.similar_view = None,
            Some(Some(action)) => self.on_editor_action(EditorOwner::Similar, action, ctx),
            Some(None) => {}
        }
    }
}

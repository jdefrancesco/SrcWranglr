//! The TODO pane's "In the source" list: the `TODO`, `FIXME`, `XXX`,
//! `HACK` and `BUG` tags in the project's comments, filtered by text or
//! tag. Clicking one shows it in the source.

use egui::text::LayoutJob;
use egui::FontId;

use super::source_tags::{count, listed, SourceTagEntry, SourceTags};
use super::visible::{self, visible};
use crate::analysis::Severity;
use crate::model::{SourceTag, Span, SymbolId};
use crate::style::severity_text_color;

/// A tag picked in the list, to show in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedTag {
    pub file: String,
    pub span: Span,
    pub function: Option<SymbolId>,
}

#[derive(Default)]
pub struct SourceTagsView {
    tags: SourceTags,
    query: String,
    /// Only this tag, if set.
    only: Option<SourceTag>,
    /// Indices into the entries the filter lets through, and the filter
    /// they were made for.
    rows: Vec<usize>,
    rows_for: Option<(String, Option<SourceTag>)>,
}

impl SourceTagsView {
    /// Lists `tags` (a newly opened project's), with no filter.
    pub fn set_tags(&mut self, tags: SourceTags) {
        *self = Self {
            tags,
            ..Self::default()
        };
    }

    pub fn len(&self) -> usize {
        self.tags.entries.len()
    }

    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<PickedTag> {
        self.controls(ui);
        if !self.tags.cut_short.is_empty() {
            ui.weak(format!(
                "{} file(s) had more marked comments than were kept.",
                self.tags.cut_short.len()
            ));
        }
        ui.separator();
        self.refresh();
        if self.rows.is_empty() {
            ui.weak(if self.tags.entries.is_empty() {
                "No TODO, FIXME, XXX, HACK or BUG comments in this project."
            } else {
                "No matching comments."
            });
            return None;
        }
        self.tag_rows(ui)
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let filter = egui::TextEdit::singleline(&mut self.query)
            .hint_text("Filter comments…")
            .desired_width(f32::INFINITY);
        ui.add(filter);
        let entries = &self.tags.entries;
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.only, None, format!("All {}", entries.len()));
            for tag in SourceTag::ALL {
                let n = count(entries, tag);
                if n > 0 {
                    ui.selectable_value(&mut self.only, Some(tag), format!("{} {n}", tag.label()));
                }
            }
        });
    }

    fn refresh(&mut self) {
        let is_current = self
            .rows_for
            .as_ref()
            .is_some_and(|(query, only)| *query == self.query && *only == self.only);
        if !is_current {
            self.rows = listed(&self.tags.entries, &self.query, self.only);
            self.rows_for = Some((self.query.clone(), self.only));
        }
    }

    /// One tag per row: its file and line, then the comment.
    fn tag_rows(&self, ui: &mut egui::Ui) -> Option<PickedTag> {
        let row_height = ui.spacing().interact_size.y;
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let visuals = ui.visuals().clone();
        let (rows, entries) = (&self.rows, &self.tags.entries);
        egui::ScrollArea::vertical()
            .id_salt("source_tags_rows")
            .auto_shrink([false, false])
            .show_rows(ui, row_height, rows.len(), |ui, range| {
                // Rows must stay one line high for `show_rows` to place them.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                let mut picked = None;
                for entry in rows[range].iter().filter_map(|&i| entries.get(i)) {
                    let label = row_label(entry, font.clone(), &visuals);
                    let row = ui
                        .selectable_label(false, label)
                        .on_hover_text(place_text(entry));
                    if row.clicked() {
                        picked = Some(PickedTag {
                            file: entry.file.clone(),
                            span: entry.span,
                            function: entry.function,
                        });
                    }
                }
                picked
            })
            .inner
    }
}

/// How much a tag says something is wrong, for its color.
fn severity(tag: SourceTag) -> Severity {
    match tag {
        SourceTag::Fixme | SourceTag::Bug => Severity::High,
        SourceTag::Xxx | SourceTag::Hack => Severity::Medium,
        SourceTag::Todo => Severity::Low,
    }
}

/// `parse.c:12  FIXME: trusts the length`: the file's name and line, then
/// the comment with its tag colored; text from the project with its hidden
/// characters marked.
fn row_label(entry: &SourceTagEntry, font: FontId, visuals: &egui::Visuals) -> LayoutJob {
    let (text, weak) = (visuals.text_color(), visuals.weak_text_color());
    let name = entry.file.rsplit('/').next().unwrap_or(&entry.file);
    let mut job = visible::job(&format!("{name}:{}  ", entry.span.line), font.clone(), weak);
    let tag_length = usize::try_from(entry.span.end.saturating_sub(entry.span.start)).unwrap_or(0);
    let split = if entry.text.is_char_boundary(tag_length) {
        tag_length
    } else {
        0
    };
    let (tag, rest) = entry.text.split_at(split);
    visible::append(
        &mut job,
        tag,
        font.clone(),
        severity_text_color(severity(entry.tag), visuals),
    );
    visible::append(&mut job, rest, font, text);
    job
}

/// `src/parse.c:12 in parse` (or `at file scope`), for the row's hover.
fn place_text(entry: &SourceTagEntry) -> String {
    let place = match &entry.function_name {
        Some(name) => format!("in {}", visible(name)),
        None => "at file scope".to_string(),
    };
    format!("{}:{} {place}", visible(&entry.file), entry.span.line)
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;
    use crate::app::source_tags::source_tags;
    use crate::index::graph_of_files;

    struct State {
        view: SourceTagsView,
        picked: Option<PickedTag>,
    }

    fn harness(source: &str) -> Harness<'static, State> {
        let graph = graph_of_files(&[("src/net.c", source)]);
        let mut view = SourceTagsView::default();
        view.set_tags(source_tags(&graph));
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
            |ui, state: &mut State| {
                if let Some(picked) = state.view.show(ui) {
                    state.picked = Some(picked);
                }
            },
            State { view, picked: None },
        );
        harness.run();
        harness
    }

    #[test]
    fn lists_each_tag_with_its_place_and_counts_per_tag() {
        let harness =
            harness("// TODO: free it\nint f(void) {\n  return 0; // FIXME \u{202E}x\n}\n");
        harness.get_by_label("net.c:1  TODO: free it");
        harness.get_by_label("net.c:3  FIXME ‹U+202E›x");
        harness.get_by_label("All 2");
        harness.get_by_label("TODO 1");
        harness.get_by_label("FIXME 1");
        assert!(harness.query_by_label("HACK 0").is_none());
    }

    #[test]
    fn picking_a_tag_says_where_it_is() {
        let mut harness = harness("int f(void) {\n  return 0; // XXX racy\n}\n");
        harness.get_by_label("net.c:2  XXX racy").click();
        harness.run();
        let picked = harness.state().picked.clone().expect("picked");
        assert_eq!(picked.file, "src/net.c");
        assert_eq!((picked.span.line, picked.span.start), (2, 15));
        assert!(picked.function.is_some());
    }

    #[test]
    fn filters_by_tag_and_text() {
        let mut harness = harness("// TODO: one\n// HACK: two\n// TODO: three\n");
        harness.get_by_label("HACK 1").click();
        harness.run();
        assert!(harness.query_by_label("net.c:1  TODO: one").is_none());
        harness.get_by_label("net.c:2  HACK: two");
        harness.get_by_label("All 3").click();
        harness.run();
        harness.get_by_role(egui::Role::TextInput).focus();
        harness.run();
        harness
            .get_by_role(egui::Role::TextInput)
            .type_text("three");
        harness.run();
        assert!(harness.query_by_label("net.c:2  HACK: two").is_none());
        harness.get_by_label("net.c:3  TODO: three");
    }

    #[test]
    fn says_so_when_there_are_none() {
        let harness = harness("int f(void) { return 0; }\n");
        harness.get_by_label("No TODO, FIXME, XXX, HACK or BUG comments in this project.");
    }
}

//! The Matching pane: code found by what it is like rather than by name.
//! "Look-alike code" lists functions and blocks of the same shape, and the
//! functions most like one picked (Find Code Like This Function). "By
//! behavior" lists the functions that do every behavior picked.

use std::collections::BTreeSet;
use std::sync::Arc;

use egui::RichText;

use super::layout::Layout;
use super::matching::{
    poll, read_behaviors, read_look_alikes, unit_name, Behaviors, Like, LookAlikes, Work,
};
use super::pane_bar::{self, Edge};
use super::visible::visible;
use crate::analysis::behavior::{Behavior, MAX_CALL_DEPTH};
use crate::analysis::lookalike::{self, UnitKind};
use crate::analysis::similar::{Example, MAX_EXAMPLES};
use crate::model::{CodeGraph, SymbolId};

const DEFAULT_WIDTH: f32 = 440.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MatchingTab {
    #[default]
    LookAlikes,
    Behavior,
}

/// What the user picked in the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchingAction {
    /// Show this place in the source.
    GoTo {
        symbol: Option<SymbolId>,
        file: String,
        line: u32,
    },
    /// Draft a semgrep rule from what these places share.
    MakeRule(Vec<Example>),
}

/// The functions listed by behavior, for what they were listed for.
struct Matched {
    wanted: BTreeSet<Behavior>,
    through_calls: bool,
    filter: String,
    symbols: Arc<Vec<SymbolId>>,
}

#[derive(Default)]
pub struct MatchingPanel {
    is_open: bool,
    tab: MatchingTab,
    look_alikes: Option<Work<LookAlikes>>,
    like: Option<Like>,
    show_blocks: bool,
    group_filter: String,
    behaviors: Option<Work<Behaviors>>,
    wanted: BTreeSet<Behavior>,
    through_calls: bool,
    behavior_filter: String,
    /// The functions with every behavior wanted, until what they are for
    /// changes.
    matched: Option<Matched>,
}

impl MatchingPanel {
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn set_open(&mut self, is_open: bool) {
        self.is_open = is_open;
    }

    /// Forgets what it found (another project opened).
    pub fn reset(&mut self) {
        *self = Self {
            is_open: self.is_open,
            tab: self.tab,
            through_calls: self.through_calls,
            ..Self::default()
        };
    }

    /// Lists the functions most like `symbol`'s, reading the project's
    /// shapes first if they aren't yet.
    pub fn find_like(&mut self, symbol: SymbolId, graph: &Arc<CodeGraph>, ctx: &egui::Context) {
        self.is_open = true;
        self.tab = MatchingTab::LookAlikes;
        self.like = Some(Like::new(symbol));
        let is_read = matches!(
            self.look_alikes,
            Some(Work::Running(_)) | Some(Work::Done(_))
        );
        if !is_read {
            self.look_alikes = Some(read_look_alikes(Arc::clone(graph), ctx));
        }
        self.poll(ctx);
    }

    /// Takes in what the background work has found, and starts ranking
    /// code like a function once the shapes are there.
    pub fn poll(&mut self, ctx: &egui::Context) {
        poll(&mut self.look_alikes);
        poll(&mut self.behaviors);
        let shapes = self
            .look_alikes
            .as_ref()
            .and_then(Work::done)
            .map(|found| &found.shapes);
        if let Some(like) = self.like.as_mut() {
            like.poll(shapes, ctx);
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: &Arc<CodeGraph>,
        layout: Layout,
    ) -> Option<MatchingAction> {
        let mut is_open = self.is_open;
        let shown = pane_bar::right_pane(layout.pane_id("matching_panel"), DEFAULT_WIDTH)
            .show_collapsible(ui, &mut is_open, |ui| self.contents(ui, graph));
        self.is_open = is_open && self.is_open;
        shown.and_then(|shown| shown.inner)
    }

    fn contents(&mut self, ui: &mut egui::Ui, graph: &Arc<CodeGraph>) -> Option<MatchingAction> {
        if pane_bar::heading(ui, "Matching", Edge::Right) {
            self.is_open = false;
        }
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, MatchingTab::LookAlikes, "Look-alike code");
            ui.selectable_value(&mut self.tab, MatchingTab::Behavior, "By behavior");
        });
        ui.separator();
        // Long rows are cut, not let to widen the pane.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        match self.tab {
            MatchingTab::LookAlikes => self.look_alikes(ui, graph),
            MatchingTab::Behavior => self.by_behavior(ui, graph),
        }
    }

    fn look_alikes(&mut self, ui: &mut egui::Ui, graph: &Arc<CodeGraph>) -> Option<MatchingAction> {
        let mut action = None;
        if let Some(like) = &self.like {
            let shapes = self.look_alikes.as_ref().and_then(Work::done);
            let (cleared, picked) = like_box(ui, like, shapes.map(|f| &*f.shapes), graph);
            action = picked;
            if cleared {
                self.like = None;
            }
        }
        match self.look_alikes.as_ref() {
            None => {
                ui.label(
                    "Functions and blocks with the same shape once names, numbers, strings and \
                     layout are set aside: copies, vendored code, one pattern in many places.",
                );
                if ui.button("Find Look-alike Code").clicked() {
                    self.look_alikes = Some(read_look_alikes(Arc::clone(graph), ui.ctx()));
                }
            }
            Some(Work::Running(_)) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading every function…");
                });
            }
            Some(Work::Failed(error)) => {
                ui.label(format!("Couldn't read the code's shapes: {error}"));
                if ui.button("Try Again").clicked() {
                    self.look_alikes = Some(read_look_alikes(Arc::clone(graph), ui.ctx()));
                }
            }
            Some(Work::Done(found)) => {
                let found = Arc::clone(found);
                action = action.or(self.group_list(ui, &found, graph));
            }
        }
        action
    }

    fn group_list(
        &mut self,
        ui: &mut egui::Ui,
        found: &LookAlikes,
        graph: &CodeGraph,
    ) -> Option<MatchingAction> {
        let groups = &found.groups;
        let functions = groups
            .iter()
            .filter(|g| g.kind == UnitKind::Function)
            .count();
        ui.label(format!(
            "{functions} groups of look-alike functions, {} of blocks",
            groups.len() - functions
        ));
        if found.shapes.is_cut_short || found.is_cut_short {
            ui.weak("Not all of it is listed: some files were too large, or there was too much.");
        }
        ui.checkbox(
            &mut self.show_blocks,
            "Blocks too (loops, ifs and switches)",
        );
        filter_box(ui, &mut self.group_filter, "Filter by function or file…");
        let filter = self.group_filter.to_lowercase();
        let shapes = &found.shapes;
        let mut picked = None;
        egui::ScrollArea::vertical()
            .id_salt("lookalike_groups")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let shown = groups
                    .iter()
                    .zip(&found.headings)
                    .enumerate()
                    .filter(|(_, (g, _))| self.show_blocks || g.kind == UnitKind::Function)
                    .filter(|(_, (_, heading))| {
                        filter.is_empty() || heading.to_lowercase().contains(&filter)
                    });
                for (index, (group, heading)) in shown {
                    let rows = egui::CollapsingHeader::new(visible(heading))
                        .id_salt(("lookalike_group", index))
                        .show(ui, |ui| group_rows(ui, &group.units, shapes, graph));
                    if let Some(row) = rows.body_returned.flatten() {
                        picked = Some(row);
                    }
                }
            });
        picked
    }

    fn by_behavior(&mut self, ui: &mut egui::Ui, graph: &Arc<CodeGraph>) -> Option<MatchingAction> {
        let work = self
            .behaviors
            .get_or_insert_with(|| read_behaviors(Arc::clone(graph), ui.ctx()));
        let behaviors = match work {
            Work::Done(done) => Arc::clone(done),
            Work::Running(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Tagging functions by the calls they make…");
                });
                return None;
            }
            Work::Failed(error) => {
                ui.label(format!("Couldn't tag the functions: {error}"));
                return None;
            }
        };
        ui.checkbox(
            &mut self.through_calls,
            format!("Count what it calls too (up to {MAX_CALL_DEPTH} calls deep)"),
        );
        let counts = behaviors.counts(self.through_calls);
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for behavior in Behavior::ALL {
                let mut is_wanted = self.wanted.contains(&behavior);
                let label = format!("{} ({})", behavior.label(), counts[&behavior]);
                if ui.toggle_value(&mut is_wanted, label).changed() {
                    match is_wanted {
                        true => self.wanted.insert(behavior),
                        false => self.wanted.remove(&behavior),
                    };
                }
            }
        });
        ui.separator();
        if self.wanted.is_empty() {
            ui.weak("Pick behaviors above to list the functions that do every one of them.");
            return None;
        }
        let symbols = self.matched(&behaviors, graph);
        let names: Vec<&str> = self.wanted.iter().map(|b| b.label()).collect();
        let count = match symbols.len() {
            1 => "1 function".to_string(),
            n => format!("{n} functions"),
        };
        ui.label(format!("{count} with every one of: {}", names.join(", ")));
        filter_box(ui, &mut self.behavior_filter, "Filter by function or file…");
        behavior_rows(ui, &symbols, &self.wanted, &behaviors, graph)
    }

    /// The functions with every behavior wanted, matching the filter: the
    /// ones listed last time unless what they are for changed.
    fn matched(&mut self, behaviors: &Behaviors, graph: &CodeGraph) -> Arc<Vec<SymbolId>> {
        let is_fresh = self.matched.as_ref().is_some_and(|m| {
            m.wanted == self.wanted
                && m.through_calls == self.through_calls
                && m.filter == self.behavior_filter
        });
        if !is_fresh {
            let filter = self.behavior_filter.to_lowercase();
            let symbols = behaviors
                .matching(graph, &self.wanted, self.through_calls)
                .into_iter()
                .filter(|id| {
                    graph.symbols.get(id).is_some_and(|s| {
                        filter.is_empty()
                            || s.qualified_name.to_lowercase().contains(&filter)
                            || s.file.to_lowercase().contains(&filter)
                    })
                })
                .collect();
            self.matched = Some(Matched {
                wanted: self.wanted.clone(),
                through_calls: self.through_calls,
                filter: self.behavior_filter.clone(),
                symbols: Arc::new(symbols),
            });
        }
        self.matched
            .as_ref()
            .map_or_else(Default::default, |m| Arc::clone(&m.symbols))
    }
}

fn filter_box(ui: &mut egui::Ui, text: &mut String, hint: &str) {
    let edit = egui::TextEdit::singleline(text)
        .hint_text(hint)
        .desired_width(f32::INFINITY);
    ui.add(edit);
}

/// The functions listed by behavior, only the rows on screen laid out; a
/// row's hover says why it has each behavior.
fn behavior_rows(
    ui: &mut egui::Ui,
    symbols: &[SymbolId],
    wanted: &BTreeSet<Behavior>,
    behaviors: &Behaviors,
    graph: &CodeGraph,
) -> Option<MatchingAction> {
    let row_height = ui.spacing().interact_size.y;
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt("behavior_rows")
        .auto_shrink([false, false])
        .show_rows(ui, row_height, symbols.len(), |ui, range| {
            for id in &symbols[range] {
                let Some(symbol) = graph.symbols.get(id) else {
                    continue;
                };
                let label = format!("{}  {}:{}", symbol.qualified_name, symbol.file, symbol.line);
                let row = ui
                    .selectable_label(false, visible(&label))
                    .on_hover_ui(|ui| {
                        for &behavior in wanted {
                            let why = behaviors.why(symbol.id, behavior);
                            ui.label(visible(&format!("{}: {why}", behavior.label())));
                        }
                    });
                if row.clicked() {
                    picked = Some(MatchingAction::GoTo {
                        symbol: Some(symbol.id),
                        file: symbol.file.clone(),
                        line: symbol.line,
                    });
                }
            }
        });
    picked
}

/// "Code like parse_header:" and the functions found, likest first.
/// Whether Clear was clicked, and the place picked.
fn like_box(
    ui: &mut egui::Ui,
    like: &Like,
    shapes: Option<&lookalike::Shapes>,
    graph: &CodeGraph,
) -> (bool, Option<MatchingAction>) {
    let name = unit_name(Some(like.symbol), graph);
    let mut picked = None;
    let mut cleared = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Code like {}", visible(&name))).strong());
            cleared = ui.small_button("Clear").clicked();
        });
        let found = like.found.as_ref().and_then(Work::done);
        match (found, shapes) {
            (Some(found), Some(shapes)) if !found.is_empty() => {
                for &(index, score) in found.iter() {
                    let unit = &shapes.units[index];
                    let label = format!(
                        "{:>3.0}%  {}  {}:{}",
                        score * 100.0,
                        unit_name(unit.symbol, graph),
                        unit.file,
                        unit.first_line
                    );
                    if ui.selectable_label(false, visible(&label)).clicked() {
                        picked = Some(go_to(unit));
                    }
                }
            }
            (Some(_), Some(_)) => {
                ui.weak("No function is much like it (or it is too short to compare).");
            }
            _ => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading every function…");
                });
            }
        }
    });
    ui.add_space(4.0);
    (cleared, picked)
}

/// A group's places. The one clicked, if any.
fn group_rows(
    ui: &mut egui::Ui,
    units: &[usize],
    shapes: &lookalike::Shapes,
    graph: &CodeGraph,
) -> Option<MatchingAction> {
    let mut picked = None;
    let make = ui.button("Make a Semgrep Rule From These…").on_hover_text(
        "Draft a rule from what these places share, to find more like them; it opens in Find \
         More Like This",
    );
    if make.clicked() {
        let places = units
            .iter()
            .take(MAX_EXAMPLES)
            .map(|&i| {
                let unit = &shapes.units[i];
                Example::lines(&unit.file, unit.first_line, unit.last_line)
            })
            .collect();
        picked = Some(MatchingAction::MakeRule(places));
    }
    for &i in units {
        let unit = &shapes.units[i];
        let label = format!(
            "{}:{}–{}  {}",
            unit.file,
            unit.first_line,
            unit.last_line,
            unit_name(unit.symbol, graph)
        );
        if ui.selectable_label(false, visible(&label)).clicked() {
            picked = Some(go_to(unit));
        }
    }
    picked
}

fn go_to(unit: &lookalike::Unit) -> MatchingAction {
    MatchingAction::GoTo {
        symbol: unit.symbol,
        file: unit.file.clone(),
        line: unit.first_line,
    }
}

#[cfg(test)]
#[path = "matching_panel_tests.rs"]
mod tests;

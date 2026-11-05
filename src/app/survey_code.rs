//! The Project Triage tabs drawn from the index: which IPC and related
//! capabilities the code uses, and how complex each function is.

use egui::{Color32, RichText};

use super::survey_view::{plural, thousands, SurveyAction};
use crate::analysis::capabilities::CapabilityUse;
use crate::model::{CodeGraph, SymbolId};

/// McCabe's usual bands: upper bound, name, color.
const BANDS: [(u32, &str, Color32); 4] = [
    (10, "simple", Color32::GRAY),
    (20, "moderate", Color32::from_rgb(0xD9, 0xA6, 0x4A)),
    (50, "complex", Color32::from_rgb(0xE8, 0x8B, 0x3C)),
    (
        u32::MAX,
        "very complex",
        Color32::from_rgb(0xE0, 0x6C, 0x6C),
    ),
];
const COMPLEX: u32 = 20;
const VERY_COMPLEX: u32 = 50;
const ROW_HEIGHT: f32 = 20.0;
const NUMBER_WIDTH: f32 = 48.0;
/// Calls listed per function before "…and N more".
const MAX_LISTED_CALLS: usize = 200;

/// One function definition, ready to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionRow {
    pub name: String,
    pub symbol: SymbolId,
    pub file: String,
    pub line: u32,
    pub lines: u32,
    pub complexity: u32,
}

/// Every function definition, most complex first.
pub fn function_rows(graph: &CodeGraph) -> Vec<FunctionRow> {
    let mut rows: Vec<FunctionRow> = graph
        .functions
        .iter()
        .filter_map(|f| {
            Some(FunctionRow {
                name: graph.symbols.get(&f.symbol)?.qualified_name.clone(),
                symbol: f.symbol,
                file: f.file.clone(),
                line: f.line,
                lines: f.lines,
                complexity: f.complexity,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        (std::cmp::Reverse(a.complexity), &a.name, &a.file, a.line).cmp(&(
            std::cmp::Reverse(b.complexity),
            &b.name,
            &b.file,
            b.line,
        ))
    });
    rows
}

fn band(complexity: u32) -> (&'static str, Color32) {
    BANDS
        .iter()
        .find(|(max, _, _)| complexity <= *max)
        .map_or(("very complex", Color32::RED), |(_, name, color)| {
            (*name, *color)
        })
}

/// `412 functions; average complexity 3.1, highest 37 (db::Open); …`.
pub fn complexity_summary(rows: &[FunctionRow]) -> String {
    let Some(top) = rows.first() else {
        return "no function definitions indexed".into();
    };
    let total: u64 = rows.iter().map(|r| u64::from(r.complexity)).sum();
    let average = total as f64 / rows.len() as f64;
    let complex = rows.iter().filter(|r| r.complexity > COMPLEX).count();
    let very = rows.iter().filter(|r| r.complexity > VERY_COMPLEX).count();
    format!(
        "{} {}; average complexity {average:.1}, highest {} ({}); {complex} above {COMPLEX}, {very} above {VERY_COMPLEX}",
        thousands(rows.len() as u64),
        plural(rows.len(), "function"),
        top.complexity,
        top.name
    )
}

pub fn complexity_tab(
    ui: &mut egui::Ui,
    rows: &[FunctionRow],
    query: &mut String,
) -> Option<SurveyAction> {
    ui.label(complexity_summary(rows));
    ui.weak("Cyclomatic complexity (McCabe): 1 + each if, loop, case, catch, ?:, && and ||. 1–10 simple, 11–20 moderate, 21–50 complex, 51+ very complex.");
    ui.horizontal(|ui| {
        let label = ui.label("Filter");
        ui.add(
            egui::TextEdit::singleline(query)
                .hint_text("function or file")
                .desired_width(240.0),
        )
        .labelled_by(label.id);
    });
    let needle = query.trim().to_lowercase();
    let shown: Vec<&FunctionRow> = rows
        .iter()
        .filter(|r| {
            needle.is_empty()
                || r.name.to_lowercase().contains(&needle)
                || r.file.to_lowercase().contains(&needle)
        })
        .collect();
    ui.separator();
    let mut action = None;
    egui::ScrollArea::vertical().auto_shrink(false).show_rows(
        ui,
        ROW_HEIGHT,
        shown.len(),
        |ui, range| {
            for function in &shown[range] {
                if function_row(ui, function) {
                    action = Some(SurveyAction::GoTo {
                        symbol: Some(function.symbol),
                        file: function.file.clone(),
                        line: function.line,
                    });
                }
            }
        },
    );
    action
}

/// Draws one function; returns whether it was clicked.
fn function_row(ui: &mut egui::Ui, function: &FunctionRow) -> bool {
    ui.horizontal(|ui| {
        let (band, color) = band(function.complexity);
        let number = RichText::new(function.complexity.to_string())
            .color(color)
            .monospace()
            .strong();
        ui.add_sized([NUMBER_WIDTH, ROW_HEIGHT], egui::Label::new(number))
            .on_hover_text(format!("complexity {}: {band}", function.complexity));
        let lines = RichText::new(format!(
            "{} {}",
            function.lines,
            plural(function.lines as usize, "line")
        ))
        .weak();
        ui.add_sized([NUMBER_WIDTH * 1.5, ROW_HEIGHT], egui::Label::new(lines));
        let clicked = ui.link(&function.name).clicked();
        ui.weak(format!("{}:{}", function.file, function.line));
        clicked
    })
    .inner
}

pub fn ipc_tab(
    ui: &mut egui::Ui,
    uses: &[CapabilityUse],
    graph: &CodeGraph,
) -> Option<SurveyAction> {
    ui.weak("From calls to library functions in the indexed source. Socket families come from socket()'s arguments; only MAP_SHARED mmap() calls (shared memory or shared file mappings) count. Binaries' imports are on the Binaries tab.");
    ui.add_space(6.0);
    if uses.is_empty() {
        ui.label("No IPC or related calls found in the indexed source.");
        return None;
    }
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let mut action = None;
            for (title, is_ipc) in [
                ("Inter-process communication", true),
                ("Related reach", false),
            ] {
                let group: Vec<&CapabilityUse> = uses
                    .iter()
                    .filter(|u| u.category.is_ipc() == is_ipc)
                    .collect();
                if group.is_empty() {
                    continue;
                }
                ui.strong(title);
                for capability in group {
                    action = capability_section(ui, capability, graph).or(action);
                }
                ui.add_space(8.0);
            }
            action
        })
        .inner
}

fn capability_section(
    ui: &mut egui::Ui,
    capability: &CapabilityUse,
    graph: &CodeGraph,
) -> Option<SurveyAction> {
    let calls = capability.call_count();
    let functions: Vec<&str> = capability
        .functions
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    let title = format!(
        "{} — {calls} {}: {}",
        capability.category.label(),
        plural(calls, "call"),
        functions.join(", ")
    );
    egui::CollapsingHeader::new(title)
        .id_salt(capability.category)
        .show(ui, |ui| {
            let mut action = None;
            for function in &capability.functions {
                let header = format!("{} ×{}", function.name, function.calls.len());
                egui::CollapsingHeader::new(header)
                    .id_salt((capability.category, &function.name))
                    .show(ui, |ui| {
                        for call in function.calls.iter().take(MAX_LISTED_CALLS) {
                            let caller = graph
                                .symbols
                                .get(&call.caller)
                                .map_or("?", |s| s.qualified_name.as_str());
                            ui.horizontal(|ui| {
                                if ui.link(format!("{}:{}", call.file, call.line)).clicked() {
                                    action = Some(SurveyAction::GoTo {
                                        symbol: Some(call.caller),
                                        file: call.file.clone(),
                                        line: call.line,
                                    });
                                }
                                ui.weak(format!("in {caller}"));
                            });
                        }
                        let more = function.calls.len().saturating_sub(MAX_LISTED_CALLS);
                        if more > 0 {
                            ui.weak(format!("…and {more} more"));
                        }
                    });
            }
            action
        })
        .body_returned
        .flatten()
}

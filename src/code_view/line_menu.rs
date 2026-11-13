//! The right-click menu of a line of code: notes, bookmarks and TODOs,
//! rules like the line, the traces that start from a variable on it (and
//! end at a call on it), and marking a call dangerous.

use egui::Response;

use crate::app::visible::visible;

use super::variables::variables_in;
use super::{LineAction, LineMark};

/// What a line's menu offers, besides what every line has.
pub(super) struct MenuContext<'a> {
    /// The line, as shown.
    pub shown: &'a str,
    pub mark: Option<&'a LineMark<'a>>,
    /// The functions called on the line.
    pub calls: &'a [&'a str],
    /// The names on the line with cross references: its symbols and calls.
    pub names: &'a [&'a str],
    /// The file the line's `#include` names, if it is one.
    pub include: Option<&'a str>,
    /// The variable picked as the source of a trace to a sink, if any.
    pub trace_source: Option<&'a str>,
    /// The lines picked (first, last), when this is one of several.
    pub picked_lines: Option<(u32, u32)>,
    /// Whether a rule is being drafted, to add this line to.
    pub is_drafting_rule: bool,
}

/// The right-click menu of a line (on its number or its code).
pub(super) fn line_menu(response: &Response, menu: &MenuContext) -> Option<LineAction> {
    let (shown, mark, calls) = (menu.shown, menu.mark, menu.calls);
    let has_note = mark.is_some_and(|m| !m.note.is_empty());
    let is_starred = mark.is_some_and(|m| m.is_starred);
    let mut action = None;
    response.context_menu(|ui| {
        if ui.button("Copy Line").clicked() {
            ui.ctx().copy_text(shown.to_string());
            ui.close();
        }
        if let Some(file) = menu.include {
            let open = ui
                .button(format!("Open {}", visible(file)))
                .on_hover_text("Show the file this line includes (G over it does the same)");
            if open.clicked() {
                action = Some(LineAction::OpenFile(file.to_string()));
            }
        }
        ui.separator();
        let note = if has_note {
            "Edit Note…"
        } else {
            "Add Note…"
        };
        let star = if is_starred {
            "Remove Bookmark"
        } else {
            "Bookmark Line"
        };
        if ui.button(note).clicked() {
            action = Some(LineAction::EditNote);
        }
        if ui.button(star).clicked() {
            action = Some(LineAction::ToggleBookmark);
        }
        if ui.button("Add TODO Here…").clicked() {
            action = Some(LineAction::AddTodo);
        }
        if let Some(rule) = rule_menu(ui, menu) {
            action = Some(rule);
        }
        let like = ui
            .button("Find Code Like This Function")
            .on_hover_text("List the functions shaped most like the one holding this line");
        if like.clicked() {
            action = Some(LineAction::FindLookAlikes);
        }
        if let Some(traced) = trace_menu(ui, shown) {
            action = Some(traced);
        }
        if let Some(to_sink) = sink_menu(ui, menu) {
            action = Some(to_sink);
        }
        if !menu.names.is_empty() {
            ui.separator();
            if let Some(name) = xrefs_menu(ui, menu.names) {
                action = Some(LineAction::CrossReferences(name));
            }
        }
        if !calls.is_empty() {
            if menu.names.is_empty() {
                ui.separator();
            }
            if let Some(name) = docs_menu(ui, calls) {
                action = Some(LineAction::ShowDocs(name));
            }
            ui.menu_button("Mark as Dangerous", |ui| {
                for name in calls {
                    if ui.button(*name).clicked() {
                        action = Some(LineAction::MarkDangerous(name.to_string()));
                    }
                }
            });
        }
        if action.is_some() {
            ui.close();
        }
    });
    action
}

/// The line menu's trace submenus: the variables on the line, to trace
/// back to where each gets its value or forward to where it goes.
fn trace_menu(ui: &mut egui::Ui, shown: &str) -> Option<LineAction> {
    let variables = variables_in(shown);
    if variables.is_empty() {
        return None;
    }
    ui.separator();
    let back = variable_menu(
        ui,
        "Trace Where It Comes From",
        &variables,
        |name| format!("Follow where `{name}` gets its value, back to outside input"),
        LineAction::Trace,
    );
    let forward = variable_menu(
        ui,
        "Trace Where It Goes",
        &variables,
        |name| format!("Follow where the value of `{name}` goes, on to risky calls"),
        LineAction::TraceForward,
    );
    let source = ui
        .menu_button("Set as Trace Source", |ui| {
            variables.iter().find_map(|(name, column)| {
                let hint = format!(
                    "Trace from `{name}` to a call you pick next: right-click its line, then \
                     Trace `{name}` to Here"
                );
                let item = ui.button(name.as_str()).on_hover_text(hint);
                let column = u32::try_from(*column).ok()?;
                item.clicked().then(|| LineAction::SetTraceSource {
                    column,
                    name: name.clone(),
                })
            })
        })
        .inner
        .flatten();
    back.or(forward).or(source)
}

/// Drafting a semgrep rule from the line (or the lines picked), and, while
/// one is drafted, adding to it.
fn rule_menu(ui: &mut egui::Ui, menu: &MenuContext) -> Option<LineAction> {
    let (label, hint) = match menu.picked_lines {
        Some((first, last)) => (
            "Find More Like These Lines…".to_string(),
            format!("Draft a semgrep rule matching code like lines {first}–{last}, and test it"),
        ),
        None => (
            "Find More Like This…".to_string(),
            "Draft a semgrep rule matching code like this line, and test it".to_string(),
        ),
    };
    let mut action = None;
    if ui.button(label).on_hover_text(hint).clicked() {
        action = Some(LineAction::FindSimilar);
    }
    if !menu.is_drafting_rule {
        return action;
    }
    let place = ui.button("Add Place to Rule").on_hover_text(
        "Draft the rule being written from what this code and its other places share",
    );
    if place.clicked() {
        action = Some(LineAction::AddRulePlace);
    }
    let exception = ui.button("Use as Rule Exception").on_hover_text(
        "Leave matches out of the rule being written: after this check, or that look like \
         this fixed code",
    );
    if exception.clicked() {
        action = Some(LineAction::UseAsRuleException);
    }
    action
}

/// "Cross References" with the names on the line: the one picked.
fn xrefs_menu(ui: &mut egui::Ui, names: &[&str]) -> Option<String> {
    ui.menu_button("Cross References", |ui| {
        names.iter().find_map(|name| {
            let hint = "Every place it is used or called (X over the name does the same)";
            let item = ui.button(visible(name)).on_hover_text(hint);
            item.clicked().then(|| name.to_string())
        })
    })
    .inner
    .flatten()
}

/// "Show Docs" with the calls on the line: the one picked.
fn docs_menu(ui: &mut egui::Ui, calls: &[&str]) -> Option<String> {
    ui.menu_button("Show Docs", |ui| {
        calls.iter().find_map(|call| {
            let hint = "Its doc comment if the project defines it, else this system's man page";
            let item = ui.button(visible(&format!("{call}()"))).on_hover_text(hint);
            item.clicked().then(|| call.to_string())
        })
    })
    .inner
    .flatten()
}

/// "Trace `n` to Here" with the calls on the line, once a source is
/// picked.
fn sink_menu(ui: &mut egui::Ui, menu: &MenuContext) -> Option<LineAction> {
    let source = menu.trace_source?;
    if menu.calls.is_empty() {
        return None;
    }
    ui.menu_button(format!("Trace `{source}` to Here"), |ui| {
        menu.calls.iter().find_map(|call| {
            let hint = format!("Every path the value of `{source}` takes into `{call}()`");
            let item = ui.button(format!("{call}()")).on_hover_text(hint);
            item.clicked()
                .then(|| LineAction::TraceToHere(call.to_string()))
        })
    })
    .inner
    .flatten()
}

/// A submenu listing `variables`; picking one is `action` at its column.
fn variable_menu(
    ui: &mut egui::Ui,
    title: &str,
    variables: &[(String, usize)],
    hint: fn(&str) -> String,
    action: fn(u32) -> LineAction,
) -> Option<LineAction> {
    ui.menu_button(title, |ui| {
        variables.iter().find_map(|(name, column)| {
            let item = ui.button(name.as_str()).on_hover_text(hint(name));
            item.clicked()
                .then(|| u32::try_from(*column).ok().map(action))
                .flatten()
        })
    })
    .inner
    .flatten()
}

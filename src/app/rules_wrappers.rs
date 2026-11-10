//! The Semgrep Rules window's suggestions: the project's functions that
//! pass their parameters straight on to a risky call, each with a rule
//! that finds the calls to it worth a look. Pick some, then edit, test
//! and save their rules like any other.

use egui::RichText;

use super::background::BackgroundJob;
use super::survey_view::ERROR_COLOR;
use super::visible::visible;
use crate::analysis::wrappers::{rules_for, Passes, Wrapper};
use crate::model::SymbolId;

/// What the user asked of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WrapperAction {
    /// Look for the wrappers.
    Find,
    /// Show where this wrapper is.
    GoTo {
        symbol: SymbolId,
        file: String,
        line: u32,
    },
    /// Edit the rules for the wrappers picked (this text).
    Edit(String),
}

#[derive(Default)]
enum State {
    #[default]
    NotStarted,
    Finding(BackgroundJob<Vec<Wrapper>>),
    Found(Found),
    Failed(String),
}

#[derive(Default)]
pub struct WrapperList {
    state: State,
    filter: String,
}

impl WrapperList {
    /// Starts showing `job`'s wrappers once it finds them.
    pub fn start(&mut self, job: BackgroundJob<Vec<Wrapper>>) {
        self.state = State::Finding(job);
    }

    #[cfg(test)]
    pub fn set_found(&mut self, wrappers: Vec<Wrapper>) {
        self.state = State::Found(Found::new(wrappers));
    }

    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<WrapperAction> {
        self.poll();
        ui.label(
            "Functions of this project that pass their parameters straight on to a risky \
             call. Each one's rule finds the calls to it worth a look: a format that isn't a \
             literal, a copy size that isn't a constant, an allocation size that can \
             overflow, or every call when the risky call is risky whatever it is passed. \
             A wrapper may check what it is passed first: these are leads.",
        );
        match &mut self.state {
            State::NotStarted => Some(WrapperAction::Find),
            State::Finding(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Looking for wrappers…");
                });
                None
            }
            State::Failed(error) => {
                ui.label(RichText::new(error.as_str()).color(ERROR_COLOR));
                ui.button("Look Again")
                    .clicked()
                    .then_some(WrapperAction::Find)
            }
            State::Found(found) => found.show(ui, &mut self.filter),
        }
    }

    fn poll(&mut self) {
        let State::Finding(job) = &self.state else {
            return;
        };
        let Some(result) = job.poll() else {
            return;
        };
        self.state = match result {
            Ok(wrappers) => State::Found(Found::new(wrappers)),
            Err(error) => State::Failed(error),
        };
    }
}

/// The wrappers found, which are picked, and what each is filtered by.
struct Found {
    wrappers: Vec<Wrapper>,
    picked: Vec<bool>,
    /// Each one's name and file, lowercase.
    keys: Vec<String>,
}

impl Found {
    fn new(wrappers: Vec<Wrapper>) -> Self {
        let keys = wrappers
            .iter()
            .map(|w| format!("{} {}", w.name, w.file).to_lowercase())
            .collect();
        Self {
            picked: vec![false; wrappers.len()],
            wrappers,
            keys,
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, filter: &mut String) -> Option<WrapperAction> {
        if self.wrappers.is_empty() {
            ui.weak("None found: no function here passes its parameters on to a risky call.");
            return None;
        }
        let mut action = self.buttons(ui);
        ui.add(egui::TextEdit::singleline(filter).hint_text("Filter by name or file…"));
        let needle = filter.to_lowercase();
        let shown: Vec<usize> = (0..self.wrappers.len())
            .filter(|&i| needle.is_empty() || self.keys[i].contains(&needle))
            .collect();
        let row_height = ui.spacing().interact_size.y;
        egui::ScrollArea::vertical()
            .id_salt("wrapper_list")
            .auto_shrink([false, false])
            .show_rows(ui, row_height, shown.len(), |ui, range| {
                for &i in &shown[range] {
                    action = row(ui, &self.wrappers[i], &mut self.picked[i]).or(action.take());
                }
            });
        action
    }

    fn buttons(&mut self, ui: &mut egui::Ui) -> Option<WrapperAction> {
        let mut action = None;
        let count = self.picked.iter().filter(|p| **p).count();
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} wrappers, most called first.",
                self.wrappers.len()
            ));
            if ui.button("Pick All").clicked() {
                self.picked.iter_mut().for_each(|p| *p = true);
            }
            if ui.button("Pick None").clicked() {
                self.picked.iter_mut().for_each(|p| *p = false);
            }
            let edit = ui.add_enabled(
                count > 0,
                egui::Button::new(format!("Edit Rules for the {count} Picked")),
            );
            if edit
                .on_hover_text("Open their rules in the editor, to test and save")
                .clicked()
            {
                let chosen: Vec<&Wrapper> = self
                    .wrappers
                    .iter()
                    .zip(&self.picked)
                    .filter(|(_, p)| **p)
                    .map(|(w, _)| w)
                    .collect();
                action = Some(WrapperAction::Edit(rules_for(&chosen)));
            }
        });
        action
    }
}

fn row(ui: &mut egui::Ui, wrapper: &Wrapper, is_picked: &mut bool) -> Option<WrapperAction> {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let mut action = None;
    ui.horizontal(|ui| {
        ui.checkbox(is_picked, "").on_hover_text("Include its rule");
        ui.label(RichText::new(format!("{}()", visible(&wrapper.name))).font(font));
        let passed = match wrapper.passed.as_slice() {
            [] => String::new(),
            names => format!(" (passes on `{}`)", visible(&names.join("`, `"))),
        };
        let calls = match wrapper.callers {
            1 => "1 call".to_string(),
            n => format!("{n} calls"),
        };
        ui.weak(format!(
            "→ {}() · {}{passed} · {calls}",
            visible(&wrapper.callee),
            from_the_caller(wrapper.passes)
        ));
        let place = format!("{}:{}", wrapper.file, wrapper.line);
        if ui.link(visible(&place)).clicked() {
            action = Some(WrapperAction::GoTo {
                symbol: wrapper.symbol,
                file: wrapper.file.clone(),
                line: wrapper.line,
            });
        }
    });
    action
}

fn from_the_caller(passes: Passes) -> &'static str {
    match passes {
        Passes::Format { .. } => "its format comes from the caller",
        Passes::CopySize { .. } => "its copy size comes from the caller",
        Passes::AllocSize { .. } => "its allocation size comes from the caller",
        Passes::Arguments => "its arguments come from the caller",
    }
}

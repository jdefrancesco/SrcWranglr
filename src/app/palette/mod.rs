//! Quick-open palette: ⌘P / Ctrl+P jumps to any symbol by fuzzy name,
//! ⌘⇧P / Ctrl+Shift+P (or typing `>`) runs an app command, `#` searches
//! notes and `!` the library functions the project calls.
//! ↑/↓ move, Enter picks, Esc or clicking outside closes.

mod search;

use egui::{Align2, Id, Key, KeyboardShortcut, Modifiers, RichText};

use super::project::{LibraryFunction, Project};
use super::visible::visible;
use search::{search, Candidates};
pub use search::{Command, Entry};
pub use search::{COMMAND_PREFIX, LIBRARY_PREFIX, NOTE_PREFIX};

pub const OPEN_SYMBOL_SHORTCUT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, Key::P);
pub const OPEN_COMMAND_SHORTCUT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::P);
/// On macOS, also honour a literal Ctrl+P, unless a text field has focus
/// (where Ctrl+P moves the cursor up).
const OPEN_SYMBOL_CTRL: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::P);

const MAX_RESULTS: usize = 50;
const WIDTH: f32 = 560.0;
const TOP_OFFSET: f32 = 70.0;
const LIST_HEIGHT: f32 = 360.0;

#[derive(Default)]
pub struct Palette {
    is_open: bool,
    query: String,
    results: Vec<Entry>,
    /// The query `results` were computed for; `None` means stale.
    results_for: Option<String>,
    highlighted: usize,
    /// Scroll the highlighted row into view on the next frame.
    should_reveal: bool,
}

impl Palette {
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// Opens on the shortcut keys. While open, the symbol shortcut closes
    /// the palette and the command shortcut switches it to commands,
    /// keeping what was typed. Call once per frame, before `show`.
    pub fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let allow_ctrl_p = cfg!(target_os = "macos") && !ctx.egui_wants_keyboard_input();
        let (symbols, commands) = ctx.input_mut(|i| {
            let commands = i.consume_shortcut(&OPEN_COMMAND_SHORTCUT);
            let symbols = i.consume_shortcut(&OPEN_SYMBOL_SHORTCUT)
                || (allow_ctrl_p && i.consume_shortcut(&OPEN_SYMBOL_CTRL));
            (symbols, commands)
        });
        match (self.is_open, commands, symbols) {
            (true, true, _) if !self.query.starts_with(COMMAND_PREFIX) => {
                self.query.insert(0, COMMAND_PREFIX);
            }
            (true, false, true) => self.is_open = false,
            (false, true, _) => self.open_with(COMMAND_PREFIX.to_string()),
            (false, false, true) => self.open_with(String::new()),
            _ => {}
        }
    }

    pub fn open_with(&mut self, query: String) {
        *self = Self {
            is_open: true,
            query,
            ..Self::default()
        };
    }

    /// Draws the palette if open. Returns the entry the user picked.
    /// `notes` are `(index, label)` of the project's notes.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        project: &Project,
        notes: &[(usize, String)],
    ) -> Option<Entry> {
        if !self.is_open {
            return None;
        }
        // Results must match the query before Enter can pick from them.
        self.refresh(project, notes);
        let confirmed = self.handle_keys(ctx);
        let id = Id::unique("quick_open_palette");
        let area = egui::Modal::default_area(id).anchor(Align2::CENTER_TOP, [0.0, TOP_OFFSET]);
        let response = egui::Modal::new(id)
            .area(area)
            .show(ctx, |ui| self.contents(ui, project, notes));
        let picked = response.inner.or(confirmed);
        if picked.is_some() || response.should_close() {
            self.is_open = false;
        }
        picked
    }

    /// Arrow keys move the highlight; Enter confirms it.
    fn handle_keys(&mut self, ctx: &egui::Context) -> Option<Entry> {
        let (down, up, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
            )
        });
        let last = self.results.len().saturating_sub(1);
        if down {
            self.highlighted = (self.highlighted + 1).min(last);
            self.should_reveal = true;
        }
        if up {
            self.highlighted = self.highlighted.saturating_sub(1);
            self.should_reveal = true;
        }
        enter.then(|| self.results.get(self.highlighted).copied())?
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        notes: &[(usize, String)],
    ) -> Option<Entry> {
        ui.set_width(WIDTH);
        let edit = egui::TextEdit::singleline(&mut self.query)
            .hint_text("Go to symbol…   (> commands, # notes, ! library calls)")
            .desired_width(f32::INFINITY);
        ui.add(edit).request_focus();
        self.refresh(project, notes);
        ui.separator();
        if self.results.is_empty() {
            ui.weak("No matches");
            return None;
        }
        egui::ScrollArea::vertical()
            .max_height(LIST_HEIGHT)
            .show(ui, |ui| self.result_rows(ui, project, notes))
            .inner
    }

    fn result_rows(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        notes: &[(usize, String)],
    ) -> Option<Entry> {
        let mut picked = None;
        for (index, entry) in self.results.iter().enumerate() {
            let is_highlighted = index == self.highlighted;
            let row = ui.selectable_label(is_highlighted, row_text(*entry, project, notes));
            if is_highlighted && self.should_reveal {
                row.scroll_to_me(None);
            }
            if row.clicked() {
                picked = Some(*entry);
            }
        }
        self.should_reveal = false;
        picked
    }

    fn refresh(&mut self, project: &Project, notes: &[(usize, String)]) {
        if self.results_for.as_deref() == Some(self.query.as_str()) {
            return;
        }
        let candidates = Candidates {
            symbols: project.search_candidates(),
            notes,
            library: project.library_candidates(),
        };
        self.results = search(&candidates, &self.query, MAX_RESULTS);
        self.results_for = Some(self.query.clone());
        self.highlighted = 0;
    }
}

fn row_text(entry: Entry, project: &Project, notes: &[(usize, String)]) -> RichText {
    match entry {
        Entry::Command(command) => RichText::new(command.label()),
        Entry::Note(index) => match notes.iter().find(|(i, _)| *i == index) {
            Some((_, label)) => RichText::new(label),
            None => RichText::new("(missing note)"),
        },
        Entry::Symbol(id) => match project.graph.symbols.get(&id) {
            Some(s) => RichText::new(format!(
                "{}    {} · {}",
                s.qualified_name,
                s.kind.label(),
                s.file
            ))
            .monospace(),
            None => RichText::new("(missing symbol)"),
        },
        Entry::LibraryFunction(index) => match project.library_functions().get(index) {
            Some(function) => RichText::new(library_row(function)).monospace(),
            None => RichText::new("(missing function)"),
        },
    }
}

/// `system()    library · 2 calls`.
fn library_row(function: &LibraryFunction) -> String {
    let plural = if function.calls == 1 { "" } else { "s" };
    format!(
        "{}()    library · {} call{plural}",
        visible(&function.name),
        function.calls
    )
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::{NodeT, Queryable};
    use egui_kittest::Harness;

    use super::*;

    struct State {
        palette: Palette,
        project: Project,
        picked: Option<Entry>,
    }

    fn harness(query: &str) -> Harness<'static, State> {
        harness_for(Project::demo().expect("demo"), query)
    }

    fn harness_for(project: Project, query: &str) -> Harness<'static, State> {
        let mut palette = Palette::default();
        palette.open_with(query.to_string());
        let state = State {
            palette,
            project,
            picked: None,
        };
        let mut harness = Harness::new_ui_state(
            |ui, state: &mut State| {
                let ctx = ui.ctx().clone();
                state.palette.handle_shortcuts(&ctx);
                if let Some(entry) = state.palette.show(&ctx, &state.project, &[]) {
                    state.picked = Some(entry);
                }
            },
            state,
        );
        harness.run();
        harness
    }

    #[test]
    fn enter_picks_the_best_symbol_match() {
        let mut harness = harness("cirar");
        harness.key_press(Key::Enter);
        harness.run();
        let circle_area = harness.state().project.find("Circle::area");
        assert_eq!(harness.state().picked, circle_area.map(Entry::Symbol));
        assert!(!harness.state().palette.is_open);
    }

    #[test]
    fn arrow_down_then_enter_picks_the_second_row() {
        let mut harness = harness(">");
        harness.key_press(Key::ArrowDown);
        harness.key_press(Key::Enter);
        harness.run();
        assert_eq!(
            harness.state().picked,
            Some(Entry::Command(Command::ALL[1]))
        );
    }

    #[test]
    fn clicking_a_row_picks_it() {
        let mut harness = harness(">quit");
        harness.get_by_label("Quit").click();
        harness.run();
        assert_eq!(harness.state().picked, Some(Entry::Command(Command::Quit)));
    }

    #[test]
    fn escape_closes_without_picking() {
        let mut harness = harness("circle");
        harness.key_press(Key::Escape);
        harness.run();
        assert!(!harness.state().palette.is_open);
        assert_eq!(harness.state().picked, None);
    }

    #[test]
    fn shortcuts_toggle_and_switch_to_commands_keeping_the_query() {
        let mut harness = harness("circle");
        let command_shortcut = OPEN_COMMAND_SHORTCUT;
        harness.key_press_modifiers(command_shortcut.modifiers, command_shortcut.logical_key);
        harness.run();
        assert_eq!(harness.state().palette.query, ">circle");

        let symbol_shortcut = OPEN_SYMBOL_SHORTCUT;
        harness.key_press_modifiers(symbol_shortcut.modifiers, symbol_shortcut.logical_key);
        harness.run();
        assert!(!harness.state().palette.is_open);
    }

    #[test]
    fn bang_lists_the_library_functions_called_with_how_often() {
        let source = "void f(char *s) { system(s); memcpy(s, s, 1); system(s); }\n";
        let graph = crate::index::graph_of_files(&[("x.c", source)]);
        let mut harness = harness_for(Project::new("p", None, graph), "!sys");
        harness.get_by_label("system()    library · 2 calls");
        harness.key_press(Key::Enter);
        harness.run();
        assert_eq!(harness.state().picked, Some(Entry::LibraryFunction(0)));
    }

    #[test]
    fn the_hint_names_every_prefix() {
        let harness = harness("");
        let input = harness.get_by_role(egui::Role::TextInput).accesskit_node();
        let hint = input.placeholder().unwrap_or_default();
        assert!(hint.contains("! library calls"), "{hint}");
    }

    #[test]
    fn typing_updates_the_results() {
        let mut harness = harness("");
        harness.get_by_role(egui::Role::TextInput).type_text("zzzq");
        harness.run();
        harness.get_by_label("No matches");
    }
}

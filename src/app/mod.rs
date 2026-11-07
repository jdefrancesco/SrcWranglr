//! Top-level window: menu, symbol list, graph, code panel, status bar, and
//! the lifecycle of background indexing jobs.

mod about;
mod code_panel;
mod dangerous_editor;
mod dangerous_lists;
mod dialogs;
mod editor;
mod finding_view;
mod findings;
mod findings_actions;
mod findings_panel;
mod fonts;
mod harness_view;
pub mod launch;
mod layout;
mod locations;
mod menus;
mod note_editor;
mod notebook;
mod notes_actions;
mod notes_panel;
mod opening;
mod palette;
mod pane_bar;
mod project;
mod session;
mod session_actions;
mod settings;
mod settings_actions;
mod settings_view;
mod survey_code;
mod survey_fuzz;
mod survey_tabs;
mod survey_view;
mod symbol_list;
mod todo_actions;
mod todo_panel;
mod tool_job;
mod trace_actions;
mod trace_panel;
mod triage_actions;
mod trust;
mod welcome;

use std::path::PathBuf;

use launch::LaunchOptions;

use egui::{Color32, Key, KeyboardShortcut, Modifiers, RichText};

use crate::analysis;
use crate::analysis::reach::Reach;
use crate::graph::{Badge, GraphView, ViewMode};
use crate::index::job::IndexJob;
use crate::index::{IndexError, IndexOptions, IndexOutcome};
use code_panel::{CodePanel, PanelAction};
use dangerous_lists::DangerousLists;
use editor::SourcePosition;
use finding_view::FindingView;
use findings::Findings;
use findings_panel::FindingsPanel;
use layout::{Layout, SourcePlacement};
use note_editor::NoteEditor;
use notebook::Notebook;
use notes_panel::{NotesPanel, NotesPanelAction};
use palette::{Command, Entry, Palette};
use pane_bar::{Edge, Pane};
use project::Project;
use survey_view::{SurveyAction, SurveyTab, SurveyView};
use symbol_list::{SymbolList, SymbolListAction};
use tool_job::ToolJob;
use welcome::RecentProjects;

const OPEN_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const ENTRY_POINT_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::E);
const EDITOR_SHORTCUT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::E);
const BOOKMARK_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::D);
const NOTE_SHORTCUT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::N);
const NOTES_PANEL_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::B);
const FINDINGS_SHORTCUT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::R);
const TRIAGE_SHORTCUT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::T);
const LAYOUT_SHORTCUT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::L);
const SETTINGS_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Comma);
/// The symbol list's width until you drag it.
const SYMBOL_LIST_WIDTH: f32 = 260.0;
/// Entry points listed in the Go menu (all of them are reachable with ⌘E).
const MAX_ENTRY_MENU_ITEMS: usize = 25;
const APP_NAME: &str = "SrcWranglr";
const ERROR_COLOR: Color32 = Color32::from_rgb(0xE0, 0x6C, 0x6C);
const WARNING_COLOR: Color32 = Color32::from_rgb(0xD9, 0xA6, 0x4A);

enum Status {
    Info(String),
    Error(String),
}

pub struct SrcWranglrApp {
    project: Project,
    graph_view: GraphView,
    code_panel: CodePanel,
    palette: Palette,
    symbol_list: SymbolList,
    notebook: Notebook,
    /// The note being edited, if the editor is open.
    note_editor: Option<NoteEditor>,
    notes_panel: NotesPanel,
    /// Your dangerous-functions lists (global, and the project's).
    dangerous: DangerousLists,
    findings: Findings,
    findings_panel: FindingsPanel,
    /// External analyzers still running.
    tool_jobs: Vec<ToolJob>,
    /// The finding being explained, if its view is open.
    finding_view: Option<FindingView>,
    /// Your verdicts on findings.
    triage: crate::notes::triage::Triage,
    /// Why the verdicts file couldn't be read; verdicts are read-only then.
    triage_error: Option<String>,
    /// Whether the project's own verdicts and dangerous list are applied
    /// (yours, or ones you trusted), or only shown.
    are_project_files_trusted: bool,
    /// The dangerous-list entry being edited, if its editor is open.
    dangerous_editor: Option<dangerous_editor::DangerousEditor>,
    /// The Project Triage window.
    survey_view: SurveyView,
    /// The drafted fuzz harness, if its window is open.
    harness_view: Option<harness_view::HarnessView>,
    /// Where the source pane sits; kept between launches.
    layout: Layout,
    /// Which functions outside input reaches, for the open project.
    reach: Reach,
    /// The trace shown in the Trace pane, if any.
    trace_panel: trace_panel::TracePanel,
    /// Your settings (editor, tool folders, source colors).
    settings: settings::Settings,
    /// The Settings window, while it is open.
    settings_view: Option<settings_view::SettingsView>,
    /// Whether Help > About is showing.
    is_about_open: bool,
    /// Where you were in each project opened.
    sessions: session::Sessions,
    /// A session file's project and session, restored once that project
    /// is indexed (and dropped if another one opens instead).
    pending_session: Option<(PathBuf, session::Session)>,
    /// A session file's project and session, waiting for you to agree to
    /// index a folder you haven't opened before.
    session_to_confirm: Option<(PathBuf, session::Session)>,
    /// The session file dialog, while it is open.
    session_dialog: Option<session_actions::SessionDialog>,
    /// The open project's TODO list.
    todos: crate::notes::todo::Todos,
    /// Why the TODO file couldn't be read; the list is read-only then.
    todos_error: Option<String>,
    todo_panel: todo_panel::TodoPanel,
    /// The Open Project folder dialog, while it is open.
    open_dialog: Option<dialogs::PendingDialog<PathBuf>>,
    /// Project folders opened before; kept between launches.
    recent: RecentProjects,
    /// Whether a project (or the demo) is open; until then the start page
    /// shows.
    is_project_open: bool,
    job: Option<IndexJob>,
    status: Status,
    warnings: Vec<String>,
    is_warnings_open: bool,
    /// `--symbol` from the command line, focused once its project loads.
    pending_symbol: Option<String>,
    /// `--triage` from the command line: open Project Triage once loaded.
    is_triage_pending: bool,
}

impl SrcWranglrApp {
    /// The app, with the layout saved in `storage` from the last launch.
    pub fn new(
        ctx: &egui::Context,
        storage: Option<&dyn eframe::Storage>,
        options: LaunchOptions,
    ) -> Self {
        fonts::install(ctx);
        let project = Project::empty();
        let status = Status::Info("Open a project folder to get started.".into());
        let notebook = Notebook::open(&project);
        let dangerous = DangerousLists::load(None, true);
        let findings = Findings::for_graph(&project.graph, dangerous.merged());
        let mut app = Self {
            dangerous,
            findings,
            findings_panel: FindingsPanel::default(),
            trace_panel: trace_panel::TracePanel::default(),
            settings: settings::Settings::default(),
            settings_view: None,
            is_about_open: false,
            sessions: session::Sessions::load(storage),
            pending_session: None,
            session_to_confirm: None,
            session_dialog: None,
            todos: Default::default(),
            todos_error: None,
            todo_panel: todo_panel::TodoPanel::default(),
            tool_jobs: Vec::new(),
            finding_view: None,
            dangerous_editor: None,
            survey_view: SurveyView::default(),
            harness_view: None,
            layout: Layout::load(storage),
            reach: Reach::default(),
            open_dialog: None,
            recent: RecentProjects::load(storage),
            is_project_open: false,
            triage: Default::default(),
            triage_error: None,
            are_project_files_trusted: true,
            project,
            notebook,
            note_editor: None,
            notes_panel: NotesPanel::default(),
            graph_view: GraphView::default(),
            code_panel: CodePanel::default(),
            palette: Palette::default(),
            symbol_list: SymbolList::default(),
            job: None,
            status,
            warnings: Vec::new(),
            is_warnings_open: false,
            pending_symbol: options.symbol,
            is_triage_pending: options.triage,
        };
        app.graph_view.set_mode(options.view);
        app.load_settings();
        match options.project {
            Some(root) => app.start_indexing(root, ctx),
            // `--symbol` or `--triage` without a folder: look in the demo.
            None if app.pending_symbol.is_some() || app.is_triage_pending => app.open_demo(ctx),
            None => {}
        }
        app
    }

    fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        // Off macOS, ⌘ is Ctrl, and Ctrl+letter edits text in text fields.
        let can_use = cfg!(target_os = "macos") || !ctx.egui_wants_keyboard_input();
        // ⌘⇧E before ⌘E: egui matches ⌘E even with Shift held.
        let (open, editor, entry, bookmark, note, panel) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&OPEN_SHORTCUT),
                can_use && i.consume_shortcut(&EDITOR_SHORTCUT),
                can_use && i.consume_shortcut(&ENTRY_POINT_SHORTCUT),
                can_use && i.consume_shortcut(&BOOKMARK_SHORTCUT),
                can_use && i.consume_shortcut(&NOTE_SHORTCUT),
                can_use && i.consume_shortcut(&NOTES_PANEL_SHORTCUT),
            )
        });
        if can_use && ctx.input_mut(|i| i.consume_shortcut(&FINDINGS_SHORTCUT)) {
            self.run_command(Command::TogglePane(Pane::Findings), ctx);
        }
        if can_use && ctx.input_mut(|i| i.consume_shortcut(&TRIAGE_SHORTCUT)) {
            self.run_command(Command::Triage(SurveyTab::Overview), ctx);
        }
        if can_use && ctx.input_mut(|i| i.consume_shortcut(&LAYOUT_SHORTCUT)) {
            self.run_command(Command::SwapSourcePane, ctx);
        }
        if can_use && ctx.input_mut(|i| i.consume_shortcut(&SETTINGS_SHORTCUT)) {
            self.open_settings();
        }
        if panel {
            self.run_command(Command::TogglePane(Pane::Notes), ctx);
        }
        if bookmark {
            self.run_command(Command::ToggleBookmark, ctx);
        }
        if note {
            self.run_command(Command::EditNote, ctx);
        }
        if open {
            self.open_project_dialog(ctx);
        }
        if editor {
            self.open_in_editor();
        }
        if entry {
            self.go_to_entry_point();
        }
    }

    /// Opens what the code panel shows in the user's editor.
    fn open_in_editor(&mut self) {
        let Some(root) = self.project.root.as_ref() else {
            self.status = Status::Info(
                "The demo's sources aren't on disk: open a project (⌘O) to use an editor.".into(),
            );
            return;
        };
        let selected = self.graph_view.selected();
        let Some(at) = self.code_panel.position(&self.project.graph, selected) else {
            self.status = Status::Info("Select a symbol to open its source.".into());
            return;
        };
        let joined = root.join(&at.file);
        // Absolute, so a relative root can't make the path look like an option.
        let path = std::path::absolute(&joined).unwrap_or(joined);
        let position = SourcePosition {
            path,
            line: at.line,
            column: at.column,
        };
        let search = self.settings.search_path();
        let setting = self.settings.editor.as_deref();
        self.status = match editor::open_in_editor(&position, setting, &search) {
            Ok(program) => Status::Info(format!("Opened {}:{} with {program}", at.file, at.line)),
            Err(error) => Status::Error(error),
        };
    }

    /// Selects `main` (or the next entry point, when repeated) and says
    /// which one it is in the status bar.
    fn go_to_entry_point(&mut self) {
        let graph = &self.project.graph;
        let Some(step) = self.graph_view.go_to_entry_point(graph) else {
            self.status = Status::Info(
                "No entry points found: no main(), and every function with calls is called.".into(),
            );
            return;
        };
        let name = graph
            .symbols
            .get(&step.id)
            .map_or("?", |s| s.qualified_name.as_str());
        let more = if step.count > 1 {
            " (again for the next)"
        } else {
            ""
        };
        self.status = Status::Info(format!(
            "Entry point {} of {}: {name}{more}",
            step.position + 1,
            step.count
        ));
    }
}

impl eframe::App for SrcWranglrApp {
    /// Keeps the layout for the next launch (eframe keeps the window's and
    /// the panes' sizes).
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.layout.save(storage);
        self.recent.save(storage);
        self.remember_session();
        self.sessions.save(storage);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_open_dialog(&ctx);
        self.poll_session_dialog(&ctx);
        self.poll_job(&ctx);
        self.poll_tool_jobs();
        // Global shortcuts pause while the palette or a dialog is open.
        let is_dialog_open = self.note_editor.is_some()
            || self.finding_view.is_some()
            || self.dangerous_editor.is_some()
            || self.settings_view.is_some()
            || self.session_to_confirm.is_some();
        if !self.palette.is_open() && !is_dialog_open {
            self.handle_global_shortcuts(&ctx);
        }
        if !is_dialog_open {
            self.palette.handle_shortcuts(&ctx);
        }
        if let Some(command) = self.show_menu_bar(ui) {
            self.run_command(command, &ctx);
        }
        self.show_status_bar(ui);
        if self.is_project_open {
            self.show_project_panes(ui);
        } else {
            self.show_start_page(ui, &ctx);
        }
        self.show_warnings_window(&ctx);
        self.show_note_editor(&ctx);
        self.show_finding_view(&ctx);
        self.show_dangerous_editor(&ctx);
        self.show_settings_view(&ctx);
        about::show(&ctx, &mut self.is_about_open);
        self.show_session_confirm(&ctx);
        let survey = self
            .survey_view
            .show(&ctx, &self.project.name, &self.project.graph);
        match survey {
            Some(SurveyAction::GoTo { symbol, file, line }) => {
                self.go_to_place(symbol, &file, line);
            }
            Some(SurveyAction::Harness { symbol, file, line }) => {
                self.open_harness(symbol, &file, line);
            }
            None => {}
        }
        self.show_harness_view(&ctx);
        let notes = if self.palette.is_open() {
            self.note_candidates()
        } else {
            Vec::new()
        };
        let picked = self.palette.show(&ctx, &self.project, &notes);
        match picked {
            Some(Entry::Symbol(id)) => self.graph_view.select(id),
            Some(Entry::Command(command)) => self.run_command(command, &ctx),
            Some(Entry::Note(index)) => self.go_to_note_at(index),
            None => {}
        }
        if picked.is_some() {
            // The graph was drawn before the pick; show its effect now.
            ctx.request_repaint();
        }
    }
}

impl SrcWranglrApp {
    /// The symbol list, notes, findings, source and graph of the open project.
    fn show_project_panes(&mut self, ui: &mut egui::Ui) {
        self.show_symbol_list(ui);
        match self.notes_panel.show(ui, &self.notebook, self.layout) {
            Some(NotesPanelAction::GoTo(target)) => self.go_to_note(&target),
            Some(NotesPanelAction::Edit(target)) => self.edit_note(target),
            None => {}
        }
        self.show_todo_panel(ui);
        let finding = self
            .findings_panel
            .show(
                ui,
                &self.findings,
                &self.project.graph,
                &self.reach,
                self.layout,
            )
            .map(|f| (f.symbol, f.file.clone(), f.line));
        if let Some((symbol, file, line)) = finding {
            self.go_to_place(symbol, &file, line);
        }
        self.show_trace_panel(ui);
        self.show_code_panel(ui);
        egui::CentralPanel::default().show(ui, |ui| {
            let notebook = &self.notebook;
            let badge_of = |id| {
                notebook.mark(id).map(|m| Badge {
                    is_starred: m.is_starred,
                    has_text: m.has_text,
                })
            };
            self.graph_view.show(ui, &self.project.graph, &badge_of);
        });
    }

    /// The start page, until a project (or the demo) is open.
    fn show_start_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let open_keys = ctx.format_shortcut(&OPEN_SHORTCUT);
        let indexing = self.job.as_ref().map(|job| job.root().to_path_buf());
        let action = egui::CentralPanel::default()
            .show(ui, |ui| {
                welcome::show(ui, &self.recent, indexing.as_deref(), &open_keys)
            })
            .inner;
        if let Some(action) = action {
            self.handle_welcome(action, ctx);
        }
    }

    fn run_command(&mut self, command: Command, ctx: &egui::Context) {
        match command {
            Command::OpenProject => self.open_project_dialog(ctx),
            Command::OpenRecent(index) => {
                if let Some(root) = self.recent.paths().get(index).cloned() {
                    self.open_recent(root, ctx);
                }
            }
            Command::OpenDemo => self.open_demo(ctx),
            Command::Reindex => match self.project.root.clone() {
                Some(root) => self.start_indexing(root, ctx),
                None => {
                    self.status =
                        Status::Info("Nothing to re-index: open a project first (⌘O).".into())
                }
            },
            Command::Overview => self.graph_view.show_overview(),
            Command::GoToEntryPoint => self.go_to_entry_point(),
            Command::OpenInEditor => self.open_in_editor(),
            Command::ToggleBookmark => {
                if let Some(target) = self.selected_target() {
                    self.toggle_bookmark(target);
                }
            }
            Command::TogglePane(pane) => self.toggle_pane(pane),
            Command::MarkDangerous => self.mark_selected_dangerous(),
            Command::TrustProject => self.trust_project_files(),
            Command::RunSemgrep => self.run_tool(analysis::tools::Tool::Semgrep, ctx),
            Command::RunCppcheck => self.run_tool(analysis::tools::Tool::Cppcheck, ctx),
            Command::RunFlawfinder => self.run_tool(analysis::tools::Tool::Flawfinder, ctx),
            Command::Settings => self.open_settings(),
            Command::SaveSession => self.save_session_dialog(ctx),
            Command::OpenSession => self.open_session_dialog(ctx),
            Command::About => self.is_about_open = true,
            Command::EditNote => {
                if let Some(target) = self.selected_target() {
                    self.edit_note(target);
                }
            }
            Command::Back => self.graph_view.back(),
            Command::Fit => self.graph_view.fit(),
            Command::TrailView => self.graph_view.set_mode(ViewMode::Trail),
            Command::CallHierarchy => self.graph_view.set_mode(ViewMode::CallHierarchy),
            Command::Triage(tab) => {
                let root = self.project.root.clone();
                self.survey_view.open(
                    tab,
                    &self.project.graph,
                    self.findings.all(),
                    root.as_deref(),
                    ctx,
                );
            }
            Command::SwapSourcePane => {
                self.layout = self.layout.swapped();
                let place = match self.layout.source {
                    SourcePlacement::Below => "below",
                    SourcePlacement::Beside => "beside",
                };
                self.status = Status::Info(format!("Source pane moved {place} the graph."));
            }
            Command::ResetLayout => {
                self.layout = self.layout.reset();
                ctx.memory_mut(egui::Memory::reset_areas);
                self.status = Status::Info(
                    "Layout reset: panes and windows are back where they start.".into(),
                );
            }
            Command::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    fn show_status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| match &self.job {
                Some(job) => {
                    let progress = job.progress();
                    let fraction = if progress.total == 0 {
                        0.0
                    } else {
                        progress.done as f32 / progress.total as f32
                    };
                    ui.spinner();
                    ui.label(format!("Indexing {}", job.root().display()));
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .desired_width(200.0)
                            .text(format!("{}/{}", progress.done, progress.total)),
                    );
                    if ui.button("Cancel").clicked() {
                        job.cancel();
                    }
                }
                None => {
                    if let Some(job) = self.tool_jobs.first() {
                        ui.spinner();
                        ui.label(format!("{} running", job.tool().name()));
                        ui.separator();
                    }
                    match &self.status {
                        Status::Info(text) => ui.label(text),
                        Status::Error(text) => ui.label(RichText::new(text).color(ERROR_COLOR)),
                    };
                    if !self.warnings.is_empty() {
                        let label = RichText::new(format!("⚠ {} warning(s)", self.warnings.len()))
                            .color(WARNING_COLOR);
                        if ui.link(label).clicked() {
                            self.is_warnings_open = !self.is_warnings_open;
                        }
                    }
                }
            });
        });
    }

    /// The symbol list, sliding in and out at the left.
    fn show_symbol_list(&mut self, ui: &mut egui::Ui) {
        let mut is_shown = self.layout.is_symbols_shown;
        let shown = egui::Panel::left(self.layout.pane_id("symbol_list"))
            .resizable(true)
            .default_size(SYMBOL_LIST_WIDTH)
            .show_collapsible(ui, &mut is_shown, |ui| {
                let selected = self.graph_view.selected();
                self.symbol_list
                    .show(ui, &self.project, selected, &self.notebook)
            });
        // Dragged shut, or hidden from its arrow.
        let action = shown.and_then(|shown| shown.inner);
        let is_hidden = !is_shown || action == Some(SymbolListAction::Hide);
        self.layout = self.layout.with_symbols_shown(!is_hidden);
        if let Some(SymbolListAction::Select(id)) = action {
            self.graph_view.select(id);
        }
    }

    fn show_code_panel(&mut self, ui: &mut egui::Ui) {
        if !self.layout.is_source_shown {
            return;
        }
        let layout = self.layout;
        let (layout, ()) = layout.show_source_pane(ui, |ui| {
            let selected = self.graph_view.selected();
            let edge = match layout.source {
                SourcePlacement::Beside => Edge::Right,
                SourcePlacement::Below => Edge::Bottom,
            };
            let action = self.code_panel.show(
                ui,
                &self.project.graph,
                selected,
                &self.notebook,
                &self.findings,
                edge,
            );
            match action {
                Some(PanelAction::Hide) => self.layout = self.layout.with_source_shown(false),
                Some(PanelAction::Explore(id)) => self.graph_view.select(id),
                Some(PanelAction::OpenInEditor) => self.open_in_editor(),
                Some(PanelAction::ToggleBookmark(target)) => self.toggle_bookmark(target),
                Some(PanelAction::EditNote(target)) => self.edit_note(target),
                Some(PanelAction::ShowFinding(id)) => self.explain_finding(id),
                Some(PanelAction::MarkDangerous(name)) => self.mark_dangerous(&name),
                Some(PanelAction::AddTodo { file, line }) => self.todo_panel.add_here(file, line),
                Some(PanelAction::Trace { file, line, column }) => {
                    self.trace_variable(&file, line, column);
                }
                Some(PanelAction::TraceForward { file, line, column }) => {
                    self.trace_forward(&file, line, column);
                }
                None => {}
            }
        });
        // Kept hidden if its arrow was clicked.
        self.layout = layout.with_source_shown(self.layout.is_source_shown);
    }

    fn show_warnings_window(&mut self, ctx: &egui::Context) {
        let mut is_open = self.is_warnings_open;
        egui::Window::new("Indexing warnings")
            .open(&mut is_open)
            .default_width(520.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for warning in &self.warnings {
                            ui.label(warning);
                        }
                    });
            });
        self.is_warnings_open = is_open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_app() -> (SrcWranglrApp, egui::Context) {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        app.open_demo(&ctx);
        (app, ctx)
    }

    #[test]
    fn the_source_pane_can_be_swapped_and_the_layout_reset() {
        let (mut app, ctx) = demo_app();
        app.run_command(Command::SwapSourcePane, &ctx);
        assert_eq!(app.layout.source, SourcePlacement::Below);
        let before_reset = app.layout;
        app.run_command(Command::ResetLayout, &ctx);
        assert_eq!(app.layout.source, SourcePlacement::Beside);
        assert_ne!(app.layout.generation, before_reset.generation);
    }

    fn status_text(app: &SrcWranglrApp) -> &str {
        match &app.status {
            Status::Info(text) | Status::Error(text) => text,
        }
    }

    #[test]
    fn the_triage_command_opens_the_triage_window() {
        let (mut app, ctx) = demo_app();
        assert!(!app.survey_view.is_open());
        app.run_command(Command::Triage(SurveyTab::Binaries), &ctx);
        assert!(app.survey_view.is_open());
    }

    #[test]
    fn go_to_entry_point_selects_main_and_says_so() {
        let (mut app, ctx) = demo_app();
        app.run_command(Command::GoToEntryPoint, &ctx);
        let main = app.project.find("main");
        assert!(main.is_some());
        assert_eq!(app.graph_view.selected(), main);
        assert!(status_text(&app).contains("main"), "{}", status_text(&app));
    }

    #[test]
    fn open_in_editor_explains_that_the_demo_is_not_on_disk() {
        let (mut app, ctx) = demo_app();
        app.run_command(Command::GoToEntryPoint, &ctx);
        app.run_command(Command::OpenInEditor, &ctx);
        assert!(status_text(&app).contains("aren't on disk"));
    }

    #[test]
    fn open_in_editor_without_a_selection_asks_for_one() {
        let (mut app, ctx) = demo_app();
        app.project.root = Some(PathBuf::from("/nonexistent"));
        app.run_command(Command::OpenInEditor, &ctx);
        assert!(status_text(&app).contains("Select a symbol"));
    }

    #[test]
    fn go_to_entry_point_without_entry_points_reports_it() {
        let (mut app, ctx) = demo_app();
        app.project = Project::empty();
        app.run_command(Command::GoToEntryPoint, &ctx);
        assert_eq!(app.graph_view.selected(), None);
        assert!(status_text(&app).contains("No entry points"));
    }
}

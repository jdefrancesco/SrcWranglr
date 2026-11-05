//! Top-level window: menu, symbol list, graph, code panel, status bar, and
//! the lifecycle of background indexing jobs.

mod code_panel;
mod dangerous_editor;
mod dangerous_lists;
mod dialogs;
mod editor;
mod finding_view;
mod findings;
mod findings_actions;
mod findings_panel;
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
mod project;
mod survey_code;
mod survey_fuzz;
mod survey_tabs;
mod survey_view;
mod symbol_list;
mod tool_job;
mod triage_actions;
mod trust;
mod welcome;

use std::path::PathBuf;

use launch::LaunchOptions;

use egui::{Color32, Key, KeyboardShortcut, Modifiers, RichText};

use crate::analysis;
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
use project::Project;
use survey_view::{SurveyAction, SurveyTab, SurveyView};
use symbol_list::SymbolList;
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
        let project = Project::empty();
        let status = Status::Info("Open a project folder to get started.".into());
        let notebook = Notebook::open(&project);
        let dangerous = DangerousLists::load(None, true);
        let findings = Findings::for_graph(&project.graph, dangerous.merged());
        let mut app = Self {
            dangerous,
            findings,
            findings_panel: FindingsPanel::default(),
            tool_jobs: Vec::new(),
            finding_view: None,
            dangerous_editor: None,
            survey_view: SurveyView::default(),
            harness_view: None,
            layout: Layout::load(storage),
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
            self.run_command(Command::ShowFindings, ctx);
        }
        if can_use && ctx.input_mut(|i| i.consume_shortcut(&TRIAGE_SHORTCUT)) {
            self.run_command(Command::Triage(SurveyTab::Overview), ctx);
        }
        if can_use && ctx.input_mut(|i| i.consume_shortcut(&LAYOUT_SHORTCUT)) {
            self.run_command(Command::SwapSourcePane, ctx);
        }
        if panel {
            self.run_command(Command::ShowNotes, ctx);
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
        self.status = match editor::open_in_editor(&position) {
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
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_open_dialog(&ctx);
        self.poll_job(&ctx);
        self.poll_tool_jobs();
        // Global shortcuts pause while the palette or a dialog is open.
        let is_dialog_open = self.note_editor.is_some()
            || self.finding_view.is_some()
            || self.dangerous_editor.is_some();
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
        let finding = self
            .findings_panel
            .show(ui, &self.findings, &self.project.graph, self.layout)
            .map(|f| (f.symbol, f.file.clone(), f.line));
        if let Some((symbol, file, line)) = finding {
            self.go_to_place(symbol, &file, line);
        }
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
            Command::ShowNotes => self.notes_panel.toggle(),
            Command::ShowFindings => self.findings_panel.toggle(),
            Command::MarkDangerous => self.mark_selected_dangerous(),
            Command::TrustProject => self.trust_project_files(),
            Command::RunSemgrep => self.run_tool(analysis::tools::Tool::Semgrep, ctx),
            Command::RunCppcheck => self.run_tool(analysis::tools::Tool::Cppcheck, ctx),
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

    fn show_symbol_list(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left(self.layout.pane_id("symbol_list"))
            .resizable(true)
            .default_size(260.0)
            .show(ui, |ui| {
                let selected = self.graph_view.selected();
                let clicked = self
                    .symbol_list
                    .show(ui, &self.project, selected, &self.notebook);
                if let Some(id) = clicked {
                    self.graph_view.select(id);
                }
            });
    }

    fn show_code_panel(&mut self, ui: &mut egui::Ui) {
        let layout = self.layout;
        let (layout, ()) = layout.show_source_pane(ui, |ui| {
            let selected = self.graph_view.selected();
            let action = self.code_panel.show(
                ui,
                &self.project.graph,
                selected,
                &self.notebook,
                &self.findings,
            );
            match action {
                Some(PanelAction::Explore(id)) => self.graph_view.select(id),
                Some(PanelAction::OpenInEditor) => self.open_in_editor(),
                Some(PanelAction::ToggleBookmark(target)) => self.toggle_bookmark(target),
                Some(PanelAction::EditNote(target)) => self.edit_note(target),
                Some(PanelAction::ShowFinding(id)) => self.explain_finding(id),
                Some(PanelAction::MarkDangerous(name)) => self.mark_dangerous(&name),
                None => {}
            }
        });
        self.layout = layout;
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
        assert_eq!(app.layout.source, SourcePlacement::Beside);
        let before_reset = app.layout;
        app.run_command(Command::ResetLayout, &ctx);
        assert_eq!(app.layout.source, SourcePlacement::Below);
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

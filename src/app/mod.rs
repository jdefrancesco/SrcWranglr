//! Top-level window: menu, symbol list, graph, code panel, status bar, and
//! the lifecycle of background indexing jobs.

mod code_panel;
pub mod launch;
mod locations;
mod project;

use std::path::PathBuf;

use launch::LaunchOptions;

use egui::{Color32, Key, KeyboardShortcut, Modifiers, RichText};

use crate::graph::GraphView;
use crate::index::job::IndexJob;
use crate::index::{IndexError, IndexOptions, IndexOutcome};
use crate::model::SymbolId;
use code_panel::CodePanel;
use project::Project;

const OPEN_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const APP_NAME: &str = "slurcetrail";
const ERROR_COLOR: Color32 = Color32::from_rgb(0xE0, 0x6C, 0x6C);
const WARNING_COLOR: Color32 = Color32::from_rgb(0xD9, 0xA6, 0x4A);

enum Status {
    Info(String),
    Error(String),
}

enum MenuAction {
    OpenProject,
    Reindex,
    Quit,
}

pub struct SlurcetrailApp {
    project: Project,
    graph_view: GraphView,
    code_panel: CodePanel,
    search_query: String,
    /// Symbol-list rows for `filtered_for`; `None` means stale.
    filtered: Vec<SymbolId>,
    filtered_for: Option<String>,
    job: Option<IndexJob>,
    status: Status,
    warnings: Vec<String>,
    is_warnings_open: bool,
    /// `--symbol` from the command line, focused once its project loads.
    pending_symbol: Option<String>,
}

impl SlurcetrailApp {
    pub fn new(ctx: &egui::Context, options: LaunchOptions) -> Self {
        let (project, status) = match Project::demo() {
            Ok(p) => (
                p,
                Status::Info(
                    "Showing the built-in demo. File → Open Project… (⌘O) to index your own code."
                        .into(),
                ),
            ),
            Err(e) => (
                Project::empty(),
                Status::Error(format!("Could not load the demo: {e}")),
            ),
        };
        let mut app = Self {
            project,
            graph_view: GraphView::default(),
            code_panel: CodePanel::default(),
            search_query: String::new(),
            filtered: Vec::new(),
            filtered_for: None,
            job: None,
            status,
            warnings: Vec::new(),
            is_warnings_open: false,
            pending_symbol: options.symbol,
        };
        match options.project {
            Some(root) => app.start_indexing(root, ctx),
            None => app.apply_pending_symbol(),
        }
        app
    }

    fn start_indexing(&mut self, root: PathBuf, ctx: &egui::Context) {
        if let Some(previous) = self.job.take() {
            previous.cancel();
        }
        // Old warnings belong to the old project; don't show them next to
        // this run's result (which may be an error).
        self.warnings.clear();
        self.is_warnings_open = false;
        self.status = Status::Info(format!("Indexing {}…", root.display()));
        self.job = Some(IndexJob::spawn(root, IndexOptions::default(), ctx.clone()));
    }

    fn open_project_dialog(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new().set_title("Open a C/C++ project folder");
        if let Some(root) = &self.project.root {
            dialog = dialog.set_directory(root);
        }
        if let Some(root) = dialog.pick_folder() {
            self.start_indexing(root, ctx);
        }
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = self.job.as_mut() else {
            return;
        };
        let Some(result) = job.poll() else {
            return;
        };
        let root = job.root().to_path_buf();
        self.job = None;
        match result {
            Ok(outcome) => self.load_project(root, outcome, ctx),
            Err(IndexError::Cancelled) => self.status = Status::Info("Indexing cancelled.".into()),
            Err(err) => self.status = Status::Error(err.to_string()),
        }
    }

    fn load_project(&mut self, root: PathBuf, outcome: IndexOutcome, ctx: &egui::Context) {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        self.status = Status::Info(format!(
            "Indexed {} files: {} symbols, {} relationships.",
            outcome.indexed_files,
            outcome.graph.symbols.len(),
            outcome.graph.edges.len()
        ));
        self.project = Project::new(name.clone(), Some(root), outcome.graph);
        self.graph_view = GraphView::default();
        self.code_panel = CodePanel::default();
        self.filtered_for = None;
        self.warnings = outcome.warnings;
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{APP_NAME} — {name}")));
        self.apply_pending_symbol();
    }

    fn apply_pending_symbol(&mut self) {
        let Some(name) = self.pending_symbol.take() else {
            return;
        };
        match self.project.find(&name) {
            Some(id) => self.graph_view.select(id),
            None => {
                self.status =
                    Status::Error(format!("Symbol {name} not found in {}", self.project.name))
            }
        }
    }

    fn refresh_filter(&mut self) {
        if self.filtered_for.as_deref() != Some(self.search_query.as_str()) {
            self.filtered = self.project.filter(&self.search_query);
            self.filtered_for = Some(self.search_query.clone());
        }
    }
}

impl eframe::App for SlurcetrailApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_job(&ctx);
        if ctx.input_mut(|i| i.consume_shortcut(&OPEN_SHORTCUT)) {
            self.open_project_dialog(&ctx);
        }
        if let Some(action) = self.show_menu_bar(ui) {
            self.run_menu_action(action, &ctx);
        }
        self.show_status_bar(ui);
        self.show_symbol_list(ui);
        self.show_code_panel(ui);
        egui::CentralPanel::default().show(ui, |ui| {
            self.graph_view.show(ui, &self.project.graph);
        });
        self.show_warnings_window(&ctx);
    }
}

impl SlurcetrailApp {
    fn show_menu_bar(&mut self, ui: &mut egui::Ui) -> Option<MenuAction> {
        egui::Panel::top("menu_bar")
            .show(ui, |ui| {
                egui::MenuBar::new()
                    .ui(ui, |ui| {
                        let action = ui
                            .menu_button("File", |ui| self.file_menu(ui))
                            .inner
                            .flatten();
                        ui.separator();
                        ui.label(RichText::new(&self.project.name).strong());
                        ui.separator();
                        ui.label("Search:");
                        ui.text_edit_singleline(&mut self.search_query);
                        action
                    })
                    .inner
            })
            .inner
    }

    fn file_menu(&self, ui: &mut egui::Ui) -> Option<MenuAction> {
        let open = egui::Button::new("Open Project…")
            .shortcut_text(ui.ctx().format_shortcut(&OPEN_SHORTCUT));
        if ui.add(open).clicked() {
            return Some(MenuAction::OpenProject);
        }
        let can_reindex = self.project.root.is_some() && self.job.is_none();
        if ui
            .add_enabled(can_reindex, egui::Button::new("Re-index"))
            .clicked()
        {
            return Some(MenuAction::Reindex);
        }
        ui.separator();
        ui.button("Quit").clicked().then_some(MenuAction::Quit)
    }

    fn run_menu_action(&mut self, action: MenuAction, ctx: &egui::Context) {
        match action {
            MenuAction::OpenProject => self.open_project_dialog(ctx),
            MenuAction::Reindex => {
                if let Some(root) = self.project.root.clone() {
                    self.start_indexing(root, ctx);
                }
            }
            MenuAction::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
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
        self.refresh_filter();
        egui::Panel::left("symbol_list")
            .resizable(true)
            .default_size(260.0)
            .show(ui, |ui| {
                ui.heading(format!("Symbols ({})", self.filtered.len()));
                ui.separator();
                let row_height = ui.spacing().interact_size.y;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show_rows(ui, row_height, self.filtered.len(), |ui, rows| {
                        for id in &self.filtered[rows] {
                            let Some(symbol) = self.project.graph.symbols.get(id) else {
                                continue;
                            };
                            let is_selected = self.graph_view.selected() == Some(*id);
                            let label =
                                format!("{}  [{}]", symbol.qualified_name, symbol.kind.label());
                            if ui.selectable_label(is_selected, label).clicked() {
                                self.graph_view.select(*id);
                            }
                        }
                    });
            });
    }

    fn show_code_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("code_panel")
            .resizable(true)
            .default_size(260.0)
            .show(ui, |ui| {
                let selected = self.graph_view.selected();
                if let Some(id) = self.code_panel.show(ui, &self.project.graph, selected) {
                    self.graph_view.select(id);
                }
            });
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

//! The Project Triage window: a first look at the project. Folder facts
//! (languages, size, links, binaries) come from a background scan; code
//! facts (IPC in use, function complexity) come from the index.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use egui::{Color32, RichText};

use super::survey_code::{self, FunctionRow};
use crate::analysis::capabilities::{capabilities_in, CapabilityUse};
use crate::model::{CodeGraph, SymbolId};
use crate::survey::{self, format_bytes, Kind, Survey};

const DEFAULT_SIZE: [f32; 2] = [760.0, 540.0];
pub(super) const ERROR_COLOR: Color32 = Color32::from_rgb(0xE0, 0x6C, 0x6C);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurveyTab {
    Overview,
    Languages,
    Links,
    Binaries,
    Ipc,
    Complexity,
}

impl SurveyTab {
    pub const ALL: [SurveyTab; 6] = [
        SurveyTab::Overview,
        SurveyTab::Languages,
        SurveyTab::Links,
        SurveyTab::Binaries,
        SurveyTab::Ipc,
        SurveyTab::Complexity,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SurveyTab::Overview => "Overview",
            SurveyTab::Languages => "Languages & Size",
            SurveyTab::Links => "Repository Links",
            SurveyTab::Binaries => "Binaries & Hardening",
            SurveyTab::Ipc => "IPC & Capabilities",
            SurveyTab::Complexity => "Function Complexity",
        }
    }

    /// The tab's name in the command palette.
    pub fn command_label(self) -> &'static str {
        match self {
            SurveyTab::Overview => "Project Triage",
            SurveyTab::Languages => "Triage: Languages & Size",
            SurveyTab::Links => "Triage: Repository Links",
            SurveyTab::Binaries => "Triage: Binaries & Hardening",
            SurveyTab::Ipc => "Triage: IPC & Capabilities",
            SurveyTab::Complexity => "Triage: Function Complexity",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurveyAction {
    /// Show this place in the code.
    GoTo {
        symbol: Option<SymbolId>,
        file: String,
        line: u32,
    },
}

/// The folder scan running on a background thread; dropping it cancels.
struct SurveyJob {
    result: Receiver<Result<Survey, String>>,
    cancel: Arc<AtomicBool>,
}

impl SurveyJob {
    fn spawn(root: PathBuf, ctx: egui::Context) -> Result<Self, String> {
        let (tx, result) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        std::thread::Builder::new()
            .name("project-survey".into())
            .spawn(move || {
                let _ = tx.send(survey::survey(&root, &flag));
                ctx.request_repaint();
            })
            .map_err(|e| format!("Could not start the folder scan: {e}"))?;
        Ok(Self { result, cancel })
    }
}

impl Drop for SurveyJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Where the folder scan is at.
enum Scan {
    /// The project isn't a folder on disk (the demo).
    NoFolder,
    Running(SurveyJob),
    Done(Box<Survey>),
    Failed(String),
}

pub struct SurveyView {
    is_open: bool,
    tab: SurveyTab,
    root: Option<PathBuf>,
    scan: Scan,
    capabilities: Vec<CapabilityUse>,
    functions: Vec<FunctionRow>,
    /// Filters the complexity list by function or file name.
    query: String,
}

impl Default for SurveyView {
    fn default() -> Self {
        Self {
            is_open: false,
            tab: SurveyTab::Overview,
            root: None,
            scan: Scan::NoFolder,
            capabilities: Vec::new(),
            functions: Vec::new(),
            query: String::new(),
        }
    }
}

impl SurveyView {
    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// Opens the window on `tab`, reading the code facts from `graph` and
    /// scanning `root` unless it was already scanned.
    pub fn open(
        &mut self,
        tab: SurveyTab,
        graph: &CodeGraph,
        root: Option<&Path>,
        ctx: &egui::Context,
    ) {
        self.is_open = true;
        self.tab = tab;
        self.capabilities = capabilities_in(graph);
        self.functions = survey_code::function_rows(graph);
        if self.root.as_deref() != root || matches!(self.scan, Scan::NoFolder | Scan::Failed(_)) {
            self.root = root.map(Path::to_path_buf);
            self.rescan(ctx);
        }
    }

    /// Forgets everything about the previous project.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn rescan(&mut self, ctx: &egui::Context) {
        self.scan = match &self.root {
            None => Scan::NoFolder,
            Some(root) => match SurveyJob::spawn(root.clone(), ctx.clone()) {
                Ok(job) => Scan::Running(job),
                Err(error) => Scan::Failed(error),
            },
        };
    }

    fn poll(&mut self) {
        let Scan::Running(job) = &self.scan else {
            return;
        };
        self.scan = match job.result.try_recv() {
            Ok(Ok(survey)) => Scan::Done(Box::new(survey)),
            Ok(Err(error)) => Scan::Failed(error),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Scan::Failed("The folder scan stopped unexpectedly.".into())
            }
        };
    }

    #[cfg(test)]
    fn with_survey(survey: Survey, graph: &CodeGraph) -> Self {
        Self {
            is_open: true,
            root: Some(PathBuf::from("/project")),
            scan: Scan::Done(Box::new(survey)),
            capabilities: capabilities_in(graph),
            functions: survey_code::function_rows(graph),
            ..Self::default()
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        project_name: &str,
        graph: &CodeGraph,
    ) -> Option<SurveyAction> {
        if !self.is_open {
            return None;
        }
        self.poll();
        let mut is_open = true;
        let action = egui::Window::new("Project Triage")
            .open(&mut is_open)
            .default_size(DEFAULT_SIZE)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| self.contents(ui, project_name, graph))
            .and_then(|r| r.inner)
            .flatten();
        self.is_open = is_open;
        action
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        project_name: &str,
        graph: &CodeGraph,
    ) -> Option<SurveyAction> {
        ui.horizontal_wrapped(|ui| {
            for tab in SurveyTab::ALL {
                ui.selectable_value(&mut self.tab, tab, tab.label());
            }
            if self.root.is_some() && !matches!(self.scan, Scan::Running(_)) {
                ui.separator();
                if ui.small_button("Rescan Folder").clicked() {
                    self.rescan(ui.ctx());
                }
            }
        });
        ui.separator();
        match self.tab {
            SurveyTab::Overview => {
                self.overview(ui);
                None
            }
            SurveyTab::Ipc => survey_code::ipc_tab(ui, &self.capabilities, graph),
            SurveyTab::Complexity => {
                survey_code::complexity_tab(ui, &self.functions, &mut self.query)
            }
            folder_tab => {
                let survey = self.folder_survey(ui)?;
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| match folder_tab {
                        SurveyTab::Languages => super::survey_tabs::languages_tab(ui, survey),
                        SurveyTab::Links => super::survey_tabs::links_tab(ui, survey, project_name),
                        _ => super::survey_tabs::binaries_tab(ui, survey),
                    });
                None
            }
        }
    }

    /// The scan's result, or (drawn in its place) why there is none yet.
    fn folder_survey(&self, ui: &mut egui::Ui) -> Option<&Survey> {
        match &self.scan {
            Scan::Done(survey) => return Some(survey),
            Scan::NoFolder => {
                ui.label("This project isn't a folder on disk (it's the demo), so there is no folder to scan.");
            }
            Scan::Running(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Scanning the project folder…");
                });
            }
            Scan::Failed(error) => {
                ui.colored_label(ERROR_COLOR, format!("The folder scan failed: {error}"));
            }
        }
        None
    }

    fn overview(&self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Grid::new("survey_overview")
                    .num_columns(2)
                    .spacing([16.0, 6.0])
                    .striped(true)
                    .show(ui, |ui| {
                        if let Scan::Done(survey) = &self.scan {
                            folder_rows(ui, survey, self.root.as_deref());
                        }
                        self.code_rows(ui);
                    });
                if !matches!(self.scan, Scan::Done(_)) {
                    ui.add_space(8.0);
                    self.folder_survey(ui);
                }
            });
    }

    fn code_rows(&self, ui: &mut egui::Ui) {
        row(
            ui,
            "Functions",
            survey_code::complexity_summary(&self.functions),
        );
        let (ipc, other): (Vec<_>, Vec<_>) =
            self.capabilities.iter().partition(|c| c.category.is_ipc());
        let names = |uses: &[&CapabilityUse]| {
            uses.iter()
                .map(|u| format!("{} ({})", u.category.label(), u.call_count()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        row(ui, "IPC in use", or_none(names(&ipc), "no IPC calls found"));
        row(
            ui,
            "Also calls",
            or_none(names(&other), "no process, loader or network-library calls"),
        );
    }
}

/// The overview's folder facts, one per row.
fn folder_rows(ui: &mut egui::Ui, survey: &Survey, root: Option<&Path>) {
    if let Some(root) = root {
        row(ui, "Folder", root.display().to_string());
    }
    let size = format!(
        "{} in {} files (everything but version-control metadata)",
        format_bytes(survey.total_bytes),
        thousands(survey.file_count as u64)
    );
    row(ui, "Size", size);
    let code = survey.code_lines();
    let code_files: usize = survey
        .languages
        .iter()
        .filter(|l| l.kind == Kind::Code)
        .map(|l| l.files)
        .sum();
    row(
        ui,
        "Lines of code",
        format!(
            "{} code, {} comment, {} blank lines in {} source files",
            thousands(code.code),
            thousands(code.comments),
            thousands(code.blank),
            thousands(code_files as u64)
        ),
    );
    row(
        ui,
        "Languages",
        or_none(language_shares(survey), "no source files recognized"),
    );
    let platforms = survey
        .platforms
        .iter()
        .map(|(name, files)| format!("{name} ({} {})", files, plural(*files, "file")))
        .collect::<Vec<_>>()
        .join(", ");
    row(
        ui,
        "Platforms",
        or_none(platforms, "no platform-specific headers or checks"),
    );
    let build = survey
        .build_systems
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ");
    row(ui, "Build", or_none(build, "no build files recognized"));
    row(
        ui,
        "Docs",
        or_none(survey.docs.join(", "), "no README or docs"),
    );
    row(
        ui,
        "License",
        survey.license.unwrap_or("not recognized").to_string(),
    );
    row(ui, "CI", or_none(survey.ci.join(", "), "none"));
    row(ui, "Repository links", links_summary(survey));
    if let Some(branch) = &survey.branch {
        row(ui, "Git branch", branch.clone());
    }
    row(ui, "Binaries", binaries_summary(survey));
    for warning in &survey.warnings {
        row(ui, "Note", warning.clone());
    }
}

/// The likeliest link and where it came from: a git remote is the
/// project's own; a mention may be a dependency.
fn links_summary(survey: &Survey) -> String {
    let Some(first) = survey.links.first() else {
        return "none found".into();
    };
    let more = match survey.links.len() - 1 {
        0 => String::new(),
        n => format!(" and {n} more"),
    };
    format!("{} (from {}){more}", first.url, first.sources.join(", "))
}

/// `C 78%, C/C++ header 12%, …` by code lines.
fn language_shares(survey: &Survey) -> String {
    let code: Vec<_> = survey
        .languages
        .iter()
        .filter(|l| l.kind == Kind::Code)
        .collect();
    let total: u64 = code.iter().map(|l| l.lines.code).sum();
    code.iter()
        .map(|l| format!("{} {}%", l.name, percent(l.lines.code, total)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn binaries_summary(survey: &Survey) -> String {
    let stripped = survey.binaries.iter().filter(|b| b.is_stripped).count();
    let count = survey.binaries.len();
    let noun = if count == 1 {
        "program or library"
    } else {
        "programs or libraries"
    };
    let mut parts = vec![format!("{count} {noun} ({stripped} stripped)")];
    if survey.object_files > 0 {
        parts.push(format!("{} object files or archives", survey.object_files));
    }
    if !survey.unreadable_binaries.is_empty() {
        parts.push(format!("{} unreadable", survey.unreadable_binaries.len()));
    }
    parts.join(", ")
}

pub(super) fn row(ui: &mut egui::Ui, name: &str, value: String) {
    ui.label(RichText::new(name).strong());
    ui.add(egui::Label::new(value).wrap());
    ui.end_row();
}

fn or_none(text: String, none: &str) -> String {
    if text.is_empty() {
        none.to_string()
    } else {
        text
    }
}

pub(super) fn percent(part: u64, total: u64) -> u64 {
    (part * 100 + total / 2).checked_div(total).unwrap_or(0)
}

pub(super) fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        noun.to_string()
    } else {
        format!("{noun}s")
    }
}

/// `1234567` → `1,234,567`.
pub(super) fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
#[path = "survey_view_tests.rs"]
mod tests;

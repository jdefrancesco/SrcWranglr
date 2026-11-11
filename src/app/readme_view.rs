//! The README window: the project's README, read in the background and
//! laid out from its Markdown: headings, bold and italic text, code,
//! lists, quotes and tables. Its text comes with its hidden characters
//! already marked (see `readme`); links show their address on hover and
//! are never opened.

use std::path::PathBuf;

use egui::text::LayoutJob;
use egui::{Color32, FontId, RichText, Stroke, TextFormat};

use super::background::BackgroundJob;
use super::fonts;
use super::markdown::Style;
use super::readme::{self, Block, Readme, Run};
use super::visible::visible;

const WIDTH: f32 = 640.0;
const HEIGHT: f32 = 560.0;
/// Text size of headings by level: 1, 2, then the rest.
const HEADING_SIZES: [f32; 3] = [22.0, 18.0, 15.0];
/// Room above a heading.
const HEADING_SPACE: f32 = 8.0;
/// Room inside a code block's box.
const CODE_MARGIN: i8 = 6;
const CODE_ROUNDING: u8 = 4;
/// Room after a paragraph.
const PARAGRAPH_SPACE: f32 = 4.0;
/// Links listed when a paragraph is hovered, at most.
const MAX_LINKS_LISTED: usize = 20;
/// How far each level of a list is indented, and its markers' width.
const LIST_INDENT: f32 = 18.0;
const MARKER_WIDTH: f32 = 22.0;

/// The README window's content: still being read, or read.
pub enum ReadmeWindow {
    Reading(BackgroundJob<Readme>),
    Shown(Readme),
}

impl ReadmeWindow {
    /// Starts reading the README at the top of `root`, off the window's
    /// thread: the folder and file come from the audited tree. With no
    /// folder (the demo), there is none.
    pub fn open(root: Option<PathBuf>, ctx: &egui::Context) -> Self {
        let Some(root) = root else {
            return Self::Shown(Readme::Missing);
        };
        let read = move |_: &_| Ok(readme::read(&root));
        Self::Reading(BackgroundJob::spawn(
            "Reading the README",
            read,
            ctx.clone(),
        ))
    }

    /// Takes in the README once read.
    fn poll(&mut self) {
        let Self::Reading(job) = self else {
            return;
        };
        if let Some(result) = job.poll() {
            *self = Self::Shown(result.unwrap_or_else(|error| Readme::Unreadable {
                file_name: "README".into(),
                error,
            }));
        }
    }
}

/// Draws the window while it is open. Closing it forgets it.
pub fn show(ctx: &egui::Context, window: &mut Option<ReadmeWindow>, project_name: &str) {
    let Some(content) = window.as_mut() else {
        return;
    };
    content.poll();
    let mut is_open = true;
    egui::Window::new("README")
        .id(egui::Id::unique("readme_window"))
        .open(&mut is_open)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .default_size([WIDTH, HEIGHT])
        .resizable(true)
        .show(ctx, |ui| match content {
            ReadmeWindow::Reading(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Reading the README…");
                });
            }
            ReadmeWindow::Shown(readme) => shown(ui, readme, project_name),
        });
    if !is_open {
        *window = None;
    }
}

fn shown(ui: &mut egui::Ui, readme: &Readme, project_name: &str) {
    match readme {
        Readme::Found {
            file_name,
            blocks,
            is_cut_short,
        } => {
            ui.weak(format!(
                "{} · {}",
                visible(project_name),
                visible(file_name)
            ));
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    blocks.iter().for_each(|b| block(ui, b));
                    if *is_cut_short {
                        ui.add_space(HEADING_SPACE);
                        ui.weak("The README goes on; open the file to read the rest.");
                    }
                });
        }
        Readme::Unreadable { file_name, error } => {
            ui.label(format!(
                "{} couldn't be read: {}",
                visible(file_name),
                visible(error)
            ));
        }
        Readme::Missing => {
            ui.weak(format!(
                "No README at the top of {}.",
                visible(project_name)
            ));
        }
    }
}

fn block(ui: &mut egui::Ui, block: &Block) {
    match block {
        Block::Heading { level, runs } => {
            ui.add_space(HEADING_SPACE);
            let index = usize::from(*level).clamp(1, HEADING_SIZES.len()) - 1;
            text(ui, runs, Some(HEADING_SIZES[index]));
        }
        Block::Paragraph(runs) => {
            text(ui, runs, None);
            ui.add_space(PARAGRAPH_SPACE);
        }
        Block::Quote(runs) => {
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .inner_margin(CODE_MARGIN)
                .corner_radius(CODE_ROUNDING)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    text(ui, runs, None);
                });
        }
        Block::Item {
            depth,
            marker,
            runs,
        } => {
            ui.horizontal_top(|ui| {
                ui.add_space(LIST_INDENT * f32::from(*depth));
                ui.add_sized([MARKER_WIDTH, 0.0], egui::Label::new(marker.as_str()));
                text(ui, runs, None);
            });
        }
        Block::Code(code) => {
            egui::Frame::new()
                .fill(ui.visuals().code_bg_color)
                .inner_margin(CODE_MARGIN)
                .corner_radius(CODE_ROUNDING)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(code).monospace());
                });
        }
        Block::Rule => {
            ui.separator();
        }
        Block::Table(rows) => table(ui, rows),
    }
}

/// `rows` as a grid, the first row (the header) in bold.
fn table(ui: &mut egui::Ui, rows: &[Vec<Vec<Run>>]) {
    let id = ui.next_auto_id();
    egui::Grid::new(id).striped(true).show(ui, |ui| {
        for (i, row) in rows.iter().enumerate() {
            for cell in row {
                match i {
                    0 => {
                        let bold: Vec<Run> = cell
                            .iter()
                            .map(|run| Run {
                                style: Style {
                                    is_strong: true,
                                    ..run.style
                                },
                                ..run.clone()
                            })
                            .collect();
                        text(ui, &bold, None);
                    }
                    _ => {
                        text(ui, cell, None);
                    }
                }
            }
            ui.end_row();
        }
    });
    ui.add_space(PARAGRAPH_SPACE);
}

/// `runs` as one wrapped label, at `size` (a heading's) or the body's.
/// Links' addresses show on hover; they aren't opened.
fn text(ui: &mut egui::Ui, runs: &[Run], size: Option<f32>) -> egui::Response {
    let body = egui::TextStyle::Body.resolve(ui.style());
    let mono = egui::TextStyle::Monospace.resolve(ui.style());
    let visuals = ui.visuals().clone();
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    for run in runs {
        let style = run.style;
        let size = size.unwrap_or(body.size);
        let font = match (style.is_code, style.is_strong || size > body.size) {
            (true, _) => FontId::new(mono.size, mono.family.clone()),
            (false, true) => FontId::new(size, fonts::medium()),
            (false, false) => FontId::new(size, body.family.clone()),
        };
        let color = match (run.link.is_some(), style.is_image) {
            (true, _) => visuals.hyperlink_color,
            (false, true) => visuals.weak_text_color(),
            (false, false) => visuals.text_color(),
        };
        let line = |is_on: bool| match is_on {
            true => Stroke::new(1.0, color),
            false => Stroke::NONE,
        };
        let format = TextFormat {
            font_id: font,
            color,
            italics: style.is_emphasis,
            underline: line(run.link.is_some()),
            strikethrough: line(style.is_struck),
            background: match style.is_code {
                true => visuals.code_bg_color,
                false => Color32::TRANSPARENT,
            },
            ..TextFormat::default()
        };
        job.append(&run.text, 0.0, format);
    }
    let response = ui.label(job);
    let links: Vec<&str> = runs.iter().filter_map(|r| r.link.as_deref()).collect();
    if links.is_empty() {
        return response;
    }
    let mut listed = links[..links.len().min(MAX_LINKS_LISTED)].join("\n");
    if links.len() > MAX_LINKS_LISTED {
        listed.push_str(&format!("\n… and {} more", links.len() - MAX_LINKS_LISTED));
    }
    response.on_hover_text(format!("Links (not opened from here):\n{listed}"))
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;
    use crate::app::launch::LaunchOptions;
    use crate::app::palette::Command;
    use crate::app::SrcWranglrApp;

    fn demo_app() -> SrcWranglrApp {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        app.open_demo(&ctx);
        app
    }

    /// The menu bar and the README window.
    fn window(app: SrcWranglrApp) -> Harness<'static, SrcWranglrApp> {
        let mut harness = crate::app::fonts::test_harness()
            .with_size(egui::vec2(1400.0, 900.0))
            .build_ui_state(
                |ui, app: &mut SrcWranglrApp| {
                    let ctx = ui.ctx().clone();
                    if let Some(command) = app.show_menu_bar(ui) {
                        app.run_command(command, &ctx);
                    }
                    show(&ctx, &mut app.readme, &app.project.name);
                },
                app,
            );
        harness.run();
        harness
    }

    #[test]
    fn clicking_the_project_name_shows_its_readme() {
        let dir = tempfile::tempdir().expect("tempdir");
        let readme = "# Demo\n\nRead me first.\n\n```c\nint x;\n```\n\n- one\n- **two**\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        std::fs::write(dir.path().join("README.md"), readme).expect("write");
        let mut app = demo_app();
        app.project.root = Some(dir.path().to_path_buf());
        let name = app.project.name.clone();
        let mut harness = window(app);

        harness.get_by_label(&name).click();
        harness.run();
        let started = std::time::Instant::now();
        while harness.query_by_label("Read me first.").is_none() {
            assert!(
                started.elapsed().as_secs() < 10,
                "the README was never shown"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
            harness.run();
        }

        harness.get_by_label("Read me first.");
        harness.get_by_label("int x;");
        harness.get_by_label("two");
        harness.get_by_label("2");
        harness.get_by_label_contains("· README.md");
    }

    #[test]
    fn a_project_with_no_folder_says_it_has_no_readme() {
        let mut app = demo_app();
        app.project.root = None;
        app.run_command(Command::ShowReadme, &egui::Context::default());
        assert!(matches!(
            app.readme,
            Some(ReadmeWindow::Shown(Readme::Missing))
        ));
        let harness = window(app);
        harness.get_by_label_contains("No README at the top of");
    }
}

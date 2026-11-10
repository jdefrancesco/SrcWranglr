//! The README window: the project's README, read in the background and
//! laid out lightly, headings larger and code in monospace. Its text comes
//! with its hidden characters already marked (see `readme`).

use std::path::PathBuf;

use egui::RichText;

use super::background::BackgroundJob;
use super::fonts;
use super::readme::{self, Block, Readme};
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
        Block::Heading { level, text } => {
            ui.add_space(HEADING_SPACE);
            let index = usize::from(*level).clamp(1, HEADING_SIZES.len()) - 1;
            let heading = RichText::new(text)
                .size(HEADING_SIZES[index])
                .family(fonts::medium())
                .strong();
            ui.label(heading);
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
        Block::Text(text) => {
            ui.label(text);
        }
    }
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
        let readme = "# Demo\n\nRead me first.\n\n```c\nint x;\n```\n";
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

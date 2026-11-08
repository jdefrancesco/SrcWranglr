mod analysis;
mod app;
mod code_view;
mod demo;
mod docs;
mod graph;
mod history;
mod index;
mod model;
mod notes;
mod process;
mod programs;
mod safe_fs;
mod style;
mod survey;
mod version;

use app::launch::parse_args;
use app::SrcWranglrApp;

/// The window's size when not maximized, until you resize it.
const WINDOW_SIZE: [f32; 2] = [1440.0, 900.0];
/// Below this the panes stop fitting.
const MIN_WINDOW_SIZE: [f32; 2] = [960.0, 600.0];

/// Usage: see `app::launch::USAGE`.
fn main() -> eframe::Result<()> {
    let mut args = std::env::args_os().skip(1).peekable();
    // The app runs itself this way to read a git history apart from it.
    if args
        .peek()
        .is_some_and(|arg| arg == history::worker::WORKER_ARG)
    {
        args.next();
        std::process::exit(history::worker::worker_main(args));
    }
    let options = match parse_args(args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    if options.should_print_version {
        println!("srcwranglr {}", version::version_line());
        return Ok(());
    }
    // Maximized the first time; after that eframe restores the window as
    // it was left, overriding these. The app id matches the Linux .desktop
    // file, so docks group windows.
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size(WINDOW_SIZE)
        .with_min_inner_size(MIN_WINDOW_SIZE)
        .with_maximized(true)
        .with_app_id("srcwranglr");
    let viewport = match app::launch::app_icon() {
        Ok(icon) => viewport.with_icon(std::sync::Arc::new(icon)),
        Err(message) => {
            eprintln!("{message}");
            viewport
        }
    };
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "SrcWranglr",
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(SrcWranglrApp::new(
                &cc.egui_ctx,
                cc.storage,
                options,
            )))
        }),
    )
}

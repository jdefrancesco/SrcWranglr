mod analysis;
mod app;
mod code_view;
mod demo;
mod graph;
mod index;
mod model;
mod notes;
mod safe_fs;
mod style;
mod survey;

use app::launch::parse_args;
use app::SrcWranglrApp;

/// The window's size when not maximized, until you resize it.
const WINDOW_SIZE: [f32; 2] = [1440.0, 900.0];
/// Below this the panes stop fitting.
const MIN_WINDOW_SIZE: [f32; 2] = [960.0, 600.0];

/// Usage: `srcwranglr [PROJECT_DIR] [--symbol QUALIFIED_NAME]`.
fn main() -> eframe::Result<()> {
    let options = match parse_args(std::env::args_os().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
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

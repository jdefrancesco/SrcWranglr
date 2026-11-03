mod app;
mod code_view;
mod demo;
mod graph;
mod index;
mod model;

use app::launch::parse_args;
use app::SlurcetrailApp;

/// Usage: `slurcetrail [PROJECT_DIR] [--symbol QUALIFIED_NAME]`.
fn main() -> eframe::Result<()> {
    let options = match parse_args(std::env::args_os().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "slurcetrail",
        native_options,
        Box::new(move |cc| Ok(Box::new(SlurcetrailApp::new(&cc.egui_ctx, options)))),
    )
}

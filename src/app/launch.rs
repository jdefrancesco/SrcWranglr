//! Command-line options:
//! `slurcetrail [PROJECT_DIR] [--symbol NAME] [--view trail|calls] [--triage]`.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::graph::ViewMode;

pub const USAGE: &str =
    "usage: slurcetrail [PROJECT_DIR] [--symbol QUALIFIED_NAME] [--view trail|calls] [--triage]";

/// The window and Dock icon (the macOS bundle also carries it as .icns).
const ICON_PNG: &[u8] = include_bytes!("../../assets/icon-256.png");
#[cfg(test)]
const ICON_SIZE: u32 = 256;

/// macOS may pass `-psn_<number>` to apps launched from the Finder.
const PROCESS_SERIAL_PREFIX: &str = "-psn_";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LaunchOptions {
    /// Project folder to index on startup.
    pub project: Option<PathBuf>,
    /// Symbol to focus once the project (or the demo) is loaded.
    pub symbol: Option<String>,
    /// Graph view to start in.
    pub view: ViewMode,
    /// Open the Project Triage window once the project is loaded.
    pub triage: bool,
}

/// Parses arguments (without the program name).
pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<LaunchOptions, String> {
    let mut options = LaunchOptions::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--symbol") => {
                let name = args.next().ok_or("--symbol needs a name")?;
                let name = name
                    .into_string()
                    .map_err(|_| "--symbol name must be valid UTF-8")?;
                options.symbol = Some(name);
            }
            Some("--view") => {
                options.view = match args.next().as_ref().and_then(|v| v.to_str()) {
                    Some("trail") => ViewMode::Trail,
                    Some("calls") => ViewMode::CallHierarchy,
                    _ => return Err("--view needs `trail` or `calls`".into()),
                };
            }
            Some("--triage") => options.triage = true,
            Some("-h" | "--help") => return Err(USAGE.into()),
            Some(serial) if serial.starts_with(PROCESS_SERIAL_PREFIX) => {}
            Some(flag) if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            _ if options.project.is_none() => options.project = Some(PathBuf::from(arg)),
            _ => return Err("only one PROJECT_DIR may be given".into()),
        }
    }
    Ok(options)
}

/// The app's icon, decoded for the window and Dock.
pub fn app_icon() -> Result<egui::IconData, String> {
    eframe::icon_data::from_png_bytes(ICON_PNG).map_err(|e| format!("bad app icon: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<LaunchOptions, String> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn no_arguments_is_default() {
        assert_eq!(parse(&[]), Ok(LaunchOptions::default()));
    }

    #[test]
    fn parses_project_and_symbol_in_any_order() {
        let expected = LaunchOptions {
            project: Some(PathBuf::from("/p")),
            symbol: Some("ns::Foo".into()),
            view: ViewMode::Trail,
            triage: false,
        };
        assert_eq!(parse(&["/p", "--symbol", "ns::Foo"]), Ok(expected.clone()));
        assert_eq!(parse(&["--symbol", "ns::Foo", "/p"]), Ok(expected));
    }

    #[test]
    fn ignores_the_process_serial_number_finder_may_pass() {
        assert_eq!(parse(&["-psn_0_12345"]), Ok(LaunchOptions::default()));
    }

    #[test]
    fn the_app_icon_decodes_to_a_square_image() {
        let icon = app_icon().expect("icon");
        assert_eq!((icon.width, icon.height), (ICON_SIZE, ICON_SIZE));
        assert_eq!(icon.rgba.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
    }

    #[test]
    fn parses_the_triage_flag() {
        let options = parse(&["/p", "--triage"]).expect("parse");
        assert!(options.triage);
        assert_eq!(options.project, Some(PathBuf::from("/p")));
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse(&["--symbol"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["/a", "/b"]).is_err());
        assert!(parse(&["--help"]).is_err());
        assert!(parse(&["--view"]).is_err());
        assert!(parse(&["--view", "sideways"]).is_err());
    }

    #[test]
    fn parses_view_mode() {
        assert_eq!(
            parse(&["--view", "calls"]).map(|o| o.view),
            Ok(ViewMode::CallHierarchy)
        );
        assert_eq!(
            parse(&["--view", "trail"]).map(|o| o.view),
            Ok(ViewMode::Trail)
        );
    }
}

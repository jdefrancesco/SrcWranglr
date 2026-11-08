//! Running another program with limits: a deadline, a cap on what it may
//! write back, and a flag that stops it. Used for SrcWranglr's own history
//! worker and for reading man pages, never for the project being audited.

use std::ffi::OsString;
use std::fmt;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How often a running program is checked on.
const POLL: Duration = Duration::from_millis(20);

/// How long a program may run, and how much of its output is read.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub deadline: Duration,
    pub max_output: u64,
}

/// Why a program gave no output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessError {
    /// It couldn't be started: why.
    Start(String),
    /// It was stopped by the flag.
    Stopped,
    /// It ran past its deadline and was killed.
    TookTooLong,
    /// Waiting for it failed: why.
    Lost(String),
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start(why) | Self::Lost(why) => f.write_str(why),
            Self::Stopped => f.write_str("Stopped."),
            Self::TookTooLong => f.write_str("It took too long and was stopped."),
        }
    }
}

/// Runs `program` with `args` and `env` added to the environment, `input`
/// on its stdin; its stdout (up to `limits.max_output` bytes) once it
/// exits. Killed, with any programs it started, if it outlives
/// `limits.deadline` or `stop` is set. It runs from `/`, so nothing it
/// resolves relative to its folder can land in a project. `what` names it
/// in errors.
pub fn run_process(
    what: &str,
    program: &Path,
    args: &[OsString],
    env: &[(&str, &str)],
    input: &[u8],
    limits: Limits,
    stop: &AtomicBool,
) -> Result<Vec<u8>, ProcessError> {
    let started = Instant::now();
    let mut command = Command::new(program);
    command
        .args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A group of its own, so what it starts can be killed with it.
        command.current_dir("/").process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| ProcessError::Start(format!("Could not start {what}: {e}")))?;
    let lost = |why: &str| ProcessError::Lost(format!("{why} {what}"));
    let Some(mut stdin) = child.stdin.take() else {
        kill(&mut child);
        return Err(lost("No pipe to"));
    };
    let Some(mut stdout) = child.stdout.take() else {
        kill(&mut child);
        return Err(lost("No pipe from"));
    };
    let input = input.to_vec();
    // Written and read on threads of their own, so neither pipe can stall
    // the other; the reader hands its result over a channel, so a pipe a
    // straggler keeps open can't hold up the caller.
    std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let (sender, output) = mpsc::channel();
    let max_output = limits.max_output;
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.by_ref().take(max_output).read_to_end(&mut out);
        // Past the cap the rest is read and dropped, so the program isn't
        // left blocked on a full pipe until its deadline.
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        let _ = sender.send(out);
    });
    loop {
        let is_stopped = stop.load(Ordering::Relaxed);
        if is_stopped || started.elapsed() > limits.deadline {
            kill(&mut child);
            return Err(match is_stopped {
                true => ProcessError::Stopped,
                false => ProcessError::TookTooLong,
            });
        }
        match has_exited(&mut child) {
            Ok(true) => break,
            Ok(false) => std::thread::sleep(POLL),
            Err(e) => {
                kill(&mut child);
                return Err(ProcessError::Lost(format!("Lost {what}: {e}")));
            }
        }
    }
    // Anything it left running would keep the pipe open: end it (its group
    // id is still its own: it isn't reaped until `kill`).
    kill(&mut child);
    let left = limits.deadline.saturating_sub(started.elapsed()).max(POLL);
    output
        .recv_timeout(left)
        .map_err(|_| ProcessError::Lost(format!("Could not read what {what} wrote.")))
}

/// Whether `child` has exited, without reaping it: until it is reaped its
/// id can't be reused, so its group can still be killed safely.
#[cfg(unix)]
fn has_exited(child: &mut Child) -> std::io::Result<bool> {
    let id = libc::id_t::from(child.id());
    // SAFETY: an all-zero `siginfo_t` is valid, and `waitid` only writes
    // into it. Zeroed, a `si_signo` still 0 after the call means no change.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let options = libc::WEXITED | libc::WNOHANG | libc::WNOWAIT;
    // SAFETY: `info` is a valid, writable `siginfo_t`.
    if unsafe { libc::waitid(libc::P_PID, id, &mut info, options) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(info.si_signo != 0)
}

#[cfg(not(unix))]
fn has_exited(child: &mut Child) -> std::io::Result<bool> {
    child.try_wait().map(|status| status.is_some())
}

/// Kills `child` and every program in its group, and reaps it.
fn kill(child: &mut Child) {
    #[cfg(unix)]
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: `killpg` only sends a signal; the group is the child's
        // own (made with `process_group(0)`), so nothing else is hit.
        unsafe {
            libc::killpg(group, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn limits(deadline: Duration) -> Limits {
        Limits {
            deadline,
            max_output: 1024,
        }
    }

    fn shell(script: &str) -> [OsString; 2] {
        ["-c".into(), script.into()]
    }

    #[test]
    fn a_process_past_its_deadline_is_killed() {
        let started = Instant::now();
        let error = run_process(
            "sleep",
            Path::new("/bin/sh"),
            &shell("sleep 30"),
            &[],
            b"",
            limits(Duration::from_millis(200)),
            &AtomicBool::new(false),
        )
        .expect_err("killed");
        assert_eq!(error, ProcessError::TookTooLong);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_stopped_process_is_killed_and_an_answer_is_read() {
        let stop = AtomicBool::new(true);
        let error = run_process(
            "sleep",
            Path::new("/bin/sh"),
            &shell("sleep 30"),
            &[],
            b"",
            limits(Duration::from_secs(60)),
            &stop,
        )
        .expect_err("stopped");
        assert_eq!(error.to_string(), "Stopped.");
        let out = run_process(
            "cat",
            Path::new("/bin/sh"),
            &shell("cat"),
            &[],
            b"[1,2]",
            limits(Duration::from_secs(10)),
            &AtomicBool::new(false),
        )
        .expect("ran");
        assert_eq!(out, b"[1,2]");
    }

    #[test]
    fn its_environment_is_added_to_and_its_output_capped() {
        let out = run_process(
            "echo",
            Path::new("/bin/sh"),
            &shell("printf '%s' \"$SRCWRANGLR_TEST_VALUE\"; yes | head -c 5000"),
            &[("SRCWRANGLR_TEST_VALUE", "abc")],
            b"",
            limits(Duration::from_secs(10)),
            &AtomicBool::new(false),
        )
        .expect("ran");
        assert!(out.starts_with(b"abc"));
        assert_eq!(out.len(), 1024);
        let long = run_process(
            "yes",
            Path::new("/bin/sh"),
            &shell("yes | head -c 2000000"),
            &[],
            b"",
            limits(Duration::from_secs(10)),
            &AtomicBool::new(false),
        )
        .expect("a long output is cut, not taken for a hang");
        assert_eq!(long.len(), 1024);
    }

    #[test]
    fn what_it_started_is_killed_with_it_and_cannot_hold_up_the_caller() {
        let started = Instant::now();
        // The shell starts a sleeper that keeps stdout open, then waits.
        let error = run_process(
            "sleep",
            Path::new("/bin/sh"),
            &shell("sleep 30 & wait"),
            &[],
            b"",
            limits(Duration::from_millis(300)),
            &AtomicBool::new(false),
        )
        .expect_err("killed");
        assert_eq!(error, ProcessError::TookTooLong);
        // And a sleeper left behind by a shell that exits at once.
        let out = run_process(
            "echo",
            Path::new("/bin/sh"),
            &shell("printf done; sleep 30 &"),
            &[],
            b"",
            limits(Duration::from_secs(2)),
            &AtomicBool::new(false),
        )
        .expect("ran");
        assert_eq!(out, b"done");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn it_runs_from_the_root_folder() {
        let out = run_process(
            "pwd",
            Path::new("/bin/sh"),
            &shell("pwd"),
            &[],
            b"",
            limits(Duration::from_secs(5)),
            &AtomicBool::new(false),
        )
        .expect("ran");
        assert_eq!(out, b"/\n");
    }

    #[test]
    fn a_missing_program_says_it_could_not_start() {
        let error = run_process(
            "the reader",
            Path::new("/nonexistent/program"),
            &[],
            &[],
            b"",
            limits(Duration::from_secs(1)),
            &AtomicBool::new(false),
        )
        .expect_err("missing");
        assert!(error.to_string().starts_with("Could not start the reader"));
    }
}

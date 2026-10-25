//! The background threads that ping hosts or time commands, feeding results to the UI.
//!
//! Nothing here is ever joined: quitting must not wait for a ping that is due in five
//! seconds or a command that takes a minute. Instead [`Stopper`] ends the processes they
//! run, which gping would otherwise leave behind, and the threads die with gping.

use crate::app::HostState;
use anyhow::{bail, Result};
use pinger::{ping, PingOptions, PingResult};
use std::io;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};
use tui::crossterm::event::Event as TerminalEvent;

pub enum Event {
    /// A ping reply (`Some(rtt)`) or a lost ping (`None`) for the host at this index, and how
    /// long ago it happened (losses are only reported once their grace period is over).
    Probe(usize, Option<Duration>, Duration),
    /// The host's pinger or command loop has stopped for good.
    Ended(usize, HostState),
    Input(TerminalEvent),
    /// Keyboard input can't be read, so the user would have no way to quit.
    InputFailed(String),
    /// We were asked to stop by this signal.
    #[cfg_attr(not(unix), allow(dead_code))]
    Signal(i32),
}

/// Starts pinging, returning an error if the ping process can't be started at all.
pub fn start_ping(options: PingOptions, host: usize, tx: Sender<Event>) -> Result<()> {
    let delay = pinger::timeout_delay(&options);
    let stream = ping(options)?;
    thread::spawn(move || {
        for result in stream {
            let event = match result {
                PingResult::Pong(rtt, _) => Event::Probe(host, Some(rtt), Duration::ZERO),
                PingResult::Timeout(_) => Event::Probe(host, None, delay),
                PingResult::Unknown(_) => continue,
                PingResult::PingExited(status, _) if status.success() => {
                    Event::Ended(host, HostState::Finished)
                }
                PingResult::PingExited(status, stderr) => {
                    let detail = stderr.trim();
                    let message = if detail.is_empty() {
                        format!("ping exited with {status}")
                    } else {
                        format!("ping exited with {status}: {detail}")
                    };
                    Event::Ended(host, HostState::Failed(message))
                }
            };
            if tx.send(event).is_err() {
                return;
            }
        }
    });
    Ok(())
}

/// Splits a `--cmd` string into a program and its arguments, honouring quotes the way a
/// shell would, without running one (so no pipes, globs or variables). On Windows only
/// double quotes group words and backslashes are left alone, since they are path separators
/// there.
pub fn parse_command(command: &str) -> Result<(String, Vec<String>)> {
    let posix = !cfg!(windows);
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' if posix => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => bail!("Unterminated ' in the command `{command}`"),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') if posix => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => word.push(c),
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => bail!("Unterminated \" in the command `{command}`"),
                        },
                        Some(c) => word.push(c),
                        None => bail!("Unterminated \" in the command `{command}`"),
                    }
                }
            }
            '\\' if posix => {
                in_word = true;
                if let Some(c) = chars.next() {
                    word.push(c);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    let mut words = words.into_iter();
    match words.next() {
        Some(program) => Ok((program, words.collect())),
        None => bail!("An empty command was given to --cmd"),
    }
}

/// Set once gping has started stopping everything, so that no new command starts.
static STOPPING: AtomicBool = AtomicBool::new(false);

/// The commands running right now, by process ID.
static COMMANDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Stops every ping and command still running when it is dropped, however gping exits.
/// A ping to a host that never answers may print nothing for as long as it runs, so it
/// would never notice that gping has gone; and a command can take as long as it likes.
pub struct Stopper;

impl Drop for Stopper {
    fn drop(&mut self) {
        STOPPING.store(true, Ordering::Release);
        pinger::stop_all();
        let running = std::mem::take(&mut *COMMANDS.lock().unwrap_or_else(PoisonError::into_inner));
        for id in running {
            stop(id);
        }
    }
}

/// A command leaves the list as soon as it has been waited for, and process IDs are
/// handed out in turn rather than reused at once, so `id` is still the command's.
#[cfg(unix)]
fn stop(id: u32) {
    // SAFETY: kill(2) takes plain integers and touches no memory of ours.
    unsafe {
        libc::kill(id as libc::pid_t, libc::SIGTERM);
    }
}

/// Windows has no signals to send; a command running as gping quits finishes on its own.
#[cfg(not(unix))]
fn stop(_id: u32) {}

/// Waits for a command to finish, listed among those to stop if gping quits meanwhile.
fn wait(mut child: Child) -> io::Result<ExitStatus> {
    let id = child.id();
    COMMANDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(id);
    // If gping started stopping everything just before it was listed, it was missed.
    if STOPPING.load(Ordering::Acquire) {
        let _ = child.kill();
    }
    let status = child.wait();
    COMMANDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|running| *running != id);
    status
}

/// Runs a command over and over, timing each run. A run that exits unsuccessfully counts
/// as lost. The command gets no stdin: it would otherwise read the keys meant for gping.
pub fn start_command(
    (program, args): (String, Vec<String>),
    host: usize,
    interval: Duration,
    tx: Sender<Event>,
) {
    thread::spawn(move || loop {
        if STOPPING.load(Ordering::Acquire) {
            return;
        }
        let started = Instant::now();
        let child = Command::new(&program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let event = match child.and_then(wait) {
            Ok(status) => Event::Probe(
                host,
                status.success().then(|| started.elapsed()),
                Duration::ZERO,
            ),
            Err(err) => {
                let _ = tx.send(Event::Ended(
                    host,
                    HostState::Failed(format!("could not run `{program}`: {err}")),
                ));
                return;
            }
        };
        if tx.send(event).is_err() {
            return;
        }
        thread::sleep(interval);
    });
}

/// Forwards keyboard and resize events from the terminal.
pub fn start_input(tx: Sender<Event>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        while !stop.load(Ordering::Acquire) {
            let event = match tui::crossterm::event::poll(Duration::from_millis(250)) {
                Ok(false) => continue,
                Ok(true) => match tui::crossterm::event::read() {
                    Ok(event) => Event::Input(event),
                    Err(err) => Event::InputFailed(err.to_string()),
                },
                Err(err) => Event::InputFailed(err.to_string()),
            };
            let failed = matches!(event, Event::InputFailed(_));
            if tx.send(event).is_err() || failed {
                return;
            }
        }
    });
}

/// Turns SIGTERM, SIGINT and SIGHUP into an orderly quit, so the terminal is restored
/// even when gping is stopped from outside (`timeout`, `kill`, a closed tmux pane).
#[cfg(unix)]
pub fn start_signals(tx: Sender<Event>) -> Result<()> {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    let mut signals = signal_hook::iterator::Signals::new([SIGTERM, SIGINT, SIGHUP])?;
    thread::spawn(move || {
        if let Some(signal) = signals.forever().next() {
            let _ = tx.send(Event::Signal(signal));
        }
    });
    Ok(())
}

#[cfg(not(unix))]
pub fn start_signals(_tx: Sender<Event>) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Commands used to be split on whitespace, so `sh -c 'exit 0'` ran sh with the
    // arguments ["-c", "'exit", "0'"] and failed every time.
    #[test]
    #[cfg(not(windows))]
    fn commands_honour_shell_quoting() {
        let (program, args) = parse_command("sh -c 'exit 0'").unwrap();
        assert_eq!(program, "sh");
        assert_eq!(args, ["-c", "exit 0"]);
        let (_, args) = parse_command(r#"echo "two words" plain"#).unwrap();
        assert_eq!(args, ["two words", "plain"]);
        let (_, args) = parse_command(r#"echo a\ b "c\"d" 'e\f'"#).unwrap();
        assert_eq!(args, ["a b", "c\"d", "e\\f"]);
        assert!(parse_command("echo 'unterminated").is_err());
    }

    #[test]
    #[cfg(windows)]
    fn windows_paths_keep_their_backslashes() {
        let (program, args) = parse_command(r#""C:\Program Files\curl.exe" -s C:\tmp\x"#).unwrap();
        assert_eq!(program, r"C:\Program Files\curl.exe");
        assert_eq!(args, ["-s", r"C:\tmp\x"]);
    }

    #[test]
    fn empty_or_malformed_commands_are_errors() {
        assert!(parse_command("").is_err());
        assert!(parse_command("   ").is_err());
        assert!(parse_command("echo \"unterminated").is_err());
    }
}

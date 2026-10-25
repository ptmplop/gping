//! The background threads that ping hosts or time commands, feeding results to the UI.
//!
//! Nothing here is ever joined: quitting must not wait for a ping that is due in five
//! seconds or a command that takes a minute. The threads notice that the UI has gone the
//! next time they try to report to it, and stop.

use crate::app::HostState;
use anyhow::{bail, Result};
use pinger::{ping, PingOptions, PingResult};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
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

/// Runs a command over and over, timing each run. A run that exits unsuccessfully counts
/// as lost. The command gets no stdin: it would otherwise read the keys meant for gping.
pub fn start_command(
    (program, args): (String, Vec<String>),
    host: usize,
    interval: Duration,
    tx: Sender<Event>,
) {
    thread::spawn(move || loop {
        let started = Instant::now();
        let child = Command::new(&program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let event = match child.and_then(|mut child| child.wait()) {
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

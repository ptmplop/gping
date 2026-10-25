//! The program proper: set up the hosts, start probing, drive the UI, and leave the
//! terminal the way we found it, with the final frame in the scrollback.

use crate::app::{App, Host, HostState};
use crate::cli::{Args, DEFAULT_CMD_INTERVAL_SECONDS, DEFAULT_PING_INTERVAL_SECONDS};
use crate::color_depth::ColorDepth;
use crate::hosts;
use crate::probes::{self, Event};
use crate::region_map;
use crate::terminal::TerminalGuard;
use crate::theme;
use crate::ui::{KeyOutcome, Ui, UiOptions};
use anyhow::{anyhow, bail, Result};
use pinger::{PingMode, PingOptions};
use std::io::{self, BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tui::backend::CrosstermBackend;
use tui::crossterm::event::{Event as TerminalEvent, KeyCode, KeyEventKind, KeyModifiers};
use tui::crossterm::execute;
use tui::crossterm::terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate};
use tui::Terminal;

/// The quickest the screen is ever redrawn. Frames are only drawn when something changed
/// (a sample, a key, the plot scrolling by a dot), so this is a ceiling, not a rate.
const MAX_FPS: f64 = 15.0;

/// How the run ended.
pub enum Outcome {
    /// Quit by the user, or every probe finished normally.
    Done,
    /// At least one probe failed, or the keyboard could not be read.
    Failed(Vec<String>),
    /// Stopped by this signal: die of it too, once the terminal is restored, so that whoever
    /// sent it (a shell, `timeout`) sees what happened.
    Signalled(i32),
}

pub fn run(args: Args) -> Result<Outcome> {
    if args.hosts_or_commands.is_empty() {
        // The likeliest way to get here with hosts on the command line.
        #[cfg(not(target_os = "windows"))]
        if args.ping_args.as_ref().is_some_and(|a| !a.is_empty()) {
            bail!("--ping-args takes every argument after it, so it has to come after the hosts: gping example.com --ping-args -c 5");
        }
        bail!("At least one host or command must be given (i.e gping google.com). Use --help for a full list of arguments.");
    }
    let ymin = if args.ymin_zero { Some(0) } else { args.ymin };
    validate_yrange(ymin, args.ymax)?;
    let yrange = (ymin.map(ms_to_us), args.ymax.map(ms_to_us));

    let names: Vec<String> = args
        .hosts_or_commands
        .iter()
        .map(|s| region_map::try_host_from_cloud_region(s).unwrap_or_else(|| s.clone()))
        .collect();
    let colors = theme::series_colors(&args.color_codes_or_names, names.len())?;
    let chosen = args.color_codes_or_names.len();
    let depth = ColorDepth::detect();

    // Work everything out before touching the terminal, so that any error is reported on a
    // normal screen.
    let interval = Duration::from_secs_f64(args.watch_interval.unwrap_or(if args.cmd {
        DEFAULT_CMD_INTERVAL_SECONDS
    } else {
        DEFAULT_PING_INTERVAL_SECONDS
    }));
    let mut hosts = Vec::with_capacity(names.len());
    let mut probes = Vec::with_capacity(names.len());
    for (index, (name, (color, tint))) in names.into_iter().zip(colors).enumerate() {
        // Palette colours have a named stand-in on 16-colour terminals; chosen ones are kept.
        let color = match depth {
            ColorDepth::Ansi16 if index >= chosen => {
                theme::SERIES_16[(index - chosen) % theme::SERIES_16.len()]
            }
            _ => color,
        };
        let mut host = if args.cmd {
            probes.push(Probe::Command(probes::parse_command(&name)?));
            Host::new(name, None, color, tint)
        } else {
            let target = hosts::resolve(&name, args.ipv4, args.ipv6)?;
            probes.push(Probe::Ping(ping_options(&args, &target, interval)));
            Host::new(name, target.address.map(|a| a.to_string()), color, tint)
        };
        host.chosen_color = index < chosen;
        hosts.push(host);
    }

    // Declared before the channel, so that it is dropped after it: by then no thread can
    // report a result, and none starts another command.
    let _stopper = probes::Stopper;
    let (tx, rx) = mpsc::channel();
    // Before raw mode, so that a signal arriving during setup is handled the same way.
    probes::start_signals(tx.clone())?;
    let clock = Instant::now();
    for (index, probe) in probes.into_iter().enumerate() {
        match probe {
            Probe::Ping(options) => probes::start_ping(options, index, tx.clone())?,
            Probe::Command(command) => probes::start_command(command, index, interval, tx.clone()),
        }
    }

    let mut app = App::new(hosts, args.buffer as f64, 0.0, yrange);
    let mut ui = Ui::new(UiOptions::from_args(&args, interval, depth));

    let guard = TerminalGuard::enter(args.clear)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(BufWriter::with_capacity(
        1 << 20,
        io::stdout(),
    )))?;
    // terminal.clear() reads the cursor position, which fails in some CI environments.
    execute!(terminal.backend_mut(), Clear(ClearType::All))?;

    let stop_input = Arc::new(AtomicBool::new(false));
    probes::start_input(tx.clone(), Arc::clone(&stop_input));
    drop(tx);

    let frame_interval = Duration::from_secs_f64(1.0 / MAX_FPS);
    let mut dirty = true;
    let mut last_frame = Instant::now() - frame_interval;
    let mut failures = Vec::new();
    let outcome = loop {
        let now = clock.elapsed().as_secs_f64();
        if (dirty || ui.wants_redraw(&app, now)) && last_frame.elapsed() >= frame_interval {
            draw(&mut terminal, &mut ui, &mut app, now, false, depth)?;
            last_frame = Instant::now();
            dirty = false;
        }

        let wait = frame_interval
            .saturating_sub(last_frame.elapsed())
            .max(Duration::from_millis(5));
        let event = match rx.recv_timeout(wait) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break Outcome::Done,
        };
        let now = clock.elapsed().as_secs_f64();
        match event {
            Event::Probe(host, reply, ago) => {
                app.record(host, (now - ago.as_secs_f64()).max(0.0), reply);
                dirty = true;
            }
            Event::Ended(host, state) => {
                if let HostState::Failed(message) = &state {
                    failures.push(format!("{}: {message}", app.hosts[host].name));
                }
                app.hosts[host].state = state;
                dirty = true;
                if app.hosts.iter().all(|h| h.state != HostState::Running) {
                    break Outcome::Done;
                }
            }
            Event::Input(TerminalEvent::Key(key)) if key.kind != KeyEventKind::Release => {
                let quit = matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL));
                if quit {
                    break Outcome::Done;
                }
                if ui.handle_key(key, &mut app, now) == KeyOutcome::Changed {
                    dirty = true;
                }
            }
            Event::Input(TerminalEvent::Resize(..)) => dirty = true,
            Event::Input(_) => {}
            Event::InputFailed(message) => {
                failures.push(format!("Could not read keyboard input: {message}"));
                break Outcome::Done;
            }
            Event::Signal(signal) => break Outcome::Signalled(signal),
        }
    };
    stop_input.store(true, Ordering::Release);

    // One last frame, re-typeset for the scrollback, unless we're about to vanish anyway.
    let now = clock.elapsed().as_secs_f64();
    let last_row = draw(&mut terminal, &mut ui, &mut app, now, true, depth).unwrap_or(0);
    drop(terminal);
    guard.leave(last_row, args.vertical_margin == 0);

    Ok(match outcome {
        Outcome::Signalled(signal) => Outcome::Signalled(signal),
        _ if !failures.is_empty() => Outcome::Failed(failures),
        outcome => outcome,
    })
}

enum Probe {
    Ping(PingOptions),
    Command((String, Vec<String>)),
}

fn ping_options(args: &Args, target: &hosts::Target, interval: Duration) -> PingOptions {
    #[cfg(not(target_os = "windows"))]
    let interface = args.interface.clone();
    #[cfg(target_os = "windows")]
    let interface = None;

    // The probe is always an address (or a scoped IPv6 literal), so its family is known.
    let mut options = if target.ipv6 {
        PingOptions::new_ipv6(&target.probe, interval, interface)
    } else {
        PingOptions::new_ipv4(&target.probe, interval, interface)
    };
    #[cfg(not(target_os = "windows"))]
    if let Some(ping_args) = &args.ping_args {
        options = options.with_raw_arguments(ping_args.clone());
    }
    if args.tcp {
        options = options.with_mode(PingMode::TCP {
            rst: args.tcp_rst.into(),
            port: Some(args.tcp_port),
        });
    }
    options
}

/// Draws a frame and returns the last row it used.
fn draw<W: Write>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    ui: &mut Ui,
    app: &mut App,
    now: f64,
    closing: bool,
    depth: ColorDepth,
) -> Result<u16> {
    let mut last_row = 0;
    // Terminals that support it show the frame all at once, instead of drawing it as it
    // arrives (a torn frame); the rest ignore the request.
    execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let drawn = terminal
        .draw(|frame| {
            last_row = ui.draw(frame, app, now, closing);
            depth.adapt_buffer(frame.buffer_mut());
        })
        .map(|_| ());
    // Ended even if the draw failed, or the terminal would hold every later update back.
    let ended = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    drawn?;
    ended?;
    Ok(last_row)
}

fn ms_to_us(ms: u64) -> f64 {
    ms as f64 * 1_000.0
}

/// An inverted or empty vertical range leaves nothing sensible to draw, so reject it up
/// front rather than quietly picking some other window to display.
pub fn validate_yrange(ymin: Option<u64>, ymax: Option<u64>) -> Result<()> {
    if let (Some(min), Some(max)) = (ymin, ymax) {
        if min >= max {
            bail!("Invalid vertical axis range: the minimum ({min}ms) must be less than the maximum ({max}ms)");
        }
    }
    if ymax == Some(0) {
        bail!("Invalid vertical axis range: the maximum must be more than 0ms");
    }
    Ok(())
}

/// Reports how the run ended, once the terminal is back to normal, and picks the exit code.
pub fn finish(outcome: Result<Outcome>) -> Result<()> {
    match outcome? {
        Outcome::Done => Ok(()),
        Outcome::Failed(failures) => Err(anyhow!(failures.join("\n"))),
        Outcome::Signalled(signal) => {
            #[cfg(unix)]
            signal_hook::low_level::emulate_default_handler(signal)?;
            #[cfg(not(unix))]
            let _ = signal;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_inverted_or_empty_yrange_is_rejected() {
        assert!(validate_yrange(Some(10), Some(2)).is_err());
        assert!(validate_yrange(Some(5), Some(5)).is_err());
        assert!(validate_yrange(None, Some(0)).is_err());
    }

    #[test]
    fn a_valid_yrange_is_accepted() {
        assert!(validate_yrange(Some(0), Some(10)).is_ok());
        assert!(validate_yrange(Some(10), None).is_ok());
        assert!(validate_yrange(None, Some(10)).is_ok());
        assert!(validate_yrange(None, None).is_ok());
    }
}

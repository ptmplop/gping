//! Putting the terminal into, and back out of, the state the UI needs.
//!
//! [`TerminalGuard`] restores the terminal when it is dropped, so every way out of `main`
//! (a normal quit, an error bubbling up through `?`, or a panic, via the hook it installs)
//! leaves the user with a working shell rather than one stuck in raw mode. Signals are
//! turned into an ordinary quit elsewhere (see `probes::start_signals`), so they come
//! through here too.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use tui::crossterm::cursor::{Hide, MoveTo, Show};
use tui::crossterm::execute;
use tui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};

/// Set once the terminal has been restored. A panic on the main thread reaches both the
/// panic hook and the guard's drop; restoring twice would leave the alternate screen twice,
/// and xterm and iTerm2 move the cursor back on each, over the panic message.
static RESTORED: AtomicBool = AtomicBool::new(false);

pub struct TerminalGuard {
    alternate_screen: bool,
}

impl TerminalGuard {
    pub fn enter(alternate_screen: bool) -> io::Result<TerminalGuard> {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore(alternate_screen, None);
            hook(info);
        }));

        enable_raw_mode()?;
        // From here on the guard puts the terminal back, even if the rest fails (as it does
        // when stdout is a closed pipe).
        let guard = TerminalGuard { alternate_screen };
        let mut stdout = io::stdout();
        if alternate_screen {
            execute!(stdout, EnterAlternateScreen)?;
        }
        execute!(stdout, Hide)?;
        Ok(guard)
    }

    /// Restores the terminal now. On the main screen the cursor is left at the start of
    /// `row`, under the final frame, so the graph stays intact above the prompt. With
    /// `newline`, a line feed first makes room for the prompt by scrolling the frame up one
    /// line (for when `row` is the frame's own last row rather than a blank one under it).
    pub fn leave(self, row: u16, newline: bool) {
        restore(self.alternate_screen, Some((row, newline)));
        std::mem::forget(self);
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore(self.alternate_screen, None);
    }
}

fn restore(alternate_screen: bool, park: Option<(u16, bool)>) {
    if RESTORED.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut stdout = io::stdout();
    let _ = disable_raw_mode();
    if alternate_screen {
        let _ = execute!(stdout, LeaveAlternateScreen);
    } else if let Some((row, newline)) = park {
        let _ = execute!(stdout, MoveTo(0, row));
        if newline {
            // An explicit carriage return, so this is right even if raw mode couldn't be left.
            let _ = stdout.write_all(b"\r\n");
        }
    }
    let _ = execute!(stdout, Show);
    let _ = stdout.flush();
}

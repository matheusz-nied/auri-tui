use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind, MouseEvent};

/// How long `poll` waits for input before giving up.
const POLL_TIMEOUT: Duration = Duration::from_millis(250);
/// How often a `Tick` is emitted (drives `Action::Refresh` in `App`).
const TICK_RATE: Duration = Duration::from_secs(2);

/// Events emitted by [`Events`]. `Tick` is a periodic heartbeat with no
/// crossterm counterpart.
#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    Tick,
}

/// Wraps crossterm event polling and injects periodic ticks.
pub struct Events {
    last_tick: Instant,
}

impl Events {
    /// Wait up to `POLL_TIMEOUT` for the next event. Returns `Ok(None)` on a
    /// plain timeout so the caller can redraw; key-release events (sent by some
    /// terminals/platforms) are filtered out.
    pub fn poll_event(&mut self) -> Result<Option<AppEvent>> {
        loop {
            if event::poll(POLL_TIMEOUT)? {
                match event::read()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        return Ok(Some(AppEvent::Key(key)));
                    }
                    Event::Key(_) => continue,
                    Event::Mouse(mouse) => return Ok(Some(AppEvent::Mouse(mouse))),
                    Event::Resize(w, h) => return Ok(Some(AppEvent::Resize(w, h))),
                    _ => continue,
                }
            }
            if self.last_tick.elapsed() >= TICK_RATE {
                self.last_tick = Instant::now();
                return Ok(Some(AppEvent::Tick));
            }
            return Ok(None);
        }
    }
}

impl Default for Events {
    fn default() -> Self {
        Self {
            last_tick: Instant::now(),
        }
    }
}

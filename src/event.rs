use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind, MouseEvent};

/// How long `poll_event` waits for input when nothing else is going on.
/// Also bounds how late a filesystem-watcher signal is picked up (the
/// wait can't be interrupted), so a change shows up well under 0.5 s.
pub const POLL_TIMEOUT: Duration = Duration::from_millis(100);
/// How often a `Tick` is emitted (drives `Action::Refresh` in `App`) when
/// nothing tells us the repo changed — no filesystem watcher.
pub const POLL_TICK: Duration = Duration::from_secs(2);
/// The tick with a filesystem watcher running: only a safety net (network
/// filesystems, a watcher that silently misses events).
pub const WATCH_TICK: Duration = Duration::from_secs(30);
/// Max events `drain` reads per frame — a bound so an endless input stream
/// can't starve rendering forever.
const MAX_DRAIN: usize = 256;

/// Events emitted by [`Events`]. `Tick` is a periodic heartbeat with no
/// crossterm counterpart.
#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    /// Text pasted while bracketed paste is on — one event, newlines
    /// included, instead of a key press per character.
    Paste(String),
    Tick,
}

/// Wraps crossterm event polling and injects periodic ticks.
pub struct Events {
    last_tick: Instant,
    tick_rate: Duration,
}

impl Events {
    pub fn tick_rate(&self) -> Duration {
        self.tick_rate
    }

    pub fn set_tick_rate(&mut self, rate: Duration) {
        self.tick_rate = rate;
    }

    /// Wait up to `timeout` for the next event. Returns `Ok(None)` on a
    /// plain timeout so the caller can redraw; key-release events (sent by some
    /// terminals/platforms) are filtered out.
    pub fn poll_event(&mut self, timeout: Duration) -> Result<Option<AppEvent>> {
        loop {
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        return Ok(Some(AppEvent::Key(key)));
                    }
                    Event::Key(_) => continue,
                    Event::Mouse(mouse) => return Ok(Some(AppEvent::Mouse(mouse))),
                    Event::Resize(w, h) => return Ok(Some(AppEvent::Resize(w, h))),
                    Event::Paste(text) => return Ok(Some(AppEvent::Paste(text))),
                    _ => continue,
                }
            }
            if self.last_tick.elapsed() >= self.tick_rate {
                self.last_tick = Instant::now();
                return Ok(Some(AppEvent::Tick));
            }
            return Ok(None);
        }
    }

    /// Read every event already queued without blocking (cap `MAX_DRAIN`).
    /// Call after `poll_event` so input bursts — e.g. dozens of trackpad
    /// scroll events — are all handled before the next redraw instead of
    /// paying one full render per event.
    pub fn drain(&mut self) -> Result<Vec<AppEvent>> {
        let mut out = Vec::new();
        while out.len() < MAX_DRAIN {
            if !event::poll(Duration::ZERO)? {
                break;
            }
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    out.push(AppEvent::Key(key));
                }
                Event::Key(_) => {}
                Event::Mouse(mouse) => out.push(AppEvent::Mouse(mouse)),
                Event::Resize(w, h) => out.push(AppEvent::Resize(w, h)),
                Event::Paste(text) => out.push(AppEvent::Paste(text)),
                _ => {}
            }
        }
        Ok(out)
    }
}

impl Default for Events {
    fn default() -> Self {
        Self {
            last_tick: Instant::now(),
            tick_rate: POLL_TICK,
        }
    }
}

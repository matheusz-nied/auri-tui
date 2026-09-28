use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::Frame;

use crate::action::Action;

/// A UI panel. Components are pure UI: they may keep local state, return
/// `Action`s from event handlers, and observe broadcast actions in `update`.
/// They must never perform I/O or call git themselves — side effects belong to
/// `App`, which reports results back via `Action`s like `StatusLoaded`.
///
/// To add a new panel: implement this trait, add a `PanelId`, and register the
/// component in `App::new`. Any new actions it needs should be added to
/// `Action` and handled in `App::execute` or emitted by other components.
pub trait Component {
    /// Handle a key press while this component is focused. Return an action to
    /// enqueue, or `None`.
    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        let _ = key;
        None
    }

    /// Handle a mouse event. `area` is the rect where the component was last
    /// rendered (including its border), so `ev.row - area.y - 1` gives a row
    /// inside the component.
    fn handle_mouse(&mut self, ev: MouseEvent, area: Rect) -> Option<Action> {
        let _ = (ev, area);
        None
    }

    /// React to a broadcast action. May itself return a follow-up action.
    fn update(&mut self, action: &Action) -> Option<Action> {
        let _ = action;
        None
    }

    /// Draw the component into `area`. `focused` indicates it owns the
    /// keyboard focus (highlight the border).
    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool);
}

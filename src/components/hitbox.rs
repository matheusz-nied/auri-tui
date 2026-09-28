//! Reusable click hit-testing: components record where they drew clickable
//! things during `render` and resolve clicks in `handle_mouse`.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;

use crate::action::Action;

/// Clickable regions drawn during the last `render`. Entries keep the screen
/// rect and the action a click there should produce.
#[derive(Default)]
pub struct Hitboxes(Vec<(Rect, Action)>);

impl Hitboxes {
    /// Call at the start of every `render`.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn push(&mut self, rect: Rect, action: Action) {
        self.0.push((rect, action));
    }

    /// The action registered for `(col, row)`, if any. On overlap the most
    /// recently pushed rect wins.
    pub fn hit(&self, col: u16, row: u16) -> Option<Action> {
        self.0
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains((col, row).into()))
            .map(|(_, action)| action.clone())
    }
}

/// A `" {label} "` span for a 3-column-wide button glyph like `+` or `↶`.
pub fn button_span(label: &str, style: Style) -> Span<'static> {
    Span::styled(format!(" {label} "), style)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_returns_action_inside_rect() {
        let mut h = Hitboxes::default();
        h.push(Rect::new(2, 1, 3, 1), Action::StageAll);
        assert!(matches!(h.hit(2, 1), Some(Action::StageAll)));
        assert!(matches!(h.hit(4, 1), Some(Action::StageAll)));
        assert!(h.hit(5, 1).is_none());
        assert!(h.hit(3, 0).is_none());
        assert!(h.hit(0, 0).is_none());
    }

    #[test]
    fn last_pushed_wins_on_overlap() {
        let mut h = Hitboxes::default();
        h.push(Rect::new(0, 0, 5, 2), Action::StageAll);
        h.push(Rect::new(2, 0, 3, 1), Action::UnstageAll);
        assert!(matches!(h.hit(2, 0), Some(Action::UnstageAll)));
        assert!(matches!(h.hit(0, 0), Some(Action::StageAll)));
    }

    #[test]
    fn clear_empties() {
        let mut h = Hitboxes::default();
        h.push(Rect::new(0, 0, 1, 1), Action::StageAll);
        h.clear();
        assert!(h.hit(0, 0).is_none());
    }
}

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::action::Action;
use crate::component::Component;
use crate::theme;

use super::hitbox::Hitboxes;

/// Modal confirmation overlay. Opens when an `Action::Confirm` is broadcast;
/// while open it captures all input. `y`/Enter-on-confirm/clicking the
/// confirm button returns the wrapped action; `n`/Esc/`q`, Enter on Cancel,
/// or a click outside the box closes it and returns `None`.
#[derive(Default)]
pub struct ConfirmDialog {
    /// (prompt, confirm_label, action to run on confirm)
    open: Option<(String, String, Action)>,
    /// Which button is highlighted: false = Cancel (the default), true = the
    /// confirm button.
    confirm_selected: bool,
    hitboxes: Hitboxes,
    dialog: Rect,
    confirm_rect: Rect,
}

impl ConfirmDialog {
    fn close(&mut self) -> Option<Action> {
        self.open = None;
        self.confirm_selected = false;
        None
    }

    fn confirm(&mut self) -> Option<Action> {
        self.confirm_selected = false;
        self.open.take().map(|(_, _, then)| then)
    }
}

impl Component for ConfirmDialog {
    fn captures_input(&self) -> bool {
        self.open.is_some()
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        self.open.as_ref()?;
        match key.code {
            KeyCode::Char('y') => self.confirm(),
            KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => self.close(),
            KeyCode::Enter => {
                if self.confirm_selected {
                    self.confirm()
                } else {
                    self.close()
                }
            }
            KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                self.confirm_selected = !self.confirm_selected;
                None
            }
            _ => None,
        }
    }

    fn handle_mouse(&mut self, ev: MouseEvent, _area: Rect) -> Option<Action> {
        self.open.as_ref()?;
        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            if self.hitboxes.hit(ev.column, ev.row).is_some() {
                if self.confirm_rect.contains((ev.column, ev.row).into()) {
                    return self.confirm();
                }
                return self.close();
            }
            if !self.dialog.contains((ev.column, ev.row).into()) {
                return self.close();
            }
        }
        None
    }

    fn update(&mut self, action: &Action) -> Option<Action> {
        if let Action::Confirm {
            prompt,
            confirm_label,
            then,
        } = action
        {
            self.open = Some((prompt.clone(), confirm_label.clone(), (**then).clone()));
            self.confirm_selected = false;
        }
        None
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        self.hitboxes.clear();
        let Some((prompt, confirm_label, then)) = &self.open else {
            return;
        };

        let width = 50.min(area.width).max(20);
        let height = 5.min(area.height);
        let dialog = Rect::new(
            area.x + (area.width.saturating_sub(width)) / 2,
            area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        );
        self.dialog = dialog;

        f.render_widget(Clear, dialog);
        let block = Block::bordered()
            .border_style(theme::border(true))
            .style(theme::base());
        let inner = block.inner(dialog);
        f.render_widget(block, dialog);
        f.render_widget(
            Paragraph::new(prompt.clone()).wrap(Wrap { trim: false }),
            inner,
        );

        // Button row at the bottom of the box, centered: "[ Cancel ]" then
        // "[ Discard ]" (or the given confirm label).
        let cancel = "[ Cancel ]".to_string();
        let confirm = format!("[ {confirm_label} ]");
        let cancel_w = cancel.chars().count() as u16;
        let confirm_w = confirm.chars().count() as u16;
        let total = cancel_w + 1 + confirm_w;
        let y = inner.y + inner.height.saturating_sub(1);
        let x = inner.x + (inner.width.saturating_sub(total)) / 2;
        let cancel_rect = Rect::new(x, y, cancel_w, 1);
        let confirm_rect = Rect::new(x + cancel_w + 1, y, confirm_w, 1);
        self.confirm_rect = confirm_rect;

        let cancel_style = if self.confirm_selected {
            theme::muted()
        } else {
            Style::default()
                .fg(theme::INK)
                .bg(theme::LINE_2)
                .add_modifier(Modifier::BOLD)
        };
        let discard_style = if self.confirm_selected {
            Style::default()
                .fg(theme::ON_GOLD)
                .bg(theme::DEL_FG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::DEL_FG).bg(theme::DEL_BG)
        };
        f.render_widget(Span::styled(cancel, cancel_style), cancel_rect);
        f.render_widget(Span::styled(confirm, discard_style), confirm_rect);

        // The cancel button's payload is ignored — `handle_mouse` checks
        // `confirm_rect` to decide which button was hit.
        self.hitboxes.push(cancel_rect, then.clone());
        self.hitboxes.push(confirm_rect, then.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char) -> KeyEvent {
        KeyEvent::from(KeyCode::Char(c))
    }

    fn confirm_action() -> Action {
        Action::Confirm {
            prompt: "Discard changes?".to_string(),
            confirm_label: "Discard".to_string(),
            then: Box::new(Action::UnstageAll),
        }
    }

    #[test]
    fn opens_on_confirm_and_captures_input() {
        let mut d = ConfirmDialog::default();
        assert!(!d.captures_input());
        assert!(d.update(&confirm_action()).is_none());
        assert!(d.captures_input());
    }

    #[test]
    fn y_returns_then_action_and_closes() {
        let mut d = ConfirmDialog::default();
        d.update(&confirm_action());
        let act = d.handle_key(key('y'));
        assert!(matches!(act, Some(Action::UnstageAll)));
        assert!(!d.captures_input());
    }

    #[test]
    fn esc_and_enter_default_cancel() {
        let mut d = ConfirmDialog::default();
        d.update(&confirm_action());
        assert!(d.handle_key(KeyEvent::from(KeyCode::Esc)).is_none());
        assert!(!d.captures_input());

        d.update(&confirm_action());
        // Cancel is highlighted by default, so Enter cancels.
        assert!(d.handle_key(KeyEvent::from(KeyCode::Enter)).is_none());
        assert!(!d.captures_input());
    }

    #[test]
    fn tab_then_enter_confirms() {
        let mut d = ConfirmDialog::default();
        d.update(&confirm_action());
        assert!(d.handle_key(KeyEvent::from(KeyCode::Tab)).is_none());
        let act = d.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(act, Some(Action::UnstageAll)));
        assert!(!d.captures_input());
    }
}

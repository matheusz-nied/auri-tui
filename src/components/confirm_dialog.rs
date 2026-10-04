use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::action::Action;
use crate::component::Component;
use crate::text;
use crate::theme;

/// An open dialog: what `Action::Confirm` carried.
struct Dialog {
    prompt: String,
    confirm_label: String,
    then: Action,
    alt: Option<(String, Action)>,
}

impl Dialog {
    /// Button labels left to right: Cancel, the alternative (if any), the
    /// confirm button.
    fn labels(&self) -> Vec<&str> {
        let mut out = vec!["Cancel"];
        out.extend(self.alt.as_ref().map(|(l, _)| l.as_str()));
        out.push(&self.confirm_label);
        out
    }
}

/// Modal confirmation overlay. Opens when an `Action::Confirm` is broadcast;
/// while open it captures all input. `y`, or Enter / a click on the confirm
/// button, returns the wrapped action; the alternative button (when given)
/// answers to its label's first letter, Enter or a click. `n`/Esc/`q`,
/// Enter on Cancel, or a click outside the box closes it and returns `None`.
#[derive(Default)]
pub struct ConfirmDialog {
    open: Option<Dialog>,
    /// Highlighted button, an index into `Dialog::labels` (0 = Cancel, the
    /// default).
    selected: usize,
    dialog: Rect,
    /// Where each button was drawn, same order as `labels`.
    buttons: Vec<Rect>,
}

impl ConfirmDialog {
    fn close(&mut self) -> Option<Action> {
        self.open = None;
        self.selected = 0;
        None
    }

    /// Activate button `i` (0 = Cancel, last = confirm).
    fn press(&mut self, i: usize) -> Option<Action> {
        let dialog = self.open.take()?;
        self.selected = 0;
        let last = dialog.labels().len() - 1;
        match (i, dialog.alt) {
            (0, _) => None,
            (i, _) if i == last => Some(dialog.then),
            (_, Some((_, alt))) => Some(alt),
            (_, None) => None,
        }
    }

    fn count(&self) -> usize {
        self.open.as_ref().map_or(0, |d| d.labels().len())
    }
}

impl Component for ConfirmDialog {
    fn captures_input(&self) -> bool {
        self.open.is_some()
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        let dialog = self.open.as_ref()?;
        let n = self.count();
        let alt_key = dialog
            .alt
            .as_ref()
            .and_then(|(l, _)| l.chars().next())
            .map(|c| c.to_ascii_lowercase());
        match key.code {
            KeyCode::Char('y') => self.press(n - 1),
            KeyCode::Char(c) if Some(c) == alt_key => self.press(1),
            KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => self.close(),
            KeyCode::Enter => self.press(self.selected),
            KeyCode::Tab | KeyCode::Right => {
                self.selected = (self.selected + 1) % n;
                None
            }
            KeyCode::BackTab | KeyCode::Left => {
                self.selected = (self.selected + n - 1) % n;
                None
            }
            _ => None,
        }
    }

    fn handle_mouse(&mut self, ev: MouseEvent, _area: Rect) -> Option<Action> {
        self.open.as_ref()?;
        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            let pos = (ev.column, ev.row).into();
            if let Some(i) = self.buttons.iter().position(|r| r.contains(pos)) {
                return self.press(i);
            }
            if !self.dialog.contains(pos) {
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
            alt,
        } = action
        {
            self.open = Some(Dialog {
                prompt: prompt.clone(),
                confirm_label: confirm_label.clone(),
                then: (**then).clone(),
                alt: alt.as_ref().map(|(l, a)| (l.clone(), (**a).clone())),
            });
            self.selected = 0;
        }
        None
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        self.buttons.clear();
        let Some(dialog) = &self.open else {
            return;
        };

        let width = 56.min(area.width).max(20.min(area.width));
        // Prompt rows (wrapped) + a blank row + the buttons, inside borders.
        let prompt_rows =
            text::width(&dialog.prompt).div_ceil(width.saturating_sub(2).max(1) as usize);
        let height = (prompt_rows as u16 + 4).max(5).min(area.height);
        let rect = Rect::new(
            area.x + (area.width.saturating_sub(width)) / 2,
            area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        );
        self.dialog = rect;

        f.render_widget(Clear, rect);
        let block = Block::bordered()
            .border_style(theme::border(true))
            .style(theme::base());
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        f.render_widget(
            Paragraph::new(dialog.prompt.clone()).wrap(Wrap { trim: false }),
            inner,
        );

        // Button row at the bottom of the box, centered: "[ Cancel ]",
        // "[ Discard ]"…, one column apart.
        let labels: Vec<String> = dialog.labels().iter().map(|l| format!("[ {l} ]")).collect();
        let total = labels
            .iter()
            .map(|l| text::width(l) as u16 + 1)
            .sum::<u16>()
            - 1;
        let y = inner.y + inner.height.saturating_sub(1);
        let mut x = inner.x + (inner.width.saturating_sub(total)) / 2;
        let last = labels.len() - 1;
        for (i, label) in labels.into_iter().enumerate() {
            let w = text::width(&label) as u16;
            let r = Rect::new(x, y, w.min(inner.right().saturating_sub(x)), 1);
            let style = button_style(i, last, dialog.alt.is_some(), i == self.selected);
            f.render_widget(Span::styled(label, style), r);
            self.buttons.push(r);
            x += w + 1;
        }
    }
}

/// Cancel is neutral; the destructive button red: the confirm button in a
/// two-button dialog, the alternative in a three-button one (whose confirm
/// button is the safe, gold primary). Highlighted = filled.
fn button_style(i: usize, last: usize, has_alt: bool, selected: bool) -> Style {
    let danger = if has_alt { i == 1 } else { i == last };
    let bold = Modifier::BOLD;
    match (i, danger, selected) {
        (0, _, true) => Style::default()
            .fg(theme::INK)
            .bg(theme::LINE_2)
            .add_modifier(bold),
        (0, _, false) => theme::muted(),
        (_, true, true) => Style::default()
            .fg(theme::ON_GOLD)
            .bg(theme::DEL_FG)
            .add_modifier(bold),
        (_, true, false) => Style::default().fg(theme::DEL_FG).bg(theme::DEL_BG),
        (_, false, true) => Style::default()
            .fg(theme::ON_GOLD)
            .bg(theme::GOLD)
            .add_modifier(bold),
        (_, false, false) => Style::default().fg(theme::GOLD).bg(theme::PANEL),
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
            alt: None,
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

    fn three_buttons() -> Action {
        Action::Confirm {
            prompt: "Save changes?".to_string(),
            confirm_label: "Save".to_string(),
            then: Box::new(Action::StageAll),
            alt: Some(("Discard".to_string(), Box::new(Action::UnstageAll))),
        }
    }

    #[test]
    fn alternative_button_by_key_tab_and_click() {
        let mut d = ConfirmDialog::default();
        d.update(&three_buttons());
        assert!(matches!(d.handle_key(key('d')), Some(Action::UnstageAll)));

        d.update(&three_buttons());
        assert!(matches!(d.handle_key(key('y')), Some(Action::StageAll)));

        // Tab: Cancel -> Discard -> Save.
        d.update(&three_buttons());
        d.handle_key(KeyEvent::from(KeyCode::Tab));
        d.handle_key(KeyEvent::from(KeyCode::Tab));
        let act = d.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(act, Some(Action::StageAll)));

        // Clicks hit the button drawn there.
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 10)).unwrap();
        d.update(&three_buttons());
        term.draw(|f| d.render(f, f.area(), true)).unwrap();
        let discard = d.buttons[1];
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: discard.x + 1,
            row: discard.y,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        };
        assert!(matches!(
            d.handle_mouse(click, Rect::default()),
            Some(Action::UnstageAll)
        ));
        assert!(!d.captures_input());
    }
}

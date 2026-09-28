use std::collections::{HashMap, VecDeque};

use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};

use crate::action::{Action, PanelId};
use crate::component::Component;
use crate::components::changes::Changes;
use crate::components::commit_input::CommitInput;
use crate::components::diff_view::DiffView;
use crate::event::{AppEvent, Events};
use crate::git::{FileChange, GitBackend, Section};

/// Owns the components, the focus state, the last-frame layout rects and the
/// action queue. It is the *only* place where `GitBackend` is called: side
/// effects requested by components (`Refresh`, `ToggleStage`, `Commit`,
/// `SelectFile`) are executed here and their results are broadcast back to all
/// components as new actions.
pub struct App {
    git: Box<dyn GitBackend>,
    events: Events,
    /// Panels in focus-cycling order.
    components: Vec<(PanelId, Box<dyn Component>)>,
    focus: PanelId,
    queue: VecDeque<Action>,
    /// Rect each panel was rendered into last frame, used for click hit-tests.
    rects: HashMap<PanelId, Rect>,
    status_bar: Rect,
    /// Last status snapshot, for the "nothing staged" commit check.
    last_status: Vec<FileChange>,
    branch: String,
    selected: Option<FileChange>,
    /// Last diff doc broadcast to components — compared against fresh loads so
    /// unchanged diffs aren't re-sent on every refresh tick.
    last_diff: Option<crate::git::DiffDoc>,
    /// (message, is_error) shown in the status bar.
    message: Option<(String, bool)>,
    running: bool,
}

impl App {
    pub fn new(git: Box<dyn GitBackend>) -> Self {
        Self {
            git,
            events: Events::default(),
            components: vec![
                (PanelId::CommitInput, Box::new(CommitInput::default())),
                (PanelId::Changes, Box::new(Changes::default())),
                (PanelId::DiffView, Box::new(DiffView::default())),
            ],
            focus: PanelId::Changes,
            queue: VecDeque::new(),
            rects: HashMap::new(),
            status_bar: Rect::default(),
            last_status: Vec::new(),
            branch: String::new(),
            selected: None,
            last_diff: None,
            message: None,
            running: true,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        self.enqueue(Action::Refresh);
        while self.running {
            terminal.draw(|f| self.render(f))?;
            match self.events.poll_event()? {
                Some(AppEvent::Key(key)) => self.on_key(key),
                Some(AppEvent::Mouse(mouse)) => self.on_mouse(mouse),
                // Resize: the next draw picks up the new size automatically.
                Some(AppEvent::Resize(..)) | None => {}
                Some(AppEvent::Tick) => self.enqueue(Action::Refresh),
            }
            self.dispatch();
        }
        Ok(())
    }

    fn enqueue(&mut self, action: Action) {
        self.queue.push_back(action);
    }

    /// Drain the queue: broadcast every action to all components (`update`)
    /// and execute the side-effecting ones locally. Actions emitted along the
    /// way are processed in order.
    fn dispatch(&mut self) {
        while let Some(action) = self.queue.pop_front() {
            for (_, comp) in &mut self.components {
                if let Some(next) = comp.update(&action) {
                    self.queue.push_back(next);
                }
            }
            self.execute(action);
        }
    }

    /// App-side handling: focus changes and git side effects.
    fn execute(&mut self, action: Action) {
        match action {
            Action::Quit => self.running = false,
            Action::FocusNext => self.cycle_focus(1),
            Action::FocusPrev => self.cycle_focus(-1),
            Action::Focus(id) => self.focus = id,
            Action::Refresh => {
                match self.git.status() {
                    Ok(files) => self.enqueue(Action::StatusLoaded(files)),
                    Err(e) => self.enqueue(Action::Error(format!("status: {e}"))),
                }
                match self.git.branch() {
                    Ok(b) => self.enqueue(Action::BranchLoaded(b)),
                    Err(e) => self.enqueue(Action::Error(format!("branch: {e}"))),
                }
            }
            Action::StatusLoaded(files) => {
                // The broadcast above ran first, so if `Changes` had to move
                // the selection (file committed/moved/first load) its
                // SelectFile is already queued — that action loads the new
                // file's diff, so reloading the old one would be stale work.
                let select_queued = self
                    .queue
                    .iter()
                    .any(|a| matches!(a, Action::SelectFile(_)));
                self.last_status = files;
                if !select_queued {
                    self.reload_selected_diff();
                }
            }
            Action::SelectFile(file) => {
                self.selected = Some(file.clone());
                match self.git.diff(&file) {
                    Ok(doc) => {
                        self.last_diff = Some(doc.clone());
                        self.enqueue(Action::DiffLoaded(doc));
                    }
                    Err(e) => self.enqueue(Action::Error(format!("diff: {e}"))),
                }
            }
            Action::ToggleStage(file) => {
                let result = if file.section == Section::Staged {
                    self.git.unstage(&file.path)
                } else {
                    self.git.stage(&file.path)
                };
                match result {
                    // Changes re-selects the file in its new section on the
                    // next StatusLoaded, which emits a fresh SelectFile.
                    Ok(()) => self.enqueue(Action::Refresh),
                    Err(e) => self.enqueue(Action::Error(format!("stage: {e}"))),
                }
            }
            Action::StageAll => match self.git.stage_all() {
                Ok(()) => self.enqueue(Action::Refresh),
                Err(e) => self.enqueue(Action::Error(format!("stage: {e}"))),
            },
            Action::Commit(msg) => {
                let staged = self
                    .last_status
                    .iter()
                    .any(|f| f.section == Section::Staged);
                if !staged {
                    self.enqueue(Action::Error("Nothing staged".to_string()));
                } else {
                    match self.git.commit(&msg) {
                        Ok(()) => {
                            self.enqueue(Action::CommitDone);
                            self.enqueue(Action::Refresh);
                        }
                        Err(e) => self.enqueue(Action::Error(format!("commit: {e}"))),
                    }
                }
            }
            Action::CommitDone => {
                self.message = Some(("Committed".to_string(), false));
            }
            Action::Error(msg) => {
                self.message = Some((msg, true));
            }
            Action::BranchLoaded(branch) => self.branch = branch,
            Action::DiffLoaded(_) | Action::DiffReloaded(_) => {}
        }
    }

    /// Reload the diff of the currently selected file, if it still exists in
    /// `last_status`. Broadcasts `DiffReloaded` only when the doc changed.
    fn reload_selected_diff(&mut self) {
        let Some(sel) = &self.selected else {
            return;
        };
        let still_there = self
            .last_status
            .iter()
            .any(|f| f.path == sel.path && f.section == sel.section);
        if !still_there {
            return;
        }
        match self.git.diff(sel) {
            Ok(doc) if self.last_diff.as_ref() != Some(&doc) => {
                self.last_diff = Some(doc.clone());
                self.enqueue(Action::DiffReloaded(doc));
            }
            Ok(_) => {}
            Err(e) => self.enqueue(Action::Error(format!("diff: {e}"))),
        }
    }

    fn cycle_focus(&mut self, delta: isize) {
        let len = self.components.len() as isize;
        let cur = self
            .components
            .iter()
            .position(|(id, _)| *id == self.focus)
            .unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(len);
        self.focus = self.components[next as usize].0;
    }

    fn on_key(&mut self, key: KeyEvent) {
        let commit_focused = self.focus == PanelId::CommitInput;
        // When the commit input is focused, only these keys are global —
        // everything else is text input.
        let global = if commit_focused {
            matches!(key.code, KeyCode::Tab | KeyCode::BackTab | KeyCode::Esc)
                || key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
        } else {
            true
        };
        if global {
            match (key.code, key.modifiers) {
                (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
                    self.enqueue(Action::Quit);
                    return;
                }
                (KeyCode::Tab, _) => {
                    self.enqueue(Action::FocusNext);
                    return;
                }
                (KeyCode::BackTab, _) => {
                    self.enqueue(Action::FocusPrev);
                    return;
                }
                (KeyCode::Esc, _) if commit_focused => {
                    self.enqueue(Action::FocusNext);
                    return;
                }
                _ if !commit_focused => match key.code {
                    KeyCode::Char('q') => {
                        self.enqueue(Action::Quit);
                        return;
                    }
                    KeyCode::Char('r') => {
                        self.enqueue(Action::Refresh);
                        return;
                    }
                    KeyCode::Char('c') => {
                        self.enqueue(Action::Focus(PanelId::CommitInput));
                        return;
                    }
                    KeyCode::Char('1') => {
                        self.enqueue(Action::Focus(PanelId::Changes));
                        return;
                    }
                    KeyCode::Char('2') => {
                        self.enqueue(Action::Focus(PanelId::DiffView));
                        return;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if let Some((_, comp)) = self.components.iter_mut().find(|(id, _)| *id == self.focus) {
            if let Some(action) = comp.handle_key(key) {
                self.enqueue(action);
            }
        }
    }

    fn on_mouse(&mut self, ev: ratatui::crossterm::event::MouseEvent) {
        use ratatui::crossterm::event::MouseEventKind;
        let pos = (ev.column, ev.row);
        // Events go to the panel under the cursor (e.g. wheel over the diff
        // scrolls it while the changes list keeps focus); only button presses
        // move focus. Events outside all panels are ignored.
        let Some((id, area)) = self
            .rects
            .iter()
            .find(|(_, r)| r.contains(pos.into()))
            .map(|(id, r)| (*id, *r))
        else {
            return;
        };
        if matches!(ev.kind, MouseEventKind::Down(_)) {
            self.focus = id;
        }
        if let Some((_, comp)) = self.components.iter_mut().find(|(cid, _)| *cid == id) {
            if let Some(action) = comp.handle_mouse(ev, area) {
                self.enqueue(action);
            }
        }
    }

    fn render(&mut self, f: &mut Frame) {
        let area = f.area();
        let vertical = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
        let main = Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(vertical[0]);
        let left = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(main[0]);

        self.rects.insert(PanelId::CommitInput, left[0]);
        self.rects.insert(PanelId::Changes, left[1]);
        self.rects.insert(PanelId::DiffView, main[1]);
        self.status_bar = vertical[1];

        for (id, comp) in &mut self.components {
            if let Some(rect) = self.rects.get(id).copied() {
                comp.render(f, rect, *id == self.focus);
            }
        }
        self.render_status_bar(f);
    }

    fn render_status_bar(&self, f: &mut Frame) {
        let area = self.status_bar;
        let branch = if self.branch.is_empty() {
            "-".to_string()
        } else {
            self.branch.clone()
        };
        let mut spans = vec![
            Span::styled(format!(" {branch}"), Style::default().fg(Color::Cyan)),
            Span::styled(
                "  q quit · tab focus · r refresh · space stage · a stage all · c commit",
                Style::default().fg(Color::DarkGray),
            ),
        ];
        if let Some((msg, is_error)) = &self.message {
            let style = if *is_error {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::Green)
            };
            spans.push(Span::styled(format!("  {msg}"), style));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}

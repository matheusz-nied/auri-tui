//! Global keys as data. `GLOBAL` is the single source for what a key does
//! outside the focused panel (`lookup`, used by `App::on_key`), the status
//! bar's key hints (`status_hints`) and the help screen (`help_rows`).
//! Panel-specific keys stay in each component (`handle_key` + `hints`).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a global key does; `App::run_command` carries it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Quit,
    FocusNext,
    FocusPrev,
    Refresh,
    ToggleSidebar,
    ShrinkSidebar,
    GrowSidebar,
    /// Explorer view, tree focused.
    ShowExplorer,
    /// Source Control view, commit box focused.
    FocusCommit,
    FocusChanges,
    FocusHistory,
    /// The main pane (diff or file).
    FocusMain,
    GenerateCommitMessage,
    ToggleAiProvider,
    /// Stop a running AI generation — only bound while one runs.
    CancelAi,
    /// Save the edited file — only bound while it has unsaved changes.
    Save,
    Help,
}

/// Where a binding applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Everywhere, even in a modal dialog.
    Anywhere,
    /// Every panel, even while typing a commit message. While editing a
    /// file only the Ctrl ones (Tab and Esc belong to the editor there).
    Global,
    /// Every panel but the commit box and the editor, where the key is text.
    Navigation,
    /// Only while typing in the commit box.
    Typing,
}

/// What `lookup` needs to know about the app's state.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ctx {
    /// A modal overlay captures input.
    pub overlay: bool,
    /// The commit box is focused.
    pub typing: bool,
    /// An AI generation is running.
    pub ai_running: bool,
    /// The file viewer is focused in edit mode.
    pub editing: bool,
    /// The edited file has unsaved changes (focused or not).
    pub modified: bool,
    /// Editing with text selected: Ctrl-C copies.
    pub selection: bool,
}

pub struct Binding {
    pub code: KeyCode,
    /// Ctrl must be held (and must not be, otherwise).
    pub ctrl: bool,
    pub command: Command,
    pub scope: Scope,
    /// The key as shown to the user.
    pub key: &'static str,
    /// What it does, for the help screen.
    pub help: &'static str,
    /// The status bar chunk for it, if it gets one.
    pub status: Option<&'static str>,
}

const fn bind(
    code: KeyCode,
    command: Command,
    scope: Scope,
    key: &'static str,
    help: &'static str,
    status: Option<&'static str>,
) -> Binding {
    Binding {
        code,
        ctrl: false,
        command,
        scope,
        key,
        help,
        status,
    }
}

const fn ctrl(
    c: char,
    command: Command,
    scope: Scope,
    key: &'static str,
    help: &'static str,
    status: Option<&'static str>,
) -> Binding {
    Binding {
        code: KeyCode::Char(c),
        ctrl: true,
        command,
        scope,
        key,
        help,
        status,
    }
}

use Command as C;
use KeyCode::Char;
use Scope::{Anywhere, Global, Navigation, Typing};

/// Every global binding, in help-screen order. `lookup` takes the first
/// match, so state-dependent ones (`CancelAi`) come before the fallback
/// meaning of the same key.
pub const GLOBAL: &[Binding] = &[
    ctrl(
        'c',
        C::Quit,
        Anywhere,
        "ctrl-c",
        "quit (copy, with text selected)",
        None,
    ),
    bind(Char('q'), C::Quit, Navigation, "q", "quit", Some("q quit")),
    ctrl(
        's',
        C::Save,
        Global,
        "ctrl-s",
        "save the edited file",
        Some("ctrl-s save"),
    ),
    bind(
        Char('?'),
        C::Help,
        Navigation,
        "?",
        "this help",
        Some("? help"),
    ),
    bind(
        KeyCode::Tab,
        C::FocusNext,
        Global,
        "tab",
        "next panel",
        Some("tab focus"),
    ),
    bind(
        KeyCode::BackTab,
        C::FocusPrev,
        Global,
        "shift-tab",
        "previous panel",
        None,
    ),
    bind(
        KeyCode::Esc,
        C::CancelAi,
        Global,
        "esc",
        "stop the AI generation",
        None,
    ),
    bind(
        KeyCode::Esc,
        C::FocusNext,
        Typing,
        "esc",
        "leave the commit box",
        Some("esc back"),
    ),
    bind(
        Char('e'),
        C::ShowExplorer,
        Navigation,
        "e",
        "Explorer view",
        Some("e files"),
    ),
    bind(
        Char('c'),
        C::FocusCommit,
        Navigation,
        "c",
        "Source Control view, commit box",
        Some("c git"),
    ),
    bind(
        Char('1'),
        C::FocusChanges,
        Navigation,
        "1",
        "Source Control view, changes",
        None,
    ),
    bind(
        Char('3'),
        C::FocusHistory,
        Navigation,
        "3",
        "Source Control view, history",
        None,
    ),
    bind(
        Char('2'),
        C::FocusMain,
        Navigation,
        "2",
        "main pane (diff or file)",
        None,
    ),
    bind(
        Char('b'),
        C::ToggleSidebar,
        Navigation,
        "b",
        "show/hide the sidebar",
        Some("b sidebar"),
    ),
    bind(
        Char('['),
        C::ShrinkSidebar,
        Navigation,
        "[",
        "narrower sidebar",
        Some("[/] resize"),
    ),
    bind(
        Char(']'),
        C::GrowSidebar,
        Navigation,
        "]",
        "wider sidebar",
        None,
    ),
    bind(Char('r'), C::Refresh, Navigation, "r", "refresh", None),
    ctrl(
        'g',
        C::GenerateCommitMessage,
        Global,
        "ctrl-g",
        "AI commit message",
        Some("ctrl-g AI"),
    ),
    ctrl(
        't',
        C::ToggleAiProvider,
        Global,
        "ctrl-t",
        "switch AI provider (codex/opencode)",
        None,
    ),
];

impl Binding {
    fn matches(&self, key: &KeyEvent) -> bool {
        // Shift is ignored: symbols like `?` arrive shifted on most layouts.
        key.code == self.code
            && key.modifiers.contains(KeyModifiers::CONTROL) == self.ctrl
            && !key.modifiers.contains(KeyModifiers::ALT)
    }

    /// Whether the binding is live in `ctx`.
    fn active(&self, ctx: Ctx) -> bool {
        let in_scope = match self.scope {
            Anywhere => true,
            Global => !ctx.overlay && (self.ctrl || !ctx.editing),
            Navigation => !ctx.overlay && !ctx.typing && !ctx.editing,
            Typing => !ctx.overlay && ctx.typing,
        };
        in_scope
            && match self.command {
                C::CancelAi => ctx.ai_running,
                C::Save => ctx.modified,
                // Ctrl-C over a selection is the editor's copy.
                C::Quit if self.ctrl => ctx.overlay || !(ctx.editing && ctx.selection),
                _ => true,
            }
    }
}

/// The global command `key` triggers in `ctx`, if any — otherwise the key
/// belongs to the overlay or the focused panel.
pub fn lookup(key: &KeyEvent, ctx: Ctx) -> Option<Command> {
    GLOBAL
        .iter()
        .find(|b| b.active(ctx) && b.matches(key))
        .map(|b| b.command)
}

/// The status bar's global key hints for `ctx`.
pub fn status_hints(ctx: Ctx) -> String {
    GLOBAL
        .iter()
        .filter(|b| b.active(ctx))
        .filter_map(|b| b.status)
        .collect::<Vec<_>>()
        .join(" · ")
}

/// One titled block of the help screen: (key, description) rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpSection {
    pub title: String,
    pub rows: Vec<(String, String)>,
}

/// Mouse gestures, for the help screen.
pub const MOUSE: &[(&str, &str)] = &[
    ("click", "focus a panel, select, press a button"),
    ("wheel", "scroll (shift: sideways in the viewers)"),
    (
        "drag",
        "the sidebar border or the changes/history divider to resize",
    ),
];

/// (key, description) for every global binding, for the help screen.
pub fn help_rows() -> Vec<(String, String)> {
    GLOBAL
        .iter()
        .map(|b| (b.key.to_string(), b.help.to_string()))
        .collect()
}

/// A panel's `hints()` string ("enter diff · o edit") as help rows: the
/// first word of each chunk is the key.
pub fn hint_rows(hints: &str) -> Vec<(String, String)> {
    hints
        .split(" · ")
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| match chunk.split_once(' ') {
            Some((key, what)) => (key.to_string(), what.to_string()),
            None => (chunk.to_string(), String::new()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn nav() -> Ctx {
        Ctx::default()
    }

    fn typing() -> Ctx {
        Ctx {
            typing: true,
            ..Ctx::default()
        }
    }

    #[test]
    fn letters_are_text_while_typing() {
        let q = key(Char('q'), KeyModifiers::NONE);
        assert_eq!(lookup(&q, nav()), Some(C::Quit));
        assert_eq!(lookup(&q, typing()), None);
        // Tab and Ctrl keys work everywhere outside dialogs.
        let tab = key(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(lookup(&tab, typing()), Some(C::FocusNext));
        let g = key(Char('g'), KeyModifiers::CONTROL);
        assert_eq!(lookup(&g, typing()), Some(C::GenerateCommitMessage));
        // Plain `g` is not Ctrl-G (it's a panel key: top of list).
        assert_eq!(lookup(&key(Char('g'), KeyModifiers::NONE), nav()), None);
    }

    #[test]
    fn only_ctrl_c_gets_past_a_dialog() {
        let overlay = Ctx {
            overlay: true,
            ..Ctx::default()
        };
        assert_eq!(
            lookup(&key(Char('c'), KeyModifiers::CONTROL), overlay),
            Some(C::Quit)
        );
        assert_eq!(lookup(&key(Char('q'), KeyModifiers::NONE), overlay), None);
        assert_eq!(
            lookup(&key(KeyCode::Tab, KeyModifiers::NONE), overlay),
            None
        );
    }

    #[test]
    fn esc_cancels_ai_first_then_leaves_the_commit_box() {
        let esc = key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(lookup(&esc, typing()), Some(C::FocusNext));
        let busy = Ctx {
            ai_running: true,
            ..typing()
        };
        assert_eq!(lookup(&esc, busy), Some(C::CancelAi));
        // Elsewhere Esc is the panel's own key.
        assert_eq!(lookup(&esc, nav()), None);
    }

    #[test]
    fn the_editor_gets_letters_tab_and_esc() {
        let editing = Ctx {
            editing: true,
            ..Ctx::default()
        };
        for code in [
            Char('q'),
            Char('e'),
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Esc,
        ] {
            assert_eq!(
                lookup(&key(code, KeyModifiers::NONE), editing),
                None,
                "{code:?}"
            );
        }
        let busy = Ctx {
            ai_running: true,
            ..editing
        };
        assert_eq!(lookup(&key(KeyCode::Esc, KeyModifiers::NONE), busy), None);
        let g = key(Char('g'), KeyModifiers::CONTROL);
        assert_eq!(lookup(&g, editing), Some(C::GenerateCommitMessage));
    }

    #[test]
    fn ctrl_s_only_with_unsaved_changes() {
        let s = key(Char('s'), KeyModifiers::CONTROL);
        assert_eq!(lookup(&s, nav()), None);
        let modified = Ctx {
            modified: true,
            ..Ctx::default()
        };
        assert_eq!(lookup(&s, modified), Some(C::Save));
        assert_eq!(
            lookup(
                &s,
                Ctx {
                    editing: true,
                    ..modified
                }
            ),
            Some(C::Save)
        );
        assert!(status_hints(modified).contains("ctrl-s save"));
        assert!(!status_hints(nav()).contains("ctrl-s"));
    }

    #[test]
    fn ctrl_c_copies_a_selection_while_editing() {
        let c = key(Char('c'), KeyModifiers::CONTROL);
        let selecting = Ctx {
            editing: true,
            selection: true,
            ..Ctx::default()
        };
        assert_eq!(lookup(&c, selecting), None);
        let editing = Ctx {
            selection: false,
            ..selecting
        };
        assert_eq!(lookup(&c, editing), Some(C::Quit));
    }

    #[test]
    fn shifted_symbols_match() {
        let question = key(Char('?'), KeyModifiers::SHIFT);
        assert_eq!(lookup(&question, nav()), Some(C::Help));
        assert_eq!(
            lookup(&key(KeyCode::BackTab, KeyModifiers::SHIFT), nav()),
            Some(C::FocusPrev)
        );
    }

    #[test]
    fn status_hints_follow_the_context() {
        assert_eq!(
            status_hints(nav()),
            "q quit · ? help · tab focus · e files · c git · b sidebar · [/] resize · ctrl-g AI"
        );
        assert_eq!(status_hints(typing()), "tab focus · esc back · ctrl-g AI");
    }

    #[test]
    fn every_key_is_bound_once_per_context() {
        for (i, a) in GLOBAL.iter().enumerate() {
            for b in &GLOBAL[i + 1..] {
                let same_key = a.code == b.code && a.ctrl == b.ctrl;
                // Esc's two meanings are told apart by AI state and focus.
                let disjoint =
                    matches!((a.command, b.command), (C::CancelAi, _) | (_, C::CancelAi));
                assert!(!same_key || disjoint, "{} bound twice", a.key);
            }
        }
    }

    #[test]
    fn hint_strings_split_into_rows() {
        assert_eq!(
            hint_rows("enter diff · n/N next/prev change · o"),
            [
                ("enter".to_string(), "diff".to_string()),
                ("n/N".to_string(), "next/prev change".to_string()),
                ("o".to_string(), String::new()),
            ]
        );
    }
}

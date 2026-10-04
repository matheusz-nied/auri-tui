//! The "astro" palette, shared with the landing page (`site/index.html`):
//! warm black, ivory text and gold as the single accent.
//!
//! Every color the UI draws comes from here — components never spell a
//! `Color::` literal. Ratatui has no alpha, so the page's translucent ivory
//! overlays are pre-composed over `BG`.

use ratatui::style::{Color, Modifier, Style};

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

// Surfaces.
pub const BG: Color = rgb(0x0b0a09);
pub const BG_PANEL: Color = rgb(0x0d0c0b);
pub const BG_BAR: Color = rgb(0x121110);
pub const BG_INPUT: Color = rgb(0x0a0908);
pub const PANEL: Color = rgb(0x161412);

// Text.
pub const INK: Color = rgb(0xece7dc);
pub const MUTED: Color = rgb(0x8d887d);
pub const FAINT: Color = rgb(0x57534b);

// Hairlines: ivory at 13% / 24% over `BG`.
pub const LINE: Color = rgb(0x282724);
pub const LINE_2: Color = rgb(0x413f3c);

// Accents.
pub const GOLD: Color = rgb(0xd6a24a);
pub const COOL: Color = rgb(0x9fb3d1);
/// Text drawn on a gold background.
pub const ON_GOLD: Color = rgb(0x1a1406);

// Git / diff: sage for additions, dusty rose for deletions.
pub const ADD_FG: Color = rgb(0xa3b394);
pub const DEL_FG: Color = rgb(0xc98d84);
/// `ADD_FG` / `DEL_FG` at 10% over `BG`.
pub const ADD_BG: Color = rgb(0x1a1b17);
pub const DEL_BG: Color = rgb(0x1e1715);

// Row states: ivory at 4% / 6% / 13% over `BG`.
pub const HOVER_BG: Color = rgb(0x141311);
pub const SEL_BG: Color = rgb(0x191716);
pub const SEL_FOCUS_BG: Color = rgb(0x282724);
/// Selected text in the editor: gold at 20% over `BG`.
pub const TEXT_SEL_BG: Color = rgb(0x342816);

// Syntax (the mockup's tokens).
pub const SYN_KEYWORD: Color = rgb(0xc9a862);
pub const SYN_STRING: Color = ADD_FG;
pub const SYN_COMMENT: Color = rgb(0x5d5951);
pub const SYN_TYPE: Color = rgb(0xb4c0cf);

/// The page: ivory on warm black. Painted over the whole frame so every
/// unstyled cell inherits it.
pub fn base() -> Style {
    Style::default().fg(INK).bg(BG)
}

/// Panel border: gold while the panel owns keyboard focus.
pub fn border(focused: bool) -> Style {
    Style::default().fg(if focused { GOLD } else { LINE_2 })
}

/// Selected row in a list-like component.
pub fn selection(focused: bool) -> Style {
    Style::default().bg(if focused { SEL_FOCUS_BG } else { SEL_BG })
}

pub fn hover() -> Style {
    Style::default().bg(HOVER_BG)
}

pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

pub fn faint() -> Style {
    Style::default().fg(FAINT)
}

pub fn accent() -> Style {
    Style::default().fg(GOLD)
}

/// Uppercase section labels ("STAGED CHANGES").
pub fn section_header() -> Style {
    Style::default().fg(MUTED).add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channels(c: Color) -> (f64, f64, f64) {
        match c {
            Color::Rgb(r, g, b) => (r as f64, g as f64, b as f64),
            other => panic!("not an Rgb color: {other:?}"),
        }
    }

    /// WCAG relative luminance.
    fn luminance(c: Color) -> f64 {
        let lin = |v: f64| {
            let v = v / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = channels(c);
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    fn contrast(a: Color, b: Color) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_tiers_are_readable_on_the_page() {
        assert!(contrast(INK, BG) >= 12.0);
        assert!(contrast(MUTED, BG) >= 5.0);
        // Line numbers and placeholders: deliberately quiet, still legible.
        assert!(contrast(FAINT, BG) >= 2.2);
        assert!(contrast(GOLD, BG) >= 7.0);
    }

    #[test]
    fn diff_tints_are_distinct_from_the_page() {
        assert_ne!(ADD_BG, BG);
        assert_ne!(DEL_BG, BG);
        assert_ne!(ADD_BG, DEL_BG);
        // Syntax colors stay readable on both tints.
        for bg in [ADD_BG, DEL_BG] {
            for fg in [INK, SYN_KEYWORD, SYN_STRING, SYN_TYPE] {
                assert!(contrast(fg, bg) >= 6.0, "{fg:?} on {bg:?}");
            }
        }
    }

    #[test]
    fn selection_is_stronger_with_focus() {
        assert!(luminance(SEL_FOCUS_BG) > luminance(SEL_BG));
        assert!(luminance(SEL_BG) > luminance(HOVER_BG));
        assert!(luminance(HOVER_BG) > luminance(BG));
    }

    #[test]
    fn focused_border_is_the_accent() {
        assert_eq!(border(true).fg, Some(GOLD));
        assert_eq!(border(false).fg, Some(LINE_2));
    }
}

//! File-type icons for the explorer: pure name -> (glyph, color) mapping.
//!
//! Two glyph sets, picked by `[explorer] icons` in preferences:
//! `text` (default) uses 2-column badges any font can draw (`rs`, `{}`,
//! `M↓`…); `nerd` uses Nerd Font glyphs, which need a Nerd Font in the
//! terminal (else they show as boxes). Every icon is exactly 2 columns so
//! names stay aligned.

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

use crate::theme;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconStyle {
    #[default]
    Text,
    Nerd,
}

/// A 2-column icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Icon {
    pub glyph: &'static str,
    pub color: Color,
}

const FOLDER: Color = theme::GOLD;

pub fn dir_icon(style: IconStyle, expanded: bool) -> Icon {
    let glyph = match (style, expanded) {
        (IconStyle::Text, _) => "▰ ",
        (IconStyle::Nerd, false) => "\u{f07b} ",
        (IconStyle::Nerd, true) => "\u{f07c} ",
    };
    Icon {
        glyph,
        color: FOLDER,
    }
}

pub fn file_icon(style: IconStyle, name: &str) -> Icon {
    let kind = FileKind::of(name);
    let (text, nerd, color) = kind.spec();
    Icon {
        glyph: match style {
            IconStyle::Text => text,
            IconStyle::Nerd => nerd,
        },
        color,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    Ruby,
    Java,
    C,
    Cpp,
    Lua,
    Shell,
    Html,
    Css,
    Json,
    Toml,
    Yaml,
    Markdown,
    Text,
    Sql,
    Image,
    Lock,
    Git,
    Docker,
    Make,
    License,
    Other,
}

impl FileKind {
    /// Exact file names first, then the extension (case-insensitive).
    fn of(name: &str) -> Self {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "dockerfile" | "containerfile" => return Self::Docker,
            "makefile" | "justfile" => return Self::Make,
            "license" | "licence" | "copying" => return Self::License,
            ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" => return Self::Git,
            _ => {}
        }
        if lower.starts_with("license.") || lower.starts_with("licence.") {
            return Self::License;
        }
        let Some((_, ext)) = lower.rsplit_once('.') else {
            return Self::Other;
        };
        match ext {
            "rs" => Self::Rust,
            "ts" | "tsx" | "mts" | "cts" => Self::TypeScript,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "py" | "pyi" => Self::Python,
            "go" => Self::Go,
            "rb" => Self::Ruby,
            "java" | "kt" | "kts" => Self::Java,
            "c" | "h" => Self::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" => Self::Cpp,
            "lua" => Self::Lua,
            "sh" | "bash" | "zsh" | "fish" => Self::Shell,
            "html" | "htm" => Self::Html,
            "css" | "scss" | "sass" | "less" => Self::Css,
            "json" | "jsonc" | "json5" => Self::Json,
            "toml" | "ini" | "cfg" | "conf" | "env" => Self::Toml,
            "yaml" | "yml" => Self::Yaml,
            "md" | "mdx" | "markdown" => Self::Markdown,
            "txt" | "log" => Self::Text,
            "sql" | "db" | "sqlite" => Self::Sql,
            "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp" => Self::Image,
            "lock" => Self::Lock,
            _ => Self::Other,
        }
    }

    /// (text badge, Nerd Font glyph, color).
    fn spec(self) -> (&'static str, &'static str, Color) {
        match self {
            Self::Rust => ("rs", "\u{e7a8} ", Color::Rgb(181, 154, 122)),
            Self::TypeScript => ("ts", "\u{e628} ", Color::Rgb(143, 167, 196)),
            Self::JavaScript => ("js", "\u{e74e} ", Color::Rgb(201, 180, 114)),
            Self::Python => ("py", "\u{e73c} ", Color::Rgb(150, 170, 150)),
            Self::Go => ("go", "\u{e627} ", Color::Rgb(140, 180, 190)),
            Self::Ruby => ("rb", "\u{e739} ", Color::Rgb(190, 120, 115)),
            Self::Java => ("jv", "\u{e738} ", Color::Rgb(196, 150, 100)),
            Self::C => ("c ", "\u{e61e} ", Color::Rgb(150, 165, 190)),
            Self::Cpp => ("c+", "\u{e61d} ", Color::Rgb(150, 165, 190)),
            Self::Lua => ("lu", "\u{e620} ", Color::Rgb(140, 150, 190)),
            Self::Shell => ("$ ", "\u{e795} ", Color::Rgb(163, 179, 148)),
            Self::Html => ("<>", "\u{e736} ", Color::Rgb(201, 130, 105)),
            Self::Css => ("# ", "\u{e749} ", Color::Rgb(150, 175, 205)),
            Self::Json => ("{}", "\u{e60b} ", Color::Rgb(190, 175, 120)),
            Self::Toml => ("tm", "\u{e615} ", Color::Rgb(138, 133, 120)),
            Self::Yaml => ("ym", "\u{e615} ", Color::Rgb(190, 120, 115)),
            Self::Markdown => ("M↓", "\u{e73e} ", Color::Rgb(154, 167, 184)),
            Self::Text => ("≡ ", "\u{f0f6} ", Color::Rgb(141, 136, 125)),
            Self::Sql => ("db", "\u{e706} ", Color::Rgb(196, 160, 100)),
            Self::Image => ("▣ ", "\u{f1c5} ", Color::Rgb(170, 150, 190)),
            Self::Lock => ("lk", "\u{f023} ", Color::Rgb(110, 106, 98)),
            Self::Git => ("◆ ", "\u{e702} ", Color::Rgb(201, 125, 100)),
            Self::Docker => ("dk", "\u{e7b0} ", Color::Rgb(130, 165, 200)),
            Self::Make => ("mk", "\u{e615} ", Color::Rgb(138, 133, 120)),
            Self::License => ("© ", "\u{f0f6} ", Color::Rgb(214, 162, 74)),
            Self::Other => ("··", "\u{f016} ", Color::Rgb(110, 106, 98)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_by_name_then_extension() {
        assert_eq!(FileKind::of("main.rs"), FileKind::Rust);
        assert_eq!(FileKind::of("App.TSX"), FileKind::TypeScript);
        assert_eq!(FileKind::of("Cargo.toml"), FileKind::Toml);
        assert_eq!(FileKind::of("Cargo.lock"), FileKind::Lock);
        assert_eq!(FileKind::of("Dockerfile"), FileKind::Docker);
        assert_eq!(FileKind::of("LICENSE"), FileKind::License);
        assert_eq!(FileKind::of("LICENSE-MIT.md"), FileKind::Markdown);
        assert_eq!(FileKind::of(".gitignore"), FileKind::Git);
        assert_eq!(FileKind::of(".env"), FileKind::Toml);
        assert_eq!(FileKind::of("noext"), FileKind::Other);
        assert_eq!(FileKind::of("weird.xyz"), FileKind::Other);
    }

    #[test]
    fn every_icon_is_two_columns() {
        use FileKind::*;
        let all = [
            Rust, TypeScript, JavaScript, Python, Go, Ruby, Java, C, Cpp, Lua, Shell, Html, Css,
            Json, Toml, Yaml, Markdown, Text, Sql, Image, Lock, Git, Docker, Make, License, Other,
        ];
        for kind in all {
            let (text, nerd, _) = kind.spec();
            assert_eq!(text.chars().count(), 2, "{kind:?} text");
            assert_eq!(nerd.chars().count(), 2, "{kind:?} nerd");
        }
        for style in [IconStyle::Text, IconStyle::Nerd] {
            for expanded in [false, true] {
                assert_eq!(dir_icon(style, expanded).glyph.chars().count(), 2);
            }
        }
    }

    #[test]
    fn style_picks_the_glyph_set() {
        assert_eq!(file_icon(IconStyle::Text, "a.rs").glyph, "rs");
        assert_eq!(file_icon(IconStyle::Nerd, "a.rs").glyph, "\u{e7a8} ");
        assert_eq!(
            file_icon(IconStyle::Text, "a.rs").color,
            file_icon(IconStyle::Nerd, "a.rs").color
        );
    }
}

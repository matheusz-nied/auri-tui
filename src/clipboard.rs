//! Copy to the system clipboard from the editor (Ctrl-C/Ctrl-X).
//!
//! `Osc52` asks the terminal to do it with an OSC 52 escape sequence — no
//! platform clipboard crate, and it works over SSH. Terminals that don't
//! support it (or tmux without `set -g set-clipboard on`) ignore it; the
//! editor's own Ctrl-V still pastes what was copied. `App` is the only
//! user (`with_clipboard`); tests keep the default, which does nothing.

use std::io::Write;

use anyhow::Result;

pub trait Clipboard {
    fn copy(&mut self, text: &str) -> Result<()>;
}

/// No clipboard (tests): copying succeeds and goes nowhere.
pub struct NoClipboard;

impl Clipboard for NoClipboard {
    fn copy(&mut self, _text: &str) -> Result<()> {
        Ok(())
    }
}

/// Writes the OSC 52 sequence to stdout, between frames.
pub struct Osc52;

impl Clipboard for Osc52 {
    fn copy(&mut self, text: &str) -> Result<()> {
        let mut out = std::io::stdout();
        out.write_all(osc52(text).as_bytes())?;
        out.flush()?;
        Ok(())
    }
}

/// `ESC ] 52 ; c ; <base64> BEL`: set the clipboard to `text`.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (input, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), want, "{input}");
        }
    }

    #[test]
    fn osc52_wraps_the_payload() {
        assert_eq!(osc52("hi\n"), "\x1b]52;c;aGkK\x07");
    }
}

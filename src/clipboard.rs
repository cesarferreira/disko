//! Putting text on the clipboard.
//!
//! Two ways, because disko is run over ssh about as often as it is run locally.
//! A helper program — `pbcopy`, `wl-copy`, `xclip` — writes to the clipboard of
//! the machine disko is *running* on. The OSC 52 escape sequence hands the text
//! to the terminal that is *showing* disko instead, which over ssh is the only
//! clipboard the person reading the screen can paste from.

use std::io::Write;
use std::process::{Command, Stdio};

/// A clipboard helper, and the arguments that make it write the system
/// clipboard rather than a primary selection.
type Helper = (&'static str, &'static [&'static str]);

/// Copy `text`, by whichever route can reach a clipboard from here.
pub fn copy(text: &str) -> Result<(), String> {
    // Over ssh a helper on this side of the connection would write to a
    // clipboard nobody is sitting in front of.
    if !over_ssh() {
        for (program, args) in helpers() {
            if run(program, args, text).is_ok() {
                return Ok(());
            }
        }
    }

    osc52(text).map_err(|error| format!("no clipboard reachable from here — {error}"))
}

fn helpers() -> Vec<Helper> {
    helpers_for(
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        std::env::var_os("DISPLAY").is_some(),
    )
}

/// Helpers worth trying, best first. A helper for a display server that is not
/// running would take the copy and quietly drop it, so each is offered only
/// when the server it talks to is there to answer.
fn helpers_for(wayland: bool, x11: bool) -> Vec<Helper> {
    if cfg!(target_os = "macos") {
        return vec![("pbcopy", &[])];
    }
    if cfg!(target_os = "windows") {
        return vec![("clip", &[])];
    }

    let mut helpers = Vec::new();
    if wayland {
        helpers.push(("wl-copy", &[][..]));
    }
    if x11 {
        helpers.push(("xclip", &["-selection", "clipboard"][..]));
        helpers.push(("xsel", &["--clipboard", "--input"][..]));
    }
    helpers
}

/// True when the terminal showing disko is on another machine.
fn over_ssh() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

fn run(program: &str, args: &[&str], text: &str) -> Result<(), String> {
    // The helper must not write to the terminal: disko owns the alternate
    // screen, and one stray line would corrupt the display.
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;

    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let written = stdin.write_all(text.as_bytes());
    // Closing the pipe is what tells the helper the text has all arrived.
    drop(stdin);
    written.map_err(|error| error.to_string())
}

/// Ask the terminal itself to take the text, by way of OSC 52.
///
/// The sequence goes straight to stdout, which disko is already drawing to: it
/// moves no cursor and prints nothing, so it cannot disturb the frame on
/// screen, and a terminal that does not understand it ignores it.
fn osc52(text: &str) -> Result<(), std::io::Error> {
    let mut out = std::io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()
}

/// Standard base64, which is the only encoding OSC 52 takes. Short enough to
/// spell out rather than take a dependency for.
fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let bits = chunk.iter().enumerate().fold(0u32, |bits, (index, byte)| {
            bits | (*byte as u32) << (16 - 8 * index)
        });

        // One character per six bits, then one '=' for every byte this chunk
        // was short of three.
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[(bits >> (18 - 6 * index) & 0b11_1111) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_including_its_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_path_survives_the_round_trip() {
        let path = "/Users/someone/Library/Application Support/über";
        let encoded = base64(path.as_bytes());
        assert!(!encoded.contains('\n'), "OSC 52 takes a single line");
        assert_eq!(decode(&encoded), path.as_bytes());
    }

    /// Only the tests need to go the other way, so the decoder lives here.
    fn decode(encoded: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for chunk in encoded.as_bytes().chunks(4) {
            let digits: Vec<u32> = chunk
                .iter()
                .filter(|byte| **byte != b'=')
                .map(|byte| ALPHABET.iter().position(|it| it == byte).unwrap() as u32)
                .collect();
            let bits = digits
                .iter()
                .enumerate()
                .fold(0u32, |bits, (index, digit)| {
                    bits | digit << (18 - 6 * index)
                });
            for index in 0..digits.len() - 1 {
                out.push((bits >> (16 - 8 * index)) as u8);
            }
        }
        out
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn a_headless_box_leaves_the_escape_sequence_as_the_only_way_out() {
        assert!(helpers_for(false, false).is_empty());
        assert_eq!(helpers_for(true, false)[0].0, "wl-copy");
        assert_eq!(helpers_for(false, true)[0].0, "xclip");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn a_mac_always_has_pbcopy() {
        assert_eq!(helpers_for(false, false)[0].0, "pbcopy");
    }
}

//! Keys as a page's own input: the DOM `key` and `code`, the legacy key code, and the text a key
//! types, for `Input.dispatchKeyEvent`.

use crate::desktop::{Chord, Mods};

/// One key, as the page sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageKey {
    pub key: String,
    pub code: String,
    pub key_code: u32,
    /// What the key types, if anything (Return types a carriage return).
    pub text: Option<String>,
}

/// The protocol's modifier bits: Alt 1, Ctrl 2, Meta 4, Shift 8.
pub fn modifier_bits(m: Mods) -> u32 {
    u32::from(m.alt) | u32::from(m.ctrl) << 1 | u32::from(m.cmd) << 2 | u32::from(m.shift) << 3
}

/// The key a chord presses, or `None` for a name the page wouldn't know.
pub fn page_key(chord: &Chord) -> Option<PageKey> {
    let named = |key: &str, code: &str, key_code: u32, text: Option<&str>| PageKey {
        key: key.into(),
        code: code.into(),
        key_code,
        text: text.map(str::to_owned),
    };
    let k = chord.key.as_str();
    Some(match k {
        "return" | "enter" => named("Enter", "Enter", 13, Some("\r")),
        "tab" => named("Tab", "Tab", 9, None),
        "escape" | "esc" => named("Escape", "Escape", 27, None),
        "space" => named(" ", "Space", 32, Some(" ")),
        "backspace" | "delete" => named("Backspace", "Backspace", 8, None),
        "forwarddelete" | "forward-delete" | "del" => named("Delete", "Delete", 46, None),
        "up" | "arrowup" => named("ArrowUp", "ArrowUp", 38, None),
        "down" | "arrowdown" => named("ArrowDown", "ArrowDown", 40, None),
        "left" | "arrowleft" => named("ArrowLeft", "ArrowLeft", 37, None),
        "right" | "arrowright" => named("ArrowRight", "ArrowRight", 39, None),
        "home" => named("Home", "Home", 36, None),
        "end" => named("End", "End", 35, None),
        "pageup" | "page-up" => named("PageUp", "PageUp", 33, None),
        "pagedown" | "page-down" => named("PageDown", "PageDown", 34, None),
        _ if k.len() > 1
            && k.starts_with('f')
            && k[1..].parse::<u32>().is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            let n: u32 = k[1..].parse().ok()?;
            let name = format!("F{n}");
            PageKey {
                key: name.clone(),
                code: name,
                key_code: 111 + n,
                text: None,
            }
        }
        _ => {
            let mut chars = k.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            let shown = if chord.mods.shift {
                c.to_uppercase().collect::<String>()
            } else {
                c.to_string()
            };
            let (code, key_code) = if c.is_ascii_alphabetic() {
                (
                    format!("Key{}", c.to_ascii_uppercase()),
                    u32::from(c.to_ascii_uppercase()),
                )
            } else if c.is_ascii_digit() {
                (format!("Digit{c}"), u32::from(c))
            } else {
                (String::new(), 0)
            };
            // A chord with ⌘ or ⌃ types nothing; it is a command.
            let text = (!chord.mods.cmd && !chord.mods.ctrl).then(|| shown.clone());
            PageKey {
                key: shown,
                code,
                key_code,
                text,
            }
        }
    })
}

/// The editing command a ⌘ chord means on macOS. The page's own key events don't run them, so
/// they are named with the key event (`commands`).
pub fn mac_command(chord: &Chord) -> Option<&'static str> {
    if !chord.mods.cmd || chord.mods.ctrl || chord.mods.alt {
        return None;
    }
    Some(match (chord.key.as_str(), chord.mods.shift) {
        ("a", false) => "selectAll",
        ("c", false) => "copy",
        ("x", false) => "cut",
        ("v", false) => "paste",
        ("z", false) => "undo",
        ("z", true) => "redo",
        ("left", false) | ("arrowleft", false) => "moveToBeginningOfLine",
        ("right", false) | ("arrowright", false) => "moveToEndOfLine",
        ("up", false) | ("arrowup", false) => "moveToBeginningOfDocument",
        ("down", false) | ("arrowdown", false) => "moveToEndOfDocument",
        ("backspace", false) | ("delete", false) => "deleteToBeginningOfLine",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_carry_their_dom_names_and_text() {
        let k = page_key(&Chord::parse("return").unwrap()).unwrap();
        assert_eq!(
            (k.key.as_str(), k.key_code, k.text.as_deref()),
            ("Enter", 13, Some("\r"))
        );
        let a = page_key(&Chord::parse("shift+a").unwrap()).unwrap();
        assert_eq!(
            (a.key.as_str(), a.code.as_str(), a.text.as_deref()),
            ("A", "KeyA", Some("A"))
        );
        let cmd_a = Chord::parse("cmd+a").unwrap();
        assert_eq!(page_key(&cmd_a).unwrap().text, None);
        assert_eq!(mac_command(&cmd_a), Some("selectAll"));
        assert_eq!(
            page_key(&Chord::parse("f5").unwrap()).unwrap().key_code,
            116
        );
        assert!(page_key(&Chord::parse("nosuchkey").unwrap()).is_none());
        assert_eq!(modifier_bits(Chord::parse("cmd+shift+z").unwrap().mods), 12);
    }
}

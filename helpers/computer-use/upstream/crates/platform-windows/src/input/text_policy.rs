//! Portable decisions for insertion. Whole-value replacement is never typing.
pub fn native_edit_class(class: &str) -> bool {
    let class = class.to_ascii_lowercase();
    class == "edit" || class.starts_with("richedit") || class == "scintilla"
}

/// Unicode WM_CHAR consumes UTF-16 units, including surrogate pairs. Use one
/// carriage return per line break; keyboard Return could submit a dialog.
pub fn text_units(text: &str) -> Vec<u16> {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r")
        .encode_utf16()
        .collect()
}

/// UIA rectangles are (left, top, right, bottom), not width/height.
/// Reject a renderer/editor unless it contains the addressed field's centre.
pub fn contains_field_center(field: (i32, i32, i32, i32), child: (i32, i32, i32, i32)) -> bool {
    let (l, t, r, b) = field;
    let (cl, ct, cr, cb) = child;
    if l >= r || t >= b || cl >= cr || ct >= cb {
        return false;
    }
    let x = i64::from(l) + (i64::from(r) - i64::from(l)) / 2;
    let y = i64::from(t) + (i64::from(b) - i64::from(t)) / 2;
    x >= i64::from(cl) && x < i64::from(cr) && y >= i64::from(ct) && y < i64::from(cb)
}

pub const INSERTION_UNAVAILABLE: &str = "This control exposes no background caret-insertion route. type_text did not replace the document or send text. set_value replaces the whole value; use it only when that is intended.";

/// Why per-character insertion stopped before the end of the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertStop {
    /// The user's own (not injected) keyboard or mouse input, pointer motion included.
    UserInput,
    /// The user's own input could no longer be watched, so typing on would ignore it.
    Unmonitored,
    /// The call's time limit, in whole seconds when known.
    Deadline(Option<u64>),
    /// The app did not accept a character within the per-character wait.
    Unacknowledged,
}

/// Insertion stopped part way. Display is the plain reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertStopped {
    pub reason: InsertStop,
    /// Characters the app accepted.
    pub typed: usize,
    pub total: usize,
    /// The start of the text that was not typed.
    pub rest: String,
}

fn is_low_surrogate(unit: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&unit)
}

/// Characters in a run of whole UTF-16 characters.
pub fn char_count(units: &[u16]) -> usize {
    units
        .iter()
        .filter(|unit| !is_low_surrogate(**unit))
        .count()
}

impl InsertStopped {
    pub fn at(reason: InsertStop, units: &[u16], index: usize) -> Self {
        let (done, left) = units.split_at(index.min(units.len()));
        let mut rest: String = char::decode_utf16(left.iter().copied())
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .take(24)
            .collect::<String>()
            .replace('\r', "\n");
        if char_count(left) > 24 {
            rest.push('…');
        }
        Self {
            reason,
            typed: char_count(done),
            total: char_count(units),
            rest,
        }
    }

    pub fn is_user_input(&self) -> bool {
        self.reason == InsertStop::UserInput
    }

    pub fn code(&self) -> &'static str {
        match self.reason {
            InsertStop::UserInput => "user_input",
            InsertStop::Unmonitored => "user_input_unmonitored",
            InsertStop::Deadline(_) => "deadline",
            InsertStop::Unacknowledged => "not_acknowledged",
        }
    }
}

impl std::fmt::Display for InsertStopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let why = match self.reason {
            InsertStop::UserInput => "the user used the mouse or keyboard".to_owned(),
            InsertStop::Unmonitored => {
                "the user's own mouse and keyboard input could not be watched".to_owned()
            }
            InsertStop::Deadline(Some(secs)) => format!("it reached its {secs}-second time limit"),
            InsertStop::Deadline(None) => "it reached its time limit".to_owned(),
            InsertStop::Unacknowledged => {
                "the app did not accept the next character within 1 second".to_owned()
            }
        };
        let left = self.total.saturating_sub(self.typed);
        if self.typed == 0 {
            write!(f, "Typing stopped because {why}. Nothing was typed")?;
        } else {
            write!(
                f,
                "Typing stopped because {why}. Typed {} of {} characters; the remaining {left} (starting {:?}) were not typed",
                self.typed, self.total, self.rest
            )?;
        }
        if self.reason == InsertStop::Unacknowledged {
            write!(
                f,
                ", though the character the app did not acknowledge may have landed"
            )?;
        }
        write!(f, ". Read fresh state before continuing.")
    }
}

impl std::error::Error for InsertStopped {}

/// Send `units` one character at a time. `stop` is asked before every
/// character (never between the halves of a surrogate pair) and ends the run
/// at once; `pause` runs after each whole character; `progress` holds the
/// characters accepted so far for a caller that times the run out.
pub fn insert_units(
    units: &[u16],
    mut stop: impl FnMut() -> Option<InsertStop>,
    mut send: impl FnMut(u16) -> bool,
    mut pause: impl FnMut(),
    progress: Option<&std::sync::atomic::AtomicUsize>,
) -> Result<(), InsertStopped> {
    let mut typed = 0;
    for (index, unit) in units.iter().copied().enumerate() {
        let starts_char = !is_low_surrogate(unit);
        if starts_char {
            if let Some(reason) = stop() {
                return Err(InsertStopped::at(reason, units, index));
            }
        }
        if !send(unit) {
            // Report from the start of the character the app did not accept.
            let start = if starts_char { index } else { index - 1 };
            return Err(InsertStopped::at(InsertStop::Unacknowledged, units, start));
        }
        let ends_char = units
            .get(index + 1)
            .is_none_or(|next| !is_low_surrogate(*next));
        if ends_char {
            typed += 1;
            if let Some(progress) = progress {
                progress.store(typed, std::sync::atomic::Ordering::Release);
            }
            if index + 1 < units.len() {
                pause();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn field_bounds_use_ltrb_and_exclude_other_renderers() {
        assert!(contains_field_center(
            (1000, 500, 1200, 700),
            (1050, 550, 1150, 650)
        ));
        assert!(!contains_field_center(
            (1000, 500, 1200, 700),
            (1500, 500, 1800, 900)
        ));
        assert!(!contains_field_center(
            (1200, 500, 1000, 700),
            (0, 0, 2000, 1000)
        ));
        assert!(contains_field_center(
            (i32::MIN, 0, i32::MAX, 100),
            (i32::MIN, 0, i32::MAX, 100)
        ));
    }
    #[test]
    fn native_editors_are_distinguished_from_frames_and_content_islands() {
        for class in ["Edit", "RICHEDIT50W", "RichEditD2DPT", "Scintilla"] {
            assert!(native_edit_class(class));
        }
        for class in [
            "Notepad",
            "Chrome_WidgetWin_1",
            "Microsoft.UI.Content.DesktopChildSiteBridge",
            "Button",
        ] {
            assert!(!native_edit_class(class));
        }
    }
    #[test]
    fn unicode_and_multiline_insertions_use_characters_without_return_keys() {
        assert_eq!(
            String::from_utf16(&text_units("a😀\r\nb\nc\rd")).unwrap(),
            "a😀\rb\rc\rd"
        );
    }
    #[test]
    fn unavailable_insertion_explains_the_real_limitation() {
        assert!(INSERTION_UNAVAILABLE.contains("did not replace"));
        assert!(INSERTION_UNAVAILABLE.contains("set_value replaces the whole value"));
    }

    /// Runs `insert_units` with the user acting before character `user_at`
    /// (never when None); returns what reached the app and the result.
    fn run(text: &str, user_at: Option<usize>) -> (String, Result<(), InsertStopped>) {
        let units = text_units(text);
        let sent = std::cell::RefCell::new(Vec::new());
        let checks = std::cell::Cell::new(0usize);
        let result = insert_units(
            &units,
            || {
                let asked = checks.get();
                checks.set(asked + 1);
                (Some(asked) == user_at).then_some(InsertStop::UserInput)
            },
            |unit| {
                sent.borrow_mut().push(unit);
                true
            },
            || {},
            None,
        );
        let sent = String::from_utf16(&sent.borrow()).unwrap();
        (sent, result)
    }

    #[test]
    fn the_users_input_stops_typing_before_the_next_character() {
        let (sent, result) = run("hello world", Some(4));
        assert_eq!(sent, "hell");
        let stopped = result.unwrap_err();
        assert!(stopped.is_user_input());
        assert_eq!(stopped.code(), "user_input");
        assert_eq!((stopped.typed, stopped.total), (4, 11));
        assert_eq!(stopped.rest, "o world");
        let reply = stopped.to_string();
        assert!(
            reply.contains("because the user used the mouse or keyboard"),
            "{reply}"
        );
        assert!(reply.contains("Typed 4 of 11 characters"), "{reply}");
        assert!(
            reply.contains("remaining 7 (starting \"o world\") were not typed"),
            "{reply}"
        );
    }

    #[test]
    fn without_user_input_every_character_is_sent() {
        let (sent, result) = run("a😀\nb", None);
        assert_eq!(sent, "a😀\rb");
        assert!(result.is_ok());
    }

    #[test]
    fn input_before_the_first_character_types_nothing() {
        let (sent, result) = run("abc", Some(0));
        assert_eq!(sent, "");
        let reply = result.unwrap_err().to_string();
        assert!(reply.contains("Nothing was typed"), "{reply}");
        assert!(!reply.contains("Typed "), "{reply}");
    }

    #[test]
    fn a_stop_never_splits_a_surrogate_pair() {
        // Checks happen once per character: before 'a', before 😀, before 'b'.
        let (sent, result) = run("a😀b", Some(2));
        assert_eq!(sent, "a😀");
        let stopped = result.unwrap_err();
        assert_eq!((stopped.typed, stopped.total), (2, 3));
        assert_eq!(stopped.rest, "b");
    }

    #[test]
    fn progress_and_pauses_follow_whole_characters() {
        let units = text_units("x😀y");
        let progress = std::sync::atomic::AtomicUsize::new(0);
        let pauses = std::cell::Cell::new(0);
        insert_units(
            &units,
            || None,
            |_| true,
            || pauses.set(pauses.get() + 1),
            Some(&progress),
        )
        .unwrap();
        assert_eq!(progress.load(std::sync::atomic::Ordering::Acquire), 3);
        // Between characters only, not after the last one.
        assert_eq!(pauses.get(), 2);
    }

    #[test]
    fn an_unacknowledged_character_reports_what_the_app_accepted() {
        let units = text_units("abcdef");
        let mut sent = 0;
        let stopped = insert_units(
            &units,
            || None,
            |_| {
                sent += 1;
                sent <= 3
            },
            || {},
            None,
        )
        .unwrap_err();
        assert_eq!(stopped.reason, InsertStop::Unacknowledged);
        assert_eq!(stopped.typed, 3);
        assert!(stopped.to_string().contains("may have landed"));
    }

    #[test]
    fn deadline_and_unwatched_input_are_said_plainly() {
        let units = text_units(&"x".repeat(40));
        let deadline = InsertStopped::at(InsertStop::Deadline(Some(85)), &units, 30);
        let reply = deadline.to_string();
        assert!(reply.contains("85-second time limit"), "{reply}");
        assert!(reply.contains("Typed 30 of 40"), "{reply}");
        assert!(!reply.contains("accessibility provider"), "{reply}");
        let long = InsertStopped::at(InsertStop::Deadline(None), &units, 0);
        assert!(long.rest.ends_with('…'));
        assert_eq!(long.rest.chars().count(), 25);
        let unwatched = InsertStopped::at(InsertStop::Unmonitored, &units, 0).to_string();
        assert!(unwatched.contains("could not be watched"), "{unwatched}");
        assert!(unwatched.contains("Nothing was typed"), "{unwatched}");
    }

    #[test]
    fn replies_avoid_words_the_layer_removes() {
        // Brigadier's layer drops sentences that mention acting in front.
        for reason in [
            InsertStop::UserInput,
            InsertStop::Unmonitored,
            InsertStop::Deadline(Some(30)),
            InsertStop::Unacknowledged,
        ] {
            let reply = InsertStopped::at(reason, &text_units("abc"), 1).to_string();
            assert!(
                !reply.to_ascii_lowercase().contains("foreground"),
                "{reply}"
            );
        }
    }
}

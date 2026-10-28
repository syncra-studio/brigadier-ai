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

pub const INSERTION_UNAVAILABLE: &str = "This control exposes no background caret-insertion route. type_text did not replace the document or send text. set_value replaces the whole value; use it only when that is intended.";

#[cfg(test)]
mod tests {
    use super::*;
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
}

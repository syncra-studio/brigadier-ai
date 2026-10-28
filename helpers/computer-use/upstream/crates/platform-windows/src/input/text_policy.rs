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
}

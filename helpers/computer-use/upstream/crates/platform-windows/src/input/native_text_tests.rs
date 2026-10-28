use crate::input::keyboard::{background_text_target, post_insert_text};
use windows::core::w;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Controls::{EM_GETMODIFY, EM_SETSEL};
use windows::Win32::UI::WindowsAndMessaging::WM_CHAR;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetWindowTextW, PeekMessageW, SendMessageW,
    MSG, PM_REMOVE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_SETTEXT, WS_CHILD, WS_POPUP,
};

#[test]
fn character_insertion_preserves_surrounding_text_and_marks_edit_modified() {
    unsafe {
        let parent = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!(""),
            WS_POPUP,
            0,
            0,
            200,
            100,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let edit = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            w!(""),
            WS_CHILD | WINDOW_STYLE(4),
            0,
            0,
            180,
            80,
            parent,
            None,
            None,
            None,
        )
        .unwrap();
        SendMessageW(
            edit,
            WM_SETTEXT,
            WPARAM(0),
            LPARAM(w!("abcdef").as_ptr() as isize),
        );
        SendMessageW(edit, EM_SETSEL, WPARAM(2), LPARAM(4));
        assert_eq!(
            background_text_target(
                parent.0 as usize as u64,
                Some(edit.0 as usize as u64),
                None,
                true
            )
            .unwrap(),
            edit.0 as usize as u64
        );
        let cancelled = std::sync::atomic::AtomicBool::new(true);
        let error = crate::input::keyboard::post_insert_text_cancelable(
            edit.0 as usize as u64,
            "MUST NOT INSERT",
            0,
            &cancelled,
        )
        .unwrap_err();
        assert!(error.to_string().contains("deadline"));
        let mut unchanged = [0u16; 64];
        let n = GetWindowTextW(edit, &mut unchanged);
        assert_eq!(
            String::from_utf16(&unchanged[..n as usize]).unwrap(),
            "abcdef"
        );
        post_insert_text(edit.0 as usize as u64, "X😀", 0).unwrap();
        let mut message = MSG::default();
        while PeekMessageW(&mut message, edit, WM_CHAR, WM_CHAR, PM_REMOVE).as_bool() {
            DispatchMessageW(&message);
        }
        let mut text = [0u16; 64];
        let n = GetWindowTextW(edit, &mut text);
        assert_eq!(String::from_utf16(&text[..n as usize]).unwrap(), "abX😀ef");
        assert_ne!(SendMessageW(edit, EM_GETMODIFY, WPARAM(0), LPARAM(0)).0, 0);
        DestroyWindow(parent).unwrap();
    }
}

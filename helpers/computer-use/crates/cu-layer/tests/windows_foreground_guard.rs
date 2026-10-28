//! Exercise the actual Windows observation decisions on every host.
#[path = "../../../upstream/crates/platform-windows/src/input/foreground_policy.rs"]
mod foreground_policy;

#[cfg(target_os = "windows")]
mod native {
    use platform_windows::input::NoActivateGuard;
    use windows::core::w;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowLongPtrW, GWL_EXSTYLE, WINDOW_EX_STYLE,
        WS_EX_NOACTIVATE, WS_POPUP,
    };

    // Hidden windows only: these tests never show, activate or inject input.
    struct Window(HWND);
    impl Window {
        fn hidden(owner: Option<HWND>, style: WINDOW_EX_STYLE) -> Self {
            Self(unsafe {
                CreateWindowExW(
                    style,
                    w!("STATIC"),
                    w!("foreground guard test"),
                    WS_POPUP,
                    0,
                    0,
                    1,
                    1,
                    owner.unwrap_or_default(),
                    None,
                    None,
                    None,
                )
                .unwrap()
            })
        }
        fn noactivate(&self) -> bool {
            (unsafe { GetWindowLongPtrW(self.0, GWL_EXSTYLE) } & WS_EX_NOACTIVATE.0 as isize) != 0
        }
    }
    impl Drop for Window {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }

    #[test]
    fn dialog_owner_stays_guarded_until_the_last_overlapping_action_ends() {
        let owner = Window::hidden(None, WINDOW_EX_STYLE(0));
        let dialog = Window::hidden(Some(owner.0), WINDOW_EX_STYLE(0));
        let first = NoActivateGuard::arm(dialog.0);
        let second = NoActivateGuard::arm(owner.0);
        assert!(dialog.noactivate());
        assert!(owner.noactivate());
        drop(first);
        assert!(!dialog.noactivate());
        assert!(
            owner.noactivate(),
            "dialog cleanup must not release another action's lease"
        );
        drop(second);
        assert!(!owner.noactivate());
    }

    #[test]
    fn existing_noactivate_style_belongs_to_the_app() {
        let window = Window::hidden(None, WS_EX_NOACTIVATE);
        let guard = NoActivateGuard::arm(window.0);
        drop(guard);
        assert!(window.noactivate());
    }
}

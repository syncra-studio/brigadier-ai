//! Windows hosted-runner desktop cleanup performed before behavioral capture.

#[cfg(any(target_os = "windows", test))]
fn is_hosted_runner_console(title: &str, class_name: &str) -> bool {
    let title = title.to_ascii_lowercase();
    let console_class = matches!(
        class_name,
        "ConsoleWindowClass" | "CASCADIA_HOSTING_WINDOW_CLASS"
    );
    console_class
        && [
            "hostedcomputeagent",
            "hosted-compute-agent",
            "runner.worker",
            "github actions runner",
        ]
        .iter()
        .any(|marker| title.contains(marker))
}

#[cfg(not(target_os = "windows"))]
pub fn minimize_hosted_runner_console() -> Result<&'static str, String> {
    Ok("not_applicable")
}

#[cfg(test)]
mod tests {
    use super::is_hosted_runner_console;

    #[test]
    fn matches_only_named_runner_console_windows() {
        assert!(is_hosted_runner_console(
            "Administrator: HostedComputeAgent.exe",
            "ConsoleWindowClass"
        ));
        assert!(is_hosted_runner_console(
            "GitHub Actions Runner",
            "CASCADIA_HOSTING_WINDOW_CLASS"
        ));
        assert!(is_hosted_runner_console(
            r"C:\ProgramData\GitHub\HostedComputeAgent\hosted-compute-agent",
            "ConsoleWindowClass"
        ));
        assert!(!is_hosted_runner_console(
            "CuTestHarness Sentinel",
            "Chrome_WidgetWin_1"
        ));
        assert!(!is_hosted_runner_console(
            "HostedComputeAgent status",
            "Chrome_WidgetWin_1"
        ));
    }
}

#[cfg(target_os = "windows")]
pub fn minimize_hosted_runner_console() -> Result<&'static str, String> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
        let mut title = [0u16; 512];
        let mut class = [0u16; 128];
        let title_len = unsafe { GetWindowTextW(hwnd, &mut title) } as usize;
        let class_len = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
        if is_hosted_runner_console(
            &String::from_utf16_lossy(&title[..title_len]),
            &String::from_utf16_lossy(&class[..class_len]),
        ) {
            unsafe {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
                *(data.0 as *mut bool) = true;
            }
        }
        BOOL(1)
    }
    let mut changed = false;
    unsafe { EnumWindows(Some(visit), LPARAM(&mut changed as *mut bool as isize)) }
        .map_err(|e| e.to_string())?;
    Ok(if changed {
        "minimized_named_runner_console"
    } else {
        "no_named_runner_console"
    })
}

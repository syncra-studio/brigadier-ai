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

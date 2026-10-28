//! Terminate one proven app process, never a shared application frame host.
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
    WaitForSingleObject, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

struct ProcessHandle(HANDLE);
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// A true return means Windows accepted termination and confirmed exit; false
/// means it confirmed the process exited independently. A timeout is an error.
pub(super) fn terminate_verified(pid: u32, expected_start: Option<u64>) -> Result<bool, String> {
    // Inspect privileges before asking for terminate rights, so elevated
    // targets get the same plain refusal as reads/actions, even when opening
    // with PROCESS_TERMINATE would fail first.
    let query = ProcessHandle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|error| format!("Windows could not inspect app process {pid}: {error}. No termination was attempted."))?);
    if let Some(message) = crate::input::process_control_integrity_refusal(query.0)? {
        return Err(message);
    }
    let process = ProcessHandle(unsafe {
        OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
    }.map_err(|error| format!(
        "Windows could not open app process {pid} for verified termination: {error}. No termination was attempted."
    ))?);

    // Check the termination handle as well: PID reuse must not substitute a
    // higher-integrity process between the query and opening this handle.
    if let Some(message) = crate::input::process_control_integrity_refusal(process.0)? {
        return Err(message);
    }

    let mut path = vec![0u16; 32768];
    let mut path_len = path.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut path_len,
        )
    }
    .map_err(|error| {
        format!("Windows could not identify process {pid}: {error}. No termination was attempted.")
    })?;
    if path_len == 0 || path_len as usize > path.len() {
        return Err(
            "Windows returned no usable executable identity. No termination was attempted.".into(),
        );
    }
    let executable = String::from_utf16_lossy(&path[..path_len as usize]);
    if super::launch_policy::is_shared_frame_host(&executable) {
        return Err("ApplicationFrameHost is shared by Store apps and will never be terminated. Use the Store app's own pid from launch_app or list_windows.".into());
    }

    if let Some(expected) = expected_start {
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe { GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user) }
            .map_err(|error| format!("Windows could not verify the lifetime of process {pid}: {error}. No termination was attempted."))?;
        let actual = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        if actual != expected {
            return Err("The app process identity changed before termination. No termination was attempted; refresh list_apps.".into());
        }
    }

    if unsafe { WaitForSingleObject(process.0, 0) } == WAIT_OBJECT_0 {
        return Ok(false);
    }
    // Hosted Store windows belong to ApplicationFrameHost at the Win32 level;
    // use their actual app pid, including the owner of a foreground popup.
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetForegroundWindow, GA_ROOTOWNER};
    let foreground = unsafe { GetForegroundWindow() };
    let owner = unsafe { GetAncestor(foreground, GA_ROOTOWNER) };
    if let Some(reason) = super::launch_policy::termination_front_refusal(
        pid,
        crate::win32::window_app_pid(foreground.0 as u64),
        crate::win32::window_app_pid(owner.0 as u64),
    ) {
        return Err(reason.into());
    }
    let request = unsafe { TerminateProcess(process.0, 1) };
    let wait = unsafe { WaitForSingleObject(process.0, 2000) };
    use super::launch_policy::{termination_observation, TerminationObservation};
    match termination_observation(request.is_ok(), wait == WAIT_OBJECT_0) {
        TerminationObservation::Terminated => Ok(true),
        TerminationObservation::AlreadyExited => Ok(false),
        TerminationObservation::ExitUnconfirmed => Err(format!(
            "Termination was requested for app process {pid}, but Windows has not confirmed its exit (wait status {:#x}). It may still be running; verify before retrying.", wait.0
        )),
        TerminationObservation::Rejected => Err(format!(
            "Windows refused to terminate app process {pid}: {}. Its exit could not be confirmed.",
            request.unwrap_err()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};

    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    #[ignore = "fixture subprocess used only by the verified-termination tests"]
    fn owned_process_fixture() {
        if std::env::var_os("CUA_DRIVER_TERMINATION_TEST_CHILD").is_some() {
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
    }

    fn fixture() -> ChildGuard {
        ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tools::launch_termination::tests::owned_process_fixture",
                    "--ignored",
                ])
                .env("CUA_DRIVER_TERMINATION_TEST_CHILD", "1")
                .creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0)
                .spawn()
                .unwrap(),
        )
    }

    #[test]
    fn shared_host_executable_is_refused_without_terminating_it() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("ApplicationFrameHost.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
        let mut child = ChildGuard(
            Command::new(executable)
                .args([
                    "--exact",
                    "tools::launch_termination::tests::owned_process_fixture",
                    "--ignored",
                ])
                .env("CUA_DRIVER_TERMINATION_TEST_CHILD", "1")
                .creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0)
                .spawn()
                .unwrap(),
        );
        let error = terminate_verified(child.0.id(), None).unwrap_err();
        assert!(error.contains("ApplicationFrameHost is shared"));
        assert!(child.0.try_wait().unwrap().is_none());
    }

    #[test]
    fn verifies_exit_of_the_exact_child_process() {
        let mut child = fixture();
        assert!(terminate_verified(child.0.id(), None).unwrap());
        assert!(child.0.try_wait().unwrap().is_some());
    }

    #[test]
    fn changed_process_lifetime_is_refused_without_killing_the_child() {
        let mut child = fixture();
        let error = terminate_verified(child.0.id(), Some(0)).unwrap_err();
        assert!(error.contains("identity changed"));
        assert!(child.0.try_wait().unwrap().is_none());
    }
}

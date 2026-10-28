#![cfg(target_os = "windows")]
//! Hidden helper processes only; no desktop windows or input are created.
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::JobObjects::{
    JobObjectBasicProcessIdList, QueryInformationJobObject, JOBOBJECT_BASIC_PROCESS_ID_LIST,
};
use windows::Win32::System::Threading::{
    OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

const ROLE: &str = "BRIGADIER_CU_JOB_TEST_ROLE";
const DIRECTORY: &str = "BRIGADIER_CU_JOB_TEST_DIR";
const OWNER: &str = "BRIGADIER_CU_JOB_TEST_OWNER_PID";
const CASE: &str = "job_membership_precedes_child_code_and_crash_reaps_descendants";

fn helper(role: &str, directory: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", CASE, "--nocapture", "--test-threads=1"])
        .env(ROLE, role)
        .env(DIRECTORY, directory)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "helper handshake timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}
struct Root(Child);
impl Drop for Root {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            // Even a failing regression must clean its still-running helper tree.
            let _ = Command::new("taskkill.exe")
                .args(["/F", "/T", "/PID", &self.0.id().to_string()])
                .creation_flags(0x08000000)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
struct Process(HANDLE);
impl Process {
    fn open(pid: u32) -> Self {
        Self(unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid) }.unwrap())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            if WaitForSingleObject(self.0, 0) == WAIT_TIMEOUT {
                let _ = TerminateProcess(self.0, 1);
            }
            let _ = CloseHandle(self.0);
        }
    }
}
fn child_is_already_in_its_own_job() -> Result<(), String> {
    // A null handle queries the caller's immediate job. The helper's owner
    // remains outside this new job; an inherited outer CI/agent job contains it.
    let mut buffer = [0usize; 256];
    unsafe {
        QueryInformationJobObject(
            None,
            JobObjectBasicProcessIdList,
            buffer.as_mut_ptr().cast(),
            std::mem::size_of_val(&buffer) as u32,
            None,
        )
    }
    .map_err(|e| e.to_string())?;
    let list = unsafe { &*(buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>()) };
    let count = list.NumberOfProcessIdsInList as usize;
    let capacity = (std::mem::size_of_val(&buffer)
        - std::mem::offset_of!(JOBOBJECT_BASIC_PROCESS_ID_LIST, ProcessIdList))
        / std::mem::size_of::<usize>();
    if count > capacity {
        return Err("job process list exceeded buffer".into());
    }
    let ids = unsafe { std::slice::from_raw_parts(list.ProcessIdList.as_ptr(), count) };
    let owner: usize = std::env::var(OWNER).unwrap().parse().unwrap();
    if !ids.contains(&(std::process::id() as usize)) || ids.contains(&owner) {
        return Err("child user code ran before assignment to its dedicated job".into());
    }
    Ok(())
}

#[test]
fn job_membership_precedes_child_code_and_crash_reaps_descendants() {
    if let Ok(role) = std::env::var(ROLE) {
        let directory = PathBuf::from(std::env::var_os(DIRECTORY).unwrap());
        match role.as_str() {
            "owner" => {
                let mut command = helper("child", &directory);
                command.env(OWNER, std::process::id().to_string());
                let child = cu_testkit::spawn_in_job(&mut command).unwrap();
                std::mem::forget(child); // Deliberately bypass ChildReaper cleanup.
                wait_until(|| directory.join("release-owner").exists());
                std::process::exit(23); // No Rust destructors: job handle closes at process exit.
            }
            "child" => {
                if let Err(error) = child_is_already_in_its_own_job() {
                    std::fs::write(directory.join("ready"), format!("ERROR: {error}")).unwrap();
                    loop {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
                let grandchild = helper("grandchild", &directory).spawn().unwrap();
                std::fs::write(
                    directory.join("ready"),
                    format!("{} {}", std::process::id(), grandchild.id()),
                )
                .unwrap();
                std::mem::forget(grandchild);
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            "grandchild" => loop {
                std::thread::sleep(Duration::from_secs(1));
            },
            _ => panic!("unknown helper role"),
        }
    }
    let scratch = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let directory = tempfile::Builder::new()
        .prefix("cu-job-cleanup-")
        .tempdir_in(scratch)
        .unwrap();
    let mut root = Root(helper("owner", directory.path()).spawn().unwrap());
    wait_until(|| directory.path().join("ready").exists());
    let ready = std::fs::read_to_string(directory.path().join("ready")).unwrap();
    assert!(!ready.starts_with("ERROR:"), "{ready}");
    let ids: Vec<u32> = ready
        .split_whitespace()
        .map(|s| s.parse().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    let child = Process::open(ids[0]);
    let grandchild = Process::open(ids[1]);
    assert_eq!(unsafe { WaitForSingleObject(child.0, 0) }, WAIT_TIMEOUT);
    assert_eq!(
        unsafe { WaitForSingleObject(grandchild.0, 0) },
        WAIT_TIMEOUT
    );
    std::fs::write(directory.path().join("release-owner"), "exit now").unwrap();
    wait_until(|| root.0.try_wait().unwrap().is_some());
    assert_eq!(root.0.wait().unwrap().code(), Some(23));
    assert_eq!(
        unsafe { WaitForSingleObject(child.0, 3000) },
        WAIT_OBJECT_0,
        "child escaped crash cleanup"
    );
    assert_eq!(
        unsafe { WaitForSingleObject(grandchild.0, 3000) },
        WAIT_OBJECT_0,
        "grandchild escaped crash cleanup"
    );
}

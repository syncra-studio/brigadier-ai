//! Verify the visible web document, rather than returning a browser startup window.
use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationLegacyIAccessiblePattern,
    TreeScope_Subtree, UIA_ControlTypePropertyId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_LegacyIAccessiblePatternId, UIA_ValueValuePropertyId,
};

pub(super) struct Apartment(bool);
impl Apartment {
    pub(super) fn new() -> Self {
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

pub(super) unsafe fn document_url(document: &IUIAutomationElement) -> Option<String> {
    let value = document
        .GetCurrentPropertyValue(UIA_ValueValuePropertyId)
        .ok();
    let value = value
        .and_then(|value| windows::core::BSTR::try_from(&value).ok())
        .map(|s| s.to_string());
    if value
        .as_deref()
        .is_some_and(super::browser_target::is_document_url)
    {
        return value;
    }
    let pattern: IUIAutomationLegacyIAccessiblePattern = document
        .GetCurrentPattern(UIA_LegacyIAccessiblePatternId)
        .ok()?
        .cast()
        .ok()?;
    let value = pattern.CurrentValue().ok()?.to_string();
    super::browser_target::is_document_url(&value).then_some(value)
}

/// Only browser chrome can supply fallback URL evidence. A web page's own
/// Edit controls must never impersonate an omnibox.
unsafe fn inside_document(
    automation: &IUIAutomation,
    element: &IUIAutomationElement,
    root: &IUIAutomationElement,
) -> bool {
    let Ok(walker) = automation.ControlViewWalker() else {
        return true;
    };
    let mut current = element.clone();
    for _ in 0..128 {
        let Ok(parent) = walker.GetParentElement(&current) else {
            return true;
        };
        if automation
            .CompareElements(&parent, root)
            .map(|same| same.as_bool())
            .unwrap_or(false)
        {
            return false;
        }
        if parent.CurrentControlType().ok() == Some(UIA_DocumentControlTypeId) {
            return true;
        }
        current = parent;
    }
    true
}

pub(super) unsafe fn address_bar_url(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
) -> Option<String> {
    let condition = automation
        .CreatePropertyCondition(UIA_ControlTypePropertyId, &UIA_EditControlTypeId.0.into())
        .ok()?;
    let edits = root.FindAll(TreeScope_Subtree, &condition).ok()?;
    let mut result = None;
    for index in 0..edits.Length().ok()? {
        let edit = edits.GetElement(index).ok()?;
        if edit
            .CurrentIsOffscreen()
            .map(|off| off.as_bool())
            .unwrap_or(true)
            || inside_document(automation, &edit, root)
        {
            continue;
        }
        let id = edit
            .CurrentAutomationId()
            .ok()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let class = edit
            .CurrentClassName()
            .ok()
            .map(|s| s.to_string())
            .unwrap_or_default();
        if !super::browser_target::is_omnibox(&id, &class) {
            continue;
        }
        // Edge often exposes the address on its Edit, not its Document. The
        // value may omit https://, so document_url's scheme filter cannot apply.
        let value = edit
            .GetCurrentPropertyValue(UIA_ValueValuePropertyId)
            .ok()
            .and_then(|v| windows::core::BSTR::try_from(&v).ok())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                let pattern: IUIAutomationLegacyIAccessiblePattern = edit
                    .GetCurrentPattern(UIA_LegacyIAccessiblePatternId)
                    .ok()?
                    .cast()
                    .ok()?;
                pattern.CurrentValue().ok().map(|s| s.to_string())
            })?;
        if result.replace(value).is_some() {
            return None;
        }
    }
    result
}

pub(super) unsafe fn matching_document(
    hwnd: u64,
    requested: &str,
    exact: bool,
) -> anyhow::Result<(IUIAutomation, IUIAutomationElement, String)> {
    let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
    let root = automation.ElementFromHandle(HWND(hwnd as *mut _))?;
    let condition = automation.CreatePropertyCondition(
        UIA_ControlTypePropertyId,
        &UIA_DocumentControlTypeId.0.into(),
    )?;
    let documents = root.FindAll(TreeScope_Subtree, &condition)?;
    let mut visible = Vec::new();
    let mut matching = None;
    for index in 0..documents.Length()? {
        let document = documents.GetElement(index)?;
        if document
            .CurrentIsOffscreen()
            .map(|off| off.as_bool())
            .unwrap_or(true)
            || inside_document(&automation, &document, &root)
        {
            continue;
        }
        let url = document_url(&document);
        if url
            .as_deref()
            .is_some_and(|url| super::browser_target::url_matches(url, requested, exact))
        {
            if matching.is_some() {
                anyhow::bail!("More than one visible document matches {requested:?}; use a more specific URL. No other page was read.");
            }
            matching = Some((document.clone(), url.clone().unwrap_or_default()));
        }
        visible.push((document, url));
    }
    if let Some((doc, url)) = matching {
        if address_bar_url(&automation, &root)
            .as_deref()
            .is_some_and(|bar| !super::browser_target::url_matches(bar, requested, exact))
        {
            anyhow::bail!("The document and address bar disagree in window {hwnd}; navigation may still be loading. No page was read.");
        }
        return Ok((automation, doc, url));
    }
    let browser = crate::win32::window_owner_pid(hwnd).is_some_and(|pid| {
        crate::win32::list_processes().iter().any(|p| {
            p.pid == pid && super::browser_target::named_browser_executable(&p.name).is_some()
        })
    });
    if browser {
        if let Some(bar) = address_bar_url(&automation, &root) {
            if let Some((document, url)) = visible.first() {
                if super::browser_target::omnibox_binds_document(
                    visible.len(),
                    url.as_deref(),
                    &bar,
                    requested,
                    exact,
                ) {
                    return Ok((automation, document.clone(), bar));
                }
            }
        }
    }
    anyhow::bail!("Windows UI Automation does not expose a uniquely visible document matching {requested:?} in window {hwnd}. Its document URL and address bar could not confirm the target. No other page was read. Select that tab in the named window and retry.")
}

pub(super) async fn window_showing_url(
    executable: &str,
    url: &str,
    timeout: std::time::Duration,
    preexisting_windows: std::collections::HashSet<u64>,
    owner_pid: Option<u32>,
) -> anyhow::Result<Option<crate::win32::WindowInfo>> {
    let executable = super::launch_policy::executable_name(executable);
    let url = url.to_owned();
    tokio::task::spawn_blocking(move || {
        crate::uia::windows_enum::run_uia_with_deadline_cancelable(
            "browser URL verification",
            timeout,
            "no unverified browser window",
            move |cancelled| {
                let _com = Apartment::new();
                let browser_pids: std::collections::HashSet<_> = crate::win32::list_processes()
                    .into_iter()
                    .filter(|process| {
                        owner_pid.is_none_or(|pid| process.pid == pid)
                            && super::launch_policy::executable_name(&process.name) == executable
                    })
                    .map(|process| process.pid)
                    .collect();
                // Browser renderer/helper processes have no top-level window.
                // Scan native windows once instead of paying a global UIA
                // timeout for every renderer pid.
                for window in crate::win32::list_windows_via_win32(None) {
                    if !browser_pids.contains(&window.pid)
                        || preexisting_windows.contains(&window.hwnd)
                    {
                        continue;
                    }
                    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                        return None;
                    }
                    if unsafe { matching_document(window.hwnd, &url, true) }.is_ok() {
                        return Some(window);
                    }
                }
                None
            },
        )
        .map_err(|error| {
            anyhow::anyhow!(
                "Browser URL verification stopped because {}",
                crate::uia::click::deadline_reason(error)
            )
        })
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_window_is_not_a_verified_browser_page() {
        let _com = Apartment::new();
        assert!(unsafe { matching_document(0, "example.com", false) }.is_err());
    }
}

/// A blank on-disk document bypasses Notepad's restore-only startup. It is the
/// agent's actual document, reported to the caller; never overwrite an old file.
pub(super) fn create_notepad_document() -> anyhow::Result<std::path::PathBuf> {
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|error| anyhow::anyhow!("Could not create a fresh document name: {error}"))?;
    let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let path = std::env::temp_dir().join(format!("brigadier-cu-{nonce}.txt"));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    Ok(path)
}

/// Notepad windows that this helper's own launches opened. Only these may
/// receive another launch (see `launch_policy::foreign_notepad_windows`).
static OWN_NOTEPAD_WINDOWS: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

/// Visible top-level Notepad windows (minimized included) with their titles.
pub(super) fn open_notepad_windows() -> Vec<(u64, String)> {
    let pids: std::collections::HashSet<_> = crate::win32::list_processes()
        .into_iter()
        .filter(|process| super::launch_policy::is_notepad_target(&process.name))
        .map(|process| process.pid)
        .collect();
    if pids.is_empty() {
        return Vec::new();
    }
    crate::win32::list_windows_via_win32(None)
        .into_iter()
        .filter(|window| pids.contains(&window.pid))
        .map(|window| (window.hwnd, window.title))
        .collect()
}

/// Open Notepad windows this helper did not open, with their titles.
pub(super) fn foreign_notepad_windows() -> Vec<(u64, String)> {
    let open = open_notepad_windows();
    let ids: Vec<u64> = open.iter().map(|(window, _)| *window).collect();
    let mut own = OWN_NOTEPAD_WINDOWS.lock().unwrap();
    own.retain(|window| ids.contains(window));
    let foreign = super::launch_policy::foreign_notepad_windows(&ids, &own);
    open.into_iter()
        .filter(|(window, _)| foreign.contains(window))
        .collect()
}

/// Held by a Notepad launch that found no foreign window: when its reply is
/// built, every Notepad window then open came from this helper's launches.
pub(super) struct OwnNotepadWindows;
impl Drop for OwnNotepadWindows {
    fn drop(&mut self) {
        record_own_notepad_windows();
    }
}

fn record_own_notepad_windows() {
    let open = open_notepad_windows();
    let mut own = OWN_NOTEPAD_WINDOWS.lock().unwrap();
    for (window, _) in open {
        if !own.contains(&window) && own.len() < 64 {
            own.push(window);
        }
    }
}

pub(super) async fn notepad_document_window(
    filename: String,
) -> anyhow::Result<Option<crate::win32::WindowInfo>> {
    tokio::task::spawn_blocking(move || {
        crate::uia::windows_enum::run_uia_with_deadline_cancelable(
            "fresh Notepad document verification",
            std::time::Duration::from_secs(2),
            "no unverified document",
            move |cancelled| unsafe {
                use windows::Win32::UI::Accessibility::{
                    IUIAutomationSelectionItemPattern, UIA_SelectionItemPatternId,
                    UIA_TabItemControlTypeId,
                };
                let _com = Apartment::new();
                let automation: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
                let pids: std::collections::HashSet<_> = crate::win32::list_processes()
                    .into_iter()
                    .filter(|process| super::launch_policy::is_notepad_target(&process.name))
                    .map(|process| process.pid)
                    .collect();
                for window in crate::win32::list_windows_via_win32(None) {
                    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                        return None;
                    }
                    if !pids.contains(&window.pid) {
                        continue;
                    }
                    // One unreadable window or tab must not hide the others.
                    let Ok(root) = automation.ElementFromHandle(HWND(window.hwnd as *mut _)) else {
                        continue;
                    };
                    let condition = automation
                        .CreatePropertyCondition(
                            UIA_ControlTypePropertyId,
                            &UIA_TabItemControlTypeId.0.into(),
                        )
                        .ok()?;
                    let Ok(tabs) = root.FindAll(TreeScope_Subtree, &condition) else {
                        continue;
                    };
                    let count = tabs.Length().unwrap_or(0);
                    if count == 0
                        && super::launch_policy::document_title_matches(&window.title, &filename)
                    {
                        record_document_window(&filename, window.pid);
                        return Some(window); // classic Notepad has no tabs
                    }
                    for index in 0..count {
                        let Ok(tab) = tabs.GetElement(index) else {
                            continue;
                        };
                        let name = tab.CurrentName().map(|n| n.to_string()).unwrap_or_default();
                        if !super::launch_policy::document_title_matches(&name, &filename) {
                            continue;
                        }
                        let selected = tab
                            .GetCurrentPattern(UIA_SelectionItemPatternId)
                            .ok()
                            .and_then(|p| p.cast::<IUIAutomationSelectionItemPattern>().ok())
                            .and_then(|p| p.CurrentIsSelected().ok())
                            .is_some_and(|s| s.as_bool());
                        if selected {
                            record_document_window(&filename, window.pid);
                            return Some(window);
                        }
                    }
                }
                None
            },
        )
        .map_err(|error| {
            anyhow::anyhow!(
                "Fresh document verification stopped because {}",
                crate::uia::click::deadline_reason(error)
            )
        })
    })
    .await?
}

pub(super) async fn additional_tabs(hwnd: u64, requested_tabs: usize) -> Option<usize> {
    tokio::task::spawn_blocking(move || {
        crate::uia::windows_enum::run_uia_with_deadline_cancelable(
            "launch tab count",
            std::time::Duration::from_millis(500),
            "unknown tab count",
            move |_| unsafe {
                use windows::Win32::UI::Accessibility::UIA_TabItemControlTypeId;
                let _com = Apartment::new();
                let automation: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
                let root = automation.ElementFromHandle(HWND(hwnd as *mut _)).ok()?;
                let condition = automation
                    .CreatePropertyCondition(
                        UIA_ControlTypePropertyId,
                        &UIA_TabItemControlTypeId.0.into(),
                    )
                    .ok()?;
                let tabs = root.FindAll(TreeScope_Subtree, &condition).ok()?;
                let mut chrome_tabs = 0;
                for index in 0..tabs.Length().ok()? {
                    let tab = tabs.GetElement(index).ok()?;
                    if !inside_document(&automation, &tab, &root) {
                        chrome_tabs += 1;
                    }
                }
                super::browser_target::additional_tab_count(chrome_tabs, requested_tabs)
            },
        )
        .ok()
        .flatten()
    })
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod fresh_document_tests {
    use super::*;
    #[test]
    fn each_fresh_notepad_file_is_empty_unique_and_never_reuses_a_document() {
        let first = create_notepad_document().unwrap();
        let second = create_notepad_document().unwrap();
        assert_ne!(first, second);
        assert!(std::fs::read(&first).unwrap().is_empty());
        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
    }
}

/// Retain ownership after submission; saved or replaced documents are preserved.
pub(super) struct UnlaunchedDocument(Option<FreshDocument>, Option<DocumentProcess>);
impl UnlaunchedDocument {
    pub(super) fn new(path: Option<std::path::PathBuf>) -> Self {
        let document = path.and_then(|path| FreshDocument::track(path).ok());
        let process = document.as_ref().map(|document| document.process.clone());
        Self(document, process)
    }
    pub(super) fn process_tracker(&self) -> Option<DocumentProcess> {
        self.1.clone()
    }
    pub(super) fn launched(&mut self) {
        if let Some(document) = self.0.take() {
            DOCUMENTS.lock().unwrap().push(document);
        }
    }
}
impl Drop for UnlaunchedDocument {
    fn drop(&mut self) {
        if let Some(process) = &self.1 {
            process.0.lock().unwrap().finished = true;
        }
        if let Some(document) = self.0.as_ref() {
            document.remove_empty();
        }
    }
}

static DOCUMENTS: std::sync::Mutex<Vec<FreshDocument>> = std::sync::Mutex::new(Vec::new());

struct FreshDocument {
    path: std::path::PathBuf,
    identity: (u32, u32, u32, u32, u32),
    scope: Option<String>,
    process: DocumentProcess,
    relinquished: std::sync::atomic::AtomicBool,
}

#[derive(Clone, Default)]
pub(super) struct DocumentProcess(std::sync::Arc<std::sync::Mutex<DocumentProcessState>>);
#[derive(Default)]
struct DocumentProcessState {
    handle: Option<std::os::windows::io::OwnedHandle>,
    verified_window: bool,
    finished: bool,
}
impl DocumentProcess {
    pub(super) fn capture_handle(&self, handle: windows::Win32::Foundation::HANDLE) {
        use std::os::windows::io::BorrowedHandle;
        if handle.is_invalid() {
            return;
        }
        // Duplicate while the launch API still owns its handle: even a process
        // which already exited retains an unambiguous kernel identity.
        let owned = unsafe { BorrowedHandle::borrow_raw(handle.0) }
            .try_clone_to_owned()
            .ok();
        if let Some(owned) = owned {
            self.0.lock().unwrap().handle = Some(owned);
        }
    }
    pub(super) fn capture_pid(&self, pid: u32) {
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
        if self.0.lock().unwrap().handle.is_some() {
            return;
        }
        if let Ok(handle) = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
            self.capture_handle(handle);
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
        }
    }
    fn ready(&self, notepad_running: bool) -> bool {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::{
            Foundation::{HANDLE, WAIT_OBJECT_0},
            System::Threading::WaitForSingleObject,
        };
        let state = self.0.lock().unwrap();
        state.finished
            && match &state.handle {
                Some(handle) => {
                    (unsafe { WaitForSingleObject(HANDLE(handle.as_raw_handle()), 0) })
                        == WAIT_OBJECT_0
                        && (state.verified_window || !notepad_running)
                }
                None => false, // An unproven broker launch can still complete later.
            }
    }
}

fn record_document_window(filename: &str, pid: u32) {
    for document in DOCUMENTS.lock().unwrap().iter() {
        if document
            .path
            .file_name()
            .is_some_and(|name| name == filename)
        {
            let process = DocumentProcess::default();
            process.capture_pid(pid);
            let mut next = process.0.lock().unwrap();
            if let Some(handle) = next.handle.take() {
                let mut state = document.process.0.lock().unwrap();
                state.handle = Some(handle);
                state.verified_window = true;
            }
        }
    }
}

impl FreshDocument {
    fn information(file: &std::fs::File) -> anyhow::Result<(u32, u32, u32, u32, u32)> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe {
            GetFileInformationByHandle(
                windows::Win32::Foundation::HANDLE(file.as_raw_handle()),
                &mut info,
            )?;
        }
        anyhow::ensure!(
            info.dwFileAttributes & 0x410 == 0 && info.nFileSizeHigh == 0 && info.nFileSizeLow == 0,
            "document is not an empty regular file"
        );
        Ok((
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow,
            info.ftLastWriteTime.dwHighDateTime,
            info.ftLastWriteTime.dwLowDateTime,
        ))
    }

    fn track(path: std::path::PathBuf) -> anyhow::Result<Self> {
        let identity = Self::information(&std::fs::File::open(&path)?)?;
        Ok(Self {
            path,
            identity,
            scope: cua_driver_core::tool::current_dispatch_runtime_scope(),
            process: DocumentProcess::default(),
            relinquished: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn remove_empty(&self) -> bool {
        if self.relinquished.load(std::sync::atomic::Ordering::Acquire) {
            return false;
        }
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows::Win32::Storage::FileSystem::{
            FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
            FILE_FLAG_OPEN_REPARSE_POINT,
        };
        // Exclude writers and renames while checking identity/last-write time and
        // deleting by handle, so a save cannot race the empty-file check.
        let Ok(file) = std::fs::OpenOptions::new()
            .access_mode(0x10000 | 0x80)
            .share_mode(1)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&self.path)
        else {
            return false;
        };
        if Self::information(&file).ok() != Some(self.identity) {
            self.relinquished
                .store(true, std::sync::atomic::Ordering::Release);
            return false;
        }
        let disposition = FILE_DISPOSITION_INFO {
            DeleteFile: true.into(),
        };
        unsafe {
            SetFileInformationByHandle(
                windows::Win32::Foundation::HANDLE(file.as_raw_handle()),
                FileDispositionInfo,
                &disposition as *const _ as *const _,
                std::mem::size_of_val(&disposition) as u32,
            )
        }
        .is_ok()
    }
}

pub(super) fn cleanup_documents(scope: Option<&str>) {
    DOCUMENTS.lock().unwrap().retain(|document| {
        document.scope.as_deref() != scope || (!document.remove_empty() && document.path.exists())
    });
}

pub(super) fn poll_documents(notepad_running: bool) {
    DOCUMENTS.lock().unwrap().retain(|document| {
        !document.process.ready(notepad_running)
            || (!document.remove_empty() && document.path.exists())
    });
}

#[cfg(test)]
mod unlaunched_document_tests {
    use super::*;
    #[test]
    fn failed_launch_cleans_only_an_untouched_blank_file() {
        let failed = create_notepad_document().unwrap();
        drop(UnlaunchedDocument::new(Some(failed.clone())));
        assert!(!failed.exists());
        let launched = create_notepad_document().unwrap();
        let mut guard = UnlaunchedDocument::new(Some(launched.clone()));
        guard.launched();
        drop(guard);
        assert!(launched.exists());
        std::fs::remove_file(launched).unwrap();
        let edited = create_notepad_document().unwrap();
        std::fs::write(&edited, "keep this content").unwrap();
        drop(UnlaunchedDocument::new(Some(edited.clone())));
        assert!(edited.exists());
        std::fs::remove_file(edited).unwrap();
    }
}

#[cfg(test)]
mod document_cleanup_tests {
    use super::*;
    #[test]
    fn process_exit_between_polls_cleans_an_unobserved_document() {
        use std::os::windows::{io::AsRawHandle, process::CommandExt};
        let path = create_notepad_document().unwrap();
        let mut guard = UnlaunchedDocument::new(Some(path.clone()));
        let tracker = guard.process_tracker().unwrap();
        guard.launched();
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/C", "exit", "0"])
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        tracker.capture_handle(windows::Win32::Foundation::HANDLE(child.as_raw_handle()));
        child.wait().unwrap();
        assert!(
            !tracker.ready(false),
            "do not clean up while launch verification is pending"
        );
        drop(guard);
        assert!(
            tracker.ready(false),
            "exit is detected without ever observing a running Notepad"
        );
        poll_documents(false);
        assert!(!path.exists());
    }

    #[test]
    fn runtime_cleanup_keeps_another_runtimes_document() {
        let path = create_notepad_document().unwrap();
        let mut document = FreshDocument::track(path.clone()).unwrap();
        let scope = format!(
            "cleanup-test-{}",
            path.file_stem().unwrap().to_string_lossy()
        );
        document.scope = Some(scope.clone());
        DOCUMENTS.lock().unwrap().push(document);
        cleanup_documents(Some("another-runtime"));
        assert!(path.exists());
        cleanup_documents(Some(&scope));
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_keeps_written_truncated_replaced_and_busy_documents() {
        let blank = create_notepad_document().unwrap();
        let owned = FreshDocument::track(blank.clone()).unwrap();
        assert!(owned.remove_empty());
        assert!(!blank.exists());

        let edited = create_notepad_document().unwrap();
        let owned = FreshDocument::track(edited.clone()).unwrap();
        std::fs::write(&edited, "saved content").unwrap();
        assert!(!owned.remove_empty());
        std::fs::write(&edited, "").unwrap();
        assert!(
            !owned.remove_empty(),
            "a saved and emptied file is user-owned"
        );
        std::fs::remove_file(edited).unwrap();

        let replaced = create_notepad_document().unwrap();
        let owned = FreshDocument::track(replaced.clone()).unwrap();
        let replacement = create_notepad_document().unwrap();
        std::fs::remove_file(&replaced).unwrap();
        std::fs::rename(&replacement, &replaced).unwrap();
        assert!(!owned.remove_empty());
        std::fs::remove_file(replaced).unwrap();

        let busy = create_notepad_document().unwrap();
        let owned = FreshDocument::track(busy.clone()).unwrap();
        let writer = std::fs::OpenOptions::new().write(true).open(&busy).unwrap();
        assert!(!owned.remove_empty());
        drop(writer);
        assert!(owned.remove_empty());
    }
}

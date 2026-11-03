//! Read-only Win32 observations for test assertions. Never activates a window.
use super::*;
use std::cell::RefCell;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::JoinHandle;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::*;

type Events = Arc<Mutex<Vec<FocusEvent>>>;
thread_local! { static JOURNAL: RefCell<Option<(Events, Option<u64>)>> = const { RefCell::new(None) }; }
unsafe extern "system" fn changed(
    _: HWINEVENTHOOK,
    _: u32,
    hwnd: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    JOURNAL.with(|slot| {
        if let Some((events, previous)) = slot.borrow_mut().as_mut() {
            let next = (!hwnd.0.is_null()).then_some(hwnd.0 as u64);
            if next != *previous {
                events.lock().unwrap().push(FocusEvent {
                    from: *previous,
                    to: next,
                });
                *previous = next;
            }
        }
    });
}
fn front() -> Option<u64> {
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.0.is_null()).then_some(hwnd.0 as u64)
}

pub struct WindowsObserver {
    stop: Arc<AtomicBool>,
    events: Events,
    sampler: Option<JoinHandle<()>>,
}
impl Default for WindowsObserver {
    fn default() -> Self {
        Self::new()
    }
}
impl WindowsObserver {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            events: Arc::new(Mutex::new(Vec::new())),
            sampler: None,
        }
    }
    fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.sampler.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for WindowsObserver {
    fn drop(&mut self) {
        self.stop();
    }
}
impl ObserverBackend for WindowsObserver {
    fn capabilities(&self) -> ObserverCapabilities {
        ObserverCapabilities {
            focus: true,
            z_order: true,
            cursor: true,
            leaked_input: false,
        }
    }
    fn snapshot(&self, target: TargetWindow) -> Result<DesktopSnapshot, ObserverError> {
        let foreground = front()
            .ok_or_else(|| ObserverError::new("Windows desktop foreground is unavailable"))?;
        let hwnd = HWND(target.native_id as *mut _);
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        let target_z = if !unsafe { IsWindow(hwnd) }.as_bool() || pid != target.pid {
            TargetZ::NotFound
        } else if unsafe { IsIconic(hwnd) }.as_bool() || !unsafe { IsWindowVisible(hwnd) }.as_bool()
        {
            TargetZ::Minimized
        } else if front() == Some(target.native_id) {
            TargetZ::Foreground
        } else {
            let mut bounds = RECT::default();
            unsafe { GetWindowRect(hwnd, &mut bounds) }
                .map_err(|e| ObserverError::new(e.to_string()))?;
            let area = (bounds.right - bounds.left).max(0) as i64
                * (bounds.bottom - bounds.top).max(0) as i64;
            if area == 0 {
                return Err(ObserverError::new("target window has no observable bounds"));
            }
            let mut other = unsafe { GetTopWindow(None) }.unwrap_or_default();
            let mut covered = false;
            // Z order is top to bottom. Bound traversal in case windows disappear.
            for _ in 0..4096 {
                if other.0.is_null() || other == hwnd {
                    break;
                }
                if unsafe { IsWindowVisible(other) }.as_bool()
                    && !unsafe { IsIconic(other) }.as_bool()
                {
                    let mut rect = RECT::default();
                    if unsafe { GetWindowRect(other, &mut rect) }.is_ok() {
                        let width = (bounds.right.min(rect.right) - bounds.left.max(rect.left))
                            .max(0) as i64;
                        let height = (bounds.bottom.min(rect.bottom) - bounds.top.max(rect.top))
                            .max(0) as i64;
                        covered |= width * height >= area / 10;
                    }
                }
                other = unsafe { GetWindow(other, GW_HWNDNEXT) }.unwrap_or_default();
            }
            if covered {
                TargetZ::BackgroundOccluded
            } else {
                TargetZ::BackgroundVisible
            }
        };
        let mut cursor = windows::Win32::Foundation::POINT::default();
        let cursor_pos = unsafe { GetCursorPos(&mut cursor) }
            .ok()
            .map(|_| (cursor.x as f64, cursor.y as f64));
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        let input_focus = unsafe { GetGUIThreadInfo(0, &mut info) }
            .ok()
            .and_then(|_| (!info.hwndFocus.0.is_null()).then_some(info.hwndFocus.0 as u64));
        Ok(DesktopSnapshot {
            foreground: Some(foreground),
            input_focus,
            target_z,
            cursor_pos,
        })
    }
    fn start_journal(&mut self) -> Result<(), ObserverError> {
        if self.sampler.is_some() {
            return Err(ObserverError::new("Windows journal already active"));
        }
        self.events.lock().unwrap().clear();
        self.stop.store(false, Ordering::Release);
        let events = self.events.clone();
        let stop = self.stop.clone();
        let (ready, receive) = std::sync::mpsc::sync_channel(1);
        self.sampler = Some(std::thread::spawn(move || {
            JOURNAL.with(|slot| *slot.borrow_mut() = Some((events, front())));
            let hook = unsafe {
                SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND,
                    EVENT_SYSTEM_FOREGROUND,
                    None,
                    Some(changed),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                )
            };
            let _ = ready.send(!hook.0.is_null());
            if !hook.0.is_null() {
                loop {
                    let mut msg = MSG::default();
                    for _ in 0..128 {
                        if !unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                            break;
                        }
                        unsafe {
                            DispatchMessageW(&msg);
                        }
                    }
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                let _ = unsafe { UnhookWinEvent(hook) };
            }
            JOURNAL.with(|slot| *slot.borrow_mut() = None);
        }));
        if receive.recv_timeout(Duration::from_secs(1)) != Ok(true) {
            self.stop();
            return Err(ObserverError::new(
                "could not install Windows foreground event hook",
            ));
        }
        Ok(())
    }
    fn drain_journal(&mut self) -> Result<DesktopJournal, ObserverError> {
        self.stop();
        Ok(DesktopJournal {
            focus_events: std::mem::take(&mut *self.events.lock().unwrap()),
            leaked_input_events: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_window_is_not_reported_as_a_visible_target() {
        let result = WindowsObserver::new().snapshot(TargetWindow {
            pid: u32::MAX,
            native_id: u64::MAX,
        });
        match result {
            Ok(snapshot) => assert_eq!(snapshot.target_z, TargetZ::NotFound),
            Err(error) => assert!(error
                .to_string()
                .contains("desktop foreground is unavailable")),
        }
        assert!(!WindowsObserver::new().capabilities().leaked_input);
    }
}

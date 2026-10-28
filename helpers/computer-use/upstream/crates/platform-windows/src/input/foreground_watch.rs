//! A bounded event-backed foreground watch, shared by launch and input guards.
//! Out-of-context WinEvents record even switches shorter than a polling tick.
//! Restoration uses no synthetic keys and never moves the pointer.
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetAncestor, GetForegroundWindow, GetWindowThreadProcessId,
    IsWindow, PeekMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WindowFromPoint,
    EVENT_SYSTEM_FOREGROUND, GA_ROOT, KBDLLHOOKSTRUCT, LLKHF_ALTDOWN, LLKHF_INJECTED,
    LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, PM_REMOVE, WH_KEYBOARD_LL, WH_MOUSE_LL,
    WINEVENT_OUTOFCONTEXT, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_RBUTTONDOWN,
    WM_SYSKEYDOWN, WM_XBUTTONDOWN,
};

#[path = "foreground_policy.rs"]
mod policy;

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Launch(Vec<String>),
    Processes(Vec<u32>),
}

impl Target {
    pub(crate) fn launch(expected: Vec<String>) -> Self {
        Self::Launch(policy::launch_names(expected))
    }
}

struct State {
    observation: policy::Observation,
    ended: Option<Instant>,
    evidence: policy::ActivationEvidence,
    holders: usize,
}

struct Watch {
    prior: usize,
    prior_pid: u32,
    user_selected: AtomicBool,
    started: Instant,
    cancelled: AtomicBool,
    target: Target,
    state: Mutex<State>,
}

thread_local! {
    static CURRENT: RefCell<Option<Weak<Watch>>> = const { RefCell::new(None) };
}

pub(crate) struct ForegroundWatch {
    watch: Arc<Watch>,
    requests: mpsc::Sender<mpsc::SyncSender<(policy::Observation, usize)>>,
}

// Reuse an existing app watch across nested guards and successive run_steps.
// Its worker owns the sender until its finite deadline, avoiding one thread
// for every click in a burst. A retired watch is never reused.
struct LiveWatch {
    watch: Weak<Watch>,
    requests: mpsc::Sender<mpsc::SyncSender<(policy::Observation, usize)>>,
}
static LIVE: Mutex<Vec<LiveWatch>> = Mutex::new(Vec::new());

pub(crate) fn window_process(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut path = vec![0u16; 32768];
        let mut len = path.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(path.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        result.ok()?;
        let path = String::from_utf16_lossy(&path[..len as usize]).to_ascii_lowercase();
        Some(
            path.rsplit(['\\', '/'])
                .next()?
                .trim_end_matches(".exe")
                .to_owned(),
        )
    }
}

impl Watch {
    fn observe(&self, event_hwnd: HWND) {
        let prior = HWND(self.prior as *mut _);
        let prior_valid = self.prior != 0
            && unsafe { IsWindow(prior) }.as_bool()
            && window_process(prior) == self.prior_pid;
        let input_unchanged = !self.user_selected.load(Ordering::Acquire);
        {
            let mut state = self.state.lock().unwrap();
            if !state.observation.monitoring {
                return;
            }
            if !prior_valid || !input_unchanged {
                state.observation.interrupted = true;
                state.observation.monitoring = false;
                return;
            }
        }
        if event_hwnd == prior || event_hwnd.0.is_null() {
            return;
        }
        let pid = window_process(event_hwnd);
        let target = match &self.target {
            Target::Processes(targets) => pid != 0 && targets.contains(&pid),
            Target::Launch(names) => process_name(pid).is_some_and(|name| names.contains(&name)),
        };
        let target = {
            let mut state = self.state.lock().unwrap();
            let target = state.evidence.observe(pid, target);
            state.observation.activated = state.evidence.activated;
            target
        };
        if !target {
            return;
        }
        // An event may describe a transient switch already restored by Windows.
        // Record it, but never overwrite a newer foreground from an old event.
        if policy::may_hand_back(
            prior_valid,
            !self.user_selected.load(Ordering::Acquire),
            target,
            unsafe { GetForegroundWindow() } == event_hwnd,
        ) {
            unsafe { super::force_foreground_attached(prior) };
        }
    }

    fn snapshot(&self) -> (policy::Observation, usize) {
        self.observe(unsafe { GetForegroundWindow() });
        let mut result = self.state.lock().unwrap().observation;
        let foreground = unsafe { GetForegroundWindow() }.0 as usize;
        result.restored = foreground == self.prior;
        (result, foreground)
    }
}

unsafe extern "system" fn foreground_event(
    _: HWINEVENTHOOK,
    _: u32,
    hwnd: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    let watch = CURRENT.with(|slot| slot.borrow().as_ref().and_then(Weak::upgrade));
    if let Some(watch) = watch {
        watch.observe(hwnd);
    }
}

// Read-only hooks distinguish real window selection from motion and from our
// injected input. No event is swallowed and no input is synthesized.
unsafe extern "system" fn mouse_event(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0
        && matches!(
            wparam.0 as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        )
    {
        let event = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        CURRENT.with(|slot| {
            if let Some(watch) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                let clicked = unsafe { GetAncestor(WindowFromPoint(event.pt), GA_ROOT) };
                if policy::switch_click(
                    event.flags & LLMHF_INJECTED != 0,
                    clicked.0 as usize == watch.prior,
                ) {
                    watch.user_selected.store(true, Ordering::Release);
                }
            }
        });
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn keyboard_event(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if policy::switch_key(
            event.vkCode,
            event.flags.contains(LLKHF_ALTDOWN),
            unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } < 0,
            event.flags.contains(LLKHF_INJECTED),
        ) {
            CURRENT.with(|slot| {
                if let Some(watch) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                    watch.user_selected.store(true, Ordering::Release);
                }
            });
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

impl ForegroundWatch {
    pub(crate) fn start(prior: usize, target: Target) -> Self {
        let mut live = LIVE.lock().unwrap();
        live.retain(|entry| {
            entry
                .watch
                .upgrade()
                .is_some_and(|watch| watch.state.lock().unwrap().observation.monitoring)
        });
        for entry in live.iter() {
            if let Some(watch) = entry.watch.upgrade() {
                if watch.prior == prior
                    && watch.target == target
                    && matches!(target, Target::Processes(_))
                    && watch.started.elapsed() < Duration::from_secs(20)
                {
                    let mut state = watch.state.lock().unwrap();
                    if state.observation.monitoring && !watch.user_selected.load(Ordering::Acquire)
                    {
                        state.holders += 1;
                        state.ended = None;
                        drop(state);
                        return Self {
                            watch,
                            requests: entry.requests.clone(),
                        };
                    }
                }
            }
        }
        let watch = Arc::new(Watch {
            prior,
            prior_pid: window_process(HWND(prior as *mut _)),
            user_selected: AtomicBool::new(false),
            started: Instant::now(),
            cancelled: AtomicBool::new(false),
            target,
            state: Mutex::new(State {
                observation: policy::Observation::default(),
                ended: None,
                evidence: policy::ActivationEvidence::default(),
                holders: 1,
            }),
        });
        let (requests, receive) = mpsc::channel::<mpsc::SyncSender<(policy::Observation, usize)>>();
        let (ready, armed) = mpsc::sync_channel(1);
        let worker = watch.clone();
        std::thread::spawn(move || {
            CURRENT.with(|slot| *slot.borrow_mut() = Some(Arc::downgrade(&worker)));
            let hook = unsafe {
                SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND,
                    EVENT_SYSTEM_FOREGROUND,
                    None,
                    Some(foreground_event),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                )
            };
            let module = unsafe { GetModuleHandleW(None) }
                .ok()
                .map(|module| HINSTANCE(module.0))
                .unwrap_or_default();
            let mouse =
                unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_event), module, 0) }.ok();
            let keyboard =
                unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_event), module, 0) }.ok();
            let monitoring = !hook.0.is_null()
                && mouse.is_some()
                && keyboard.is_some()
                && prior != 0
                && unsafe { GetForegroundWindow() }.0 as usize == prior;
            worker.state.lock().unwrap().observation.monitoring = monitoring;
            let _ = ready.send(());
            while monitoring && !worker.cancelled.load(Ordering::Acquire) {
                let mut message = MSG::default();
                // Pump the hook's owning thread; callbacks are delivered here.
                // Bound each drain as an app may produce events continuously.
                for _ in 0..128 {
                    if !unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                        break;
                    }
                    unsafe { DispatchMessageW(&message) };
                }
                worker.observe(unsafe { GetForegroundWindow() });
                for reply in receive.try_iter().take(128) {
                    let _ = reply.send(worker.snapshot());
                }
                let state = worker.state.lock().unwrap();
                if !state.observation.monitoring
                    || policy::expired(
                        worker.started.elapsed().as_millis(),
                        state.ended.map(|end| end.elapsed().as_millis()),
                    )
                {
                    break;
                }
                drop(state);
                std::thread::sleep(Duration::from_millis(2));
            }
            worker.state.lock().unwrap().observation.monitoring = false;
            if !hook.0.is_null() {
                let _ = unsafe { UnhookWinEvent(hook) };
            }
            if let Some(mouse) = mouse {
                let _ = unsafe { UnhookWindowsHookEx(mouse) };
            }
            if let Some(keyboard) = keyboard {
                let _ = unsafe { UnhookWindowsHookEx(keyboard) };
            }
            CURRENT.with(|slot| *slot.borrow_mut() = None);
        });
        // Ready before any activation is sent. Failure remains an unknown
        // observation in reports, never a false "no activation" claim.
        if armed.recv_timeout(Duration::from_millis(250)).is_err() {
            watch.cancelled.store(true, Ordering::Release);
            watch.state.lock().unwrap().observation.monitoring = false;
        }
        live.push(LiveWatch {
            watch: Arc::downgrade(&watch),
            requests: requests.clone(),
        });
        Self { watch, requests }
    }

    pub(crate) fn register_pid(&self, pid: u32) {
        if pid != 0 {
            let mut state = self.watch.state.lock().unwrap();
            state.evidence.bind(pid);
            state.observation.activated = state.evidence.activated;
        }
    }

    pub(crate) fn report(&self) -> (&'static str, bool, usize) {
        let (reply, receive) = mpsc::sync_channel(1);
        if self.requests.send(reply).is_ok() {
            if let Ok((observation, foreground)) = receive.recv_timeout(Duration::from_millis(100))
            {
                let (note, failed) = observation.report();
                return (note, failed, foreground);
            }
        }
        let mut observation = self.watch.state.lock().unwrap().observation;
        let foreground = unsafe { GetForegroundWindow() }.0 as usize;
        observation.restored = foreground == self.watch.prior;
        observation.monitoring = false;
        let (note, failed) = observation.report();
        (note, failed, foreground)
    }
}

impl Drop for ForegroundWatch {
    fn drop(&mut self) {
        let mut state = self.watch.state.lock().unwrap();
        state.holders = state.holders.saturating_sub(1);
        if state.holders == 0 {
            state.ended = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn null_prior_is_reported_unconfirmed_without_touching_foreground() {
        let watch = ForegroundWatch::start(0, Target::Processes(Vec::new()));
        assert!(watch.report().1);
    }
}

//! A bounded event-backed foreground watch, shared by launch and input guards.
//! Out-of-context WinEvents record even switches shorter than a polling tick.
//! Restoration uses no synthetic keys and never moves the pointer.
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    WINEVENT_OUTOFCONTEXT, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

#[path = "foreground_policy.rs"]
pub(super) mod policy;

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Launch(Vec<String>),
    Processes(Vec<u32>, Vec<usize>),
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
    // Any physical key or button press: the user may be opening something.
    user_active: AtomicBool,
    first_input: AtomicU64,
    input_generation: u64,
    hold_lock: bool,
    restore_allowed: bool,
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
    fn observe(&self, event_hwnd: HWND, event_time: Option<u32>) {
        let prior = HWND(self.prior as *mut _);
        let prior_valid = self.prior != 0
            && unsafe { IsWindow(prior) }.as_bool()
            && window_process(prior) == self.prior_pid;
        let input_unchanged = !self.user_active.load(Ordering::Acquire)
            && super::foreground_generation() == self.input_generation;
        let first = self
            .first_input
            .load(Ordering::Acquire)
            .checked_sub(1)
            .map(|time| time as u32)
            .or_else(super::foreground_input_time);
        // A queued pre-input event is still evidence, never permission to restore.
        let record = policy::record_before_input(input_unchanged, event_time, first);
        if !input_unchanged || !prior_valid {
            let mut state = self.state.lock().unwrap();
            state.observation.interrupted = true;
            state.observation.user_input |= !input_unchanged;
        }
        if !record {
            return;
        }
        if event_hwnd.0.is_null() {
            return;
        }
        if event_hwnd == prior {
            return;
        }
        // Keep transient event evidence; check current foreground before restoration.
        let pid = window_process(event_hwnd);
        let named = match &self.target {
            Target::Processes(targets, windows) => policy::input_window_matches(
                pid != 0 && targets.contains(&pid),
                windows.contains(&(event_hwnd.0 as usize)),
                process_name(pid).is_none_or(|name| name == "applicationframehost"),
            ),
            Target::Launch(names) => process_name(pid).is_some_and(|name| names.contains(&name)),
        };
        let target = {
            let mut state = self.state.lock().unwrap();
            let target = state.evidence.observe(pid, named);
            state.observation.activated = state.evidence.activated;
            state.observation.uncertain_broker |= !target
                && matches!(self.target, Target::Launch(_))
                && process_name(pid).as_deref() == Some("applicationframehost");
            state.observation.foreground_seen |=
                target && unsafe { GetForegroundWindow() } == event_hwnd;
            target
        };
        if !target {
            return;
        }
        if self.restore_allowed
            && policy::may_hand_back(
                prior_valid,
                input_unchanged,
                target,
                unsafe { GetForegroundWindow() } == event_hwnd,
            )
        {
            unsafe {
                super::hand_back_foreground(prior, event_hwnd, || {
                    !self.user_active.load(Ordering::Acquire)
                        && super::foreground_generation() == self.input_generation
                })
            };
        }
    }

    fn snapshot(&self) -> (policy::Observation, usize) {
        self.observe(unsafe { GetForegroundWindow() }, None);
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
    event_time: u32,
) {
    let watch = CURRENT.with(|slot| slot.borrow().as_ref().and_then(Weak::upgrade));
    if let Some(watch) = watch {
        watch.observe(hwnd, Some(event_time));
    }
}

// Read-only hooks distinguish real window selection from motion and from our
// injected input. No event is swallowed and no input is synthesized.
unsafe extern "system" fn mouse_event(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0
        && matches!(
            wparam.0 as u32,
            WM_LBUTTONDOWN
                | WM_RBUTTONDOWN
                | WM_MBUTTONDOWN
                | WM_XBUTTONDOWN
                | WM_LBUTTONUP
                | WM_RBUTTONUP
                | WM_MBUTTONUP
                | WM_XBUTTONUP
                | WM_MOUSEWHEEL
                | WM_MOUSEHWHEEL
        )
    {
        let event = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        CURRENT.with(|slot| {
            if let Some(watch) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                if event.flags & LLMHF_INJECTED == 0 {
                    watch
                        .first_input
                        .compare_exchange(
                            0,
                            u64::from(event.time) + 1,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .ok();
                    watch.user_active.store(true, Ordering::Release);
                    super::revoke_foreground_lock(event.time);
                }
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
    if code >= 0
        && matches!(
            wparam.0 as u32,
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP
        )
    {
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if !event.flags.contains(LLKHF_INJECTED) {
            CURRENT.with(|slot| {
                if let Some(watch) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                    watch
                        .first_input
                        .compare_exchange(
                            0,
                            u64::from(event.time) + 1,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .ok();
                    watch.user_active.store(true, Ordering::Release);
                    super::revoke_foreground_lock(event.time);
                }
            });
        }
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
    /// `hold_lock` keeps other processes from taking the front for the whole
    /// watch, not just the call; it is lifted at the user's first own input.
    pub(crate) fn start(prior: usize, target: Target, hold_lock: bool) -> Self {
        Self::start_mode(prior, target, hold_lock, true)
    }

    /// Records input/event evidence without acquiring a lock or restoring a
    /// window. Used by transports which must not manipulate native focus.
    pub(crate) fn start_observing(prior: usize, target: Target) -> Self {
        Self::start_mode(prior, target, false, false)
    }

    /// Holds the activation-prevention lock like [`Self::start`] (lifted at
    /// the user's first own input) but never restores a window.
    pub(crate) fn start_locked_observing(prior: usize, target: Target) -> Self {
        Self::start_mode(prior, target, true, false)
    }

    fn start_mode(prior: usize, target: Target, hold_lock: bool, restore_allowed: bool) -> Self {
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
                    && watch.restore_allowed == restore_allowed
                    && watch.hold_lock == hold_lock
                    && watch.target == target
                    && matches!(target, Target::Processes(_, _))
                    && watch.started.elapsed() < Duration::from_secs(20)
                {
                    let mut state = watch.state.lock().unwrap();
                    if state.observation.monitoring && !watch.user_active.load(Ordering::Acquire) {
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
            user_active: AtomicBool::new(false),
            first_input: AtomicU64::new(0),
            input_generation: super::foreground_generation(),
            hold_lock,
            restore_allowed,
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
            // Locked before the caller acts; Windows lifts it on the user's own input.
            let mut lock = (monitoring && worker.hold_lock)
                .then(super::ForegroundLockGuard::acquire)
                .filter(super::ForegroundLockGuard::acquired);
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
                if worker.user_active.load(Ordering::Acquire) {
                    lock = None;
                }
                worker.observe(unsafe { GetForegroundWindow() }, None);
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
                if worker.user_active.load(Ordering::Acquire) {
                    lock = None;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            worker.state.lock().unwrap().observation.monitoring = false;
            drop(lock);
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
                let (note, failed) = self.describe(observation);
                return (note, failed, foreground);
            }
        }
        let mut observation = self.watch.state.lock().unwrap().observation;
        let foreground = unsafe { GetForegroundWindow() }.0 as usize;
        observation.restored = foreground == self.watch.prior;
        observation.monitoring = false;
        let (note, failed) = self.describe(observation);
        (note, failed, foreground)
    }

    fn describe(&self, observation: policy::Observation) -> (&'static str, bool) {
        if self.watch.restore_allowed {
            return observation.report();
        }
        if observation.activated || observation.uncertain_broker {
            ("Windows reported that an app window became active during this launch. This launch only observes activation and never switches windows back, so whether your window stayed active is unconfirmed.", true)
        } else {
            ("No app activation was recorded before this reply. Activation stays observed but is never reversed, and this is not a guarantee against a later flash.", true)
        }
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
        let watch = ForegroundWatch::start(0, Target::Processes(Vec::new(), Vec::new()), false);
        assert!(watch.report().1);
    }
}

//! Host-owned native application surfaces.
//!
//! ScreenCaptureKit and CGEvent remain in the independently registered helper
//! process, so Brigadier itself never acquires Screen Recording or Accessibility
//! authority. Control and input use the authenticated daemon socket. Frames use
//! a permission-restricted, versioned triple-buffer in POSIX shared memory.

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CString};
use std::os::fd::RawFd;
use std::ptr::NonNull;
use std::sync::atomic::{fence, AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::capture_deadline::{self, CaptureOperation, SystemCaptureClock};
use anyhow::{anyhow, bail, Context};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventType, CGMouseButton, ScrollEventUnit};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use foreign_types::ForeignType;
use screencapturekit::cm::{CMSampleBufferExt, CMSampleBufferSCExt};
use screencapturekit::prelude::{
    PixelFormat, SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamOutputType,
};
use screencapturekit::stream::StreamCallbacks;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{permissions, windows};

const FRAME_HEADER_BYTE_COUNT: usize = 64;
const FRAME_PUBLISHED_WORD_OFFSET: usize = 32;
const FRAME_SLOT_VERSION_OFFSET: usize = 40;
const FRAME_SLOT_VERSION_STRIDE: usize = 8;
const FRAME_MAGIC: u32 = 0x434D_5846;
const FRAME_VERSION: u32 = 2;
const FRAME_PIXEL_BYTE_COUNT: usize = 4;
const FRAME_SLOT_COUNT: usize = 3;
const MAXIMUM_FRAME_DIMENSION: usize = 16_384;
const MAXIMUM_FRAME_RING_BYTE_COUNT: usize = 256 * 1_024 * 1_024;
const BGRA_PIXEL_FORMAT: u32 = u32::from_be_bytes(*b"BGRA");

#[derive(Debug, Clone, Serialize)]
pub struct ApplicationWindow {
    pub window_id: u32,
    pub process_id: i32,
    pub owner: String,
    pub title: String,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameTransportDescriptor {
    pub shared_memory_name: String,
    pub width: usize,
    pub height: usize,
    pub bytes_per_row: usize,
    pub slot_count: usize,
    pub shared_memory_byte_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationSurfaceStartResult {
    pub session_id: String,
    pub frame_transport: FrameTransportDescriptor,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApplicationSurfaceStartRequest {
    pub window_id: u32,
    pub process_id: i32,
    #[serde(default = "default_frame_rate")]
    pub frame_rate: u32,
}

fn default_frame_rate() -> u32 {
    60
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApplicationSurfaceEvent {
    pub session: String,
    pub kind: String,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub button: String,
    #[serde(default)]
    pub key_code: u16,
    #[serde(default)]
    pub key_down: bool,
    #[serde(default)]
    pub modifiers: u64,
    #[serde(default = "default_click_count")]
    pub click_count: i64,
    #[serde(default)]
    pub delta_x: f64,
    #[serde(default)]
    pub delta_y: f64,
}

fn default_click_count() -> i64 {
    1
}

#[derive(Debug, Clone, Copy)]
struct FrameLayout {
    width: usize,
    height: usize,
    bytes_per_row: usize,
    slot_byte_count: usize,
    total_byte_count: usize,
}

impl FrameLayout {
    fn new(width: usize, height: usize) -> anyhow::Result<Self> {
        if !(1..=MAXIMUM_FRAME_DIMENSION).contains(&width)
            || !(1..=MAXIMUM_FRAME_DIMENSION).contains(&height)
        {
            bail!("frame dimensions are outside supported bounds");
        }
        let bytes_per_row = width
            .checked_mul(FRAME_PIXEL_BYTE_COUNT)
            .ok_or_else(|| anyhow!("frame row size overflowed"))?;
        let slot_byte_count = bytes_per_row
            .checked_mul(height)
            .ok_or_else(|| anyhow!("frame slot size overflowed"))?;
        let unaligned = FRAME_HEADER_BYTE_COUNT
            .checked_add(
                slot_byte_count
                    .checked_mul(FRAME_SLOT_COUNT)
                    .ok_or_else(|| anyhow!("frame ring size overflowed"))?,
            )
            .ok_or_else(|| anyhow!("frame ring size overflowed"))?;
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if page_size <= 0 {
            bail!("page geometry is unavailable");
        }
        let page_size = usize::try_from(page_size).context("invalid page geometry")?;
        let padding = (page_size - (unaligned % page_size)) % page_size;
        let total_byte_count = unaligned
            .checked_add(padding)
            .ok_or_else(|| anyhow!("frame mapping size overflowed"))?;
        if total_byte_count > MAXIMUM_FRAME_RING_BYTE_COUNT {
            bail!("frame mapping exceeds the supported size");
        }
        Ok(Self {
            width,
            height,
            bytes_per_row,
            slot_byte_count,
            total_byte_count,
        })
    }

    fn slot_offset(self, slot: usize) -> Option<usize> {
        (slot < FRAME_SLOT_COUNT).then(|| FRAME_HEADER_BYTE_COUNT + slot * self.slot_byte_count)
    }
}

struct SharedFrameRing {
    name: CString,
    mapping: NonNull<u8>,
    layout: FrameLayout,
    next_sequence: AtomicU64,
    is_closed: AtomicBool,
}

// SAFETY: publication is serialized by ScreenCaptureKit's screen-output queue.
// The only concurrently read fields are immutable or atomic. The mapped atomic
// words are naturally aligned by the fixed protocol header.
unsafe impl Send for SharedFrameRing {}
unsafe impl Sync for SharedFrameRing {}

impl SharedFrameRing {
    fn create(width: usize, height: usize) -> anyhow::Result<Arc<Self>> {
        let layout = FrameLayout::new(width, height)?;
        let (name, descriptor_handle) = create_shared_memory()?;
        let result = Self::map(name, descriptor_handle, layout);
        unsafe {
            libc::close(descriptor_handle);
        }
        result.map(Arc::new)
    }

    fn map(name: CString, descriptor_handle: RawFd, layout: FrameLayout) -> anyhow::Result<Self> {
        if unsafe { libc::ftruncate(descriptor_handle, layout.total_byte_count as libc::off_t) }
            != 0
        {
            unlink_shared_memory(&name);
            return Err(std::io::Error::last_os_error()).context("could not size frame ring");
        }
        let raw_mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                layout.total_byte_count,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                descriptor_handle,
                0,
            )
        };
        let mapping = NonNull::new(raw_mapping.cast::<u8>())
            .filter(|pointer| pointer.as_ptr().cast::<c_void>() != libc::MAP_FAILED)
            .ok_or_else(std::io::Error::last_os_error)
            .context("could not map frame ring")?;

        let ring = Self {
            name,
            mapping,
            layout,
            next_sequence: AtomicU64::new(0),
            is_closed: AtomicBool::new(false),
        };
        ring.initialize_header();
        Ok(ring)
    }

    fn descriptor(&self) -> FrameTransportDescriptor {
        FrameTransportDescriptor {
            shared_memory_name: self.name.to_string_lossy().into_owned(),
            width: self.layout.width,
            height: self.layout.height,
            bytes_per_row: self.layout.bytes_per_row,
            slot_count: FRAME_SLOT_COUNT,
            shared_memory_byte_count: self.layout.total_byte_count,
        }
    }

    fn initialize_header(&self) {
        unsafe {
            std::ptr::write_bytes(self.mapping.as_ptr(), 0, FRAME_HEADER_BYTE_COUNT);
            self.write_header_value(0, FRAME_MAGIC);
            self.write_header_value(4, FRAME_VERSION);
            self.write_header_value(8, self.layout.width as u32);
            self.write_header_value(12, self.layout.height as u32);
            self.write_header_value(16, self.layout.bytes_per_row as u32);
            self.write_header_value(20, FRAME_SLOT_COUNT as u32);
            self.write_header_value(24, self.layout.total_byte_count as u64);
            self.atomic_word(FRAME_PUBLISHED_WORD_OFFSET)
                .store(0, Ordering::Release);
            for slot in 0..FRAME_SLOT_COUNT {
                self.atomic_word(FRAME_SLOT_VERSION_OFFSET + slot * FRAME_SLOT_VERSION_STRIDE)
                    .store(0, Ordering::Release);
            }
        }
    }

    unsafe fn write_header_value<T: Copy>(&self, offset: usize, value: T) {
        unsafe {
            self.mapping
                .as_ptr()
                .add(offset)
                .cast::<T>()
                .write_unaligned(value);
        }
    }

    unsafe fn atomic_word(&self, offset: usize) -> &AtomicU64 {
        unsafe { &*self.mapping.as_ptr().add(offset).cast::<AtomicU64>() }
    }

    fn publish(&self, source: &[u8], source_bytes_per_row: usize) -> anyhow::Result<()> {
        if self.is_closed.load(Ordering::Acquire) {
            bail!("frame ring is closed");
        }
        if source_bytes_per_row < self.layout.bytes_per_row
            || source.len() < source_bytes_per_row.saturating_mul(self.layout.height)
        {
            bail!("captured frame has inconsistent row geometry");
        }
        let sequence = self
            .next_sequence
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(1)
                    .filter(|next| *next <= (i64::MAX as u64 >> 2))
            })
            .map_err(|_| anyhow!("frame sequence exceeded transport bounds"))?
            + 1;
        let slot = usize::try_from((sequence - 1) % FRAME_SLOT_COUNT as u64)
            .expect("triple-ring slot always fits usize");
        let completed_version = sequence
            .checked_mul(2)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or_else(|| anyhow!("frame version exceeded transport bounds"))?;
        let published_word = (sequence << 2) | slot as u64;
        let slot_offset = self
            .layout
            .slot_offset(slot)
            .ok_or_else(|| anyhow!("invalid frame slot"))?;

        unsafe {
            self.atomic_word(FRAME_SLOT_VERSION_OFFSET + slot * FRAME_SLOT_VERSION_STRIDE)
                .swap(completed_version - 1, Ordering::AcqRel);
            let destination = self.mapping.as_ptr().add(slot_offset);
            if source_bytes_per_row == self.layout.bytes_per_row {
                std::ptr::copy_nonoverlapping(
                    source.as_ptr(),
                    destination,
                    self.layout.slot_byte_count,
                );
            } else {
                for row in 0..self.layout.height {
                    std::ptr::copy_nonoverlapping(
                        source.as_ptr().add(row * source_bytes_per_row),
                        destination.add(row * self.layout.bytes_per_row),
                        self.layout.bytes_per_row,
                    );
                }
            }
            self.atomic_word(FRAME_SLOT_VERSION_OFFSET + slot * FRAME_SLOT_VERSION_STRIDE)
                .store(completed_version, Ordering::Release);
            self.atomic_word(FRAME_PUBLISHED_WORD_OFFSET)
                .store(published_word, Ordering::Release);
        }
        fence(Ordering::SeqCst);
        post_frame_notification(&self.name)?;
        Ok(())
    }
}

impl Drop for SharedFrameRing {
    fn drop(&mut self) {
        if self.is_closed.swap(true, Ordering::AcqRel) {
            return;
        }
        unsafe {
            libc::munmap(
                self.mapping.as_ptr().cast::<c_void>(),
                self.layout.total_byte_count,
            );
        }
        unlink_shared_memory(&self.name);
    }
}

fn create_shared_memory() -> anyhow::Result<(CString, RawFd)> {
    for _ in 0..8 {
        let token = Uuid::new_v4().simple().to_string();
        let name = CString::new(format!("/bcu-frame-{}", &token[..12]))
            .expect("generated shared-memory names contain no NUL");
        let handle = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600,
            )
        };
        if handle >= 0 {
            if unsafe { libc::fcntl(handle, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
                let error = std::io::Error::last_os_error();
                unsafe {
                    libc::close(handle);
                }
                unlink_shared_memory(&name);
                return Err(error).context("could not protect frame-ring descriptor");
            }
            return Ok((name, handle));
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EEXIST) {
            return Err(error).context("could not create frame ring");
        }
    }
    bail!("could not reserve a unique frame-ring name")
}

fn unlink_shared_memory(name: &CString) {
    unsafe {
        libc::shm_unlink(name.as_ptr());
    }
}

fn post_frame_notification(shared_memory_name: &CString) -> anyhow::Result<()> {
    let suffix = shared_memory_name
        .to_str()
        .context("frame-ring name is not UTF-8")?
        .strip_prefix('/')
        .ok_or_else(|| anyhow!("frame-ring name has no POSIX prefix"))?;
    let notification = CString::new(format!("com.brigadier.simulator.frame.{suffix}"))
        .expect("generated notification names contain no NUL");
    let result = unsafe { notify_post(notification.as_ptr()) };
    if result == 0 {
        Ok(())
    } else {
        bail!("could not signal frame publication")
    }
}

#[link(name = "System")]
extern "C" {
    fn notify_post(name: *const c_char) -> u32;
}

#[derive(Debug, Clone, Copy)]
struct NormalizedContentRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Default for NormalizedContentRect {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }
    }
}

impl NormalizedContentRect {
    fn from_frame_rect(
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        frame_width: usize,
        frame_height: usize,
    ) -> Option<Self> {
        let frame_width = frame_width as f64;
        let frame_height = frame_height as f64;
        if !x.is_finite()
            || !y.is_finite()
            || !width.is_finite()
            || !height.is_finite()
            || width <= 0.0
            || height <= 0.0
            || frame_width <= 0.0
            || frame_height <= 0.0
        {
            return None;
        }
        let rect = Self {
            x: (x / frame_width).clamp(0.0, 1.0),
            y: (y / frame_height).clamp(0.0, 1.0),
            width: (width / frame_width).clamp(0.0, 1.0),
            height: (height / frame_height).clamp(0.0, 1.0),
        };
        (rect.width > 0.0 && rect.height > 0.0).then_some(rect)
    }

    fn source_point(self, x: f64, y: f64) -> Option<(f64, f64)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let source_x = (x - self.x) / self.width;
        let source_y = (y - self.y) / self.height;
        ((0.0..=1.0).contains(&source_x) && (0.0..=1.0).contains(&source_y))
            .then_some((source_x, source_y))
    }
}

struct CaptureFrameState {
    ring: Arc<SharedFrameRing>,
    content_rect: RwLock<NormalizedContentRect>,
    failed: AtomicBool,
}

impl CaptureFrameState {
    fn receive(&self, sample: screencapturekit::cm::CMSampleBuffer) {
        let Some(pixel_buffer) = sample.image_buffer() else {
            return;
        };
        if pixel_buffer.pixel_format() != BGRA_PIXEL_FORMAT
            || pixel_buffer.width() != self.ring.layout.width
            || pixel_buffer.height() != self.ring.layout.height
        {
            self.failed.store(true, Ordering::Release);
            return;
        }
        if let Some(info) = sample.frame_info() {
            if let Some(rect) = info.content_rect {
                // ScreenCaptureKit reports this rectangle in source points.
                // Convert it into configured output pixels before normalizing.
                let output_scale =
                    info.scale_factor.unwrap_or(1.0) * info.content_scale.unwrap_or(1.0);
                let normalized = NormalizedContentRect::from_frame_rect(
                    rect.origin.x * output_scale,
                    rect.origin.y * output_scale,
                    rect.size.width * output_scale,
                    rect.size.height * output_scale,
                    pixel_buffer.width(),
                    pixel_buffer.height(),
                );
                if let Some(normalized) = normalized {
                    *self
                        .content_rect
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = normalized;
                }
            }
        }
        let Ok(guard) = pixel_buffer.lock_read_only() else {
            self.failed.store(true, Ordering::Release);
            return;
        };
        if self
            .ring
            .publish(guard.as_slice(), guard.bytes_per_row())
            .is_err()
        {
            self.failed.store(true, Ordering::Release);
        }
    }
}

struct ApplicationSurfaceSession {
    target_window_id: u32,
    target_process_id: i32,
    stream: SCStream,
    frame_state: Arc<CaptureFrameState>,
    pointer_state: Arc<Mutex<ApplicationSurfacePointerState>>,
}

impl Drop for ApplicationSurfaceSession {
    fn drop(&mut self) {
        stop_stream(self.stream.clone());
    }
}

#[derive(Default)]
struct ApplicationSurfaceManager {
    sessions: HashMap<String, ApplicationSurfaceSession>,
}

#[derive(Default)]
struct ApplicationSurfacePointerState {
    left_click_group_id: Option<i64>,
    right_click_group_id: Option<i64>,
}

impl ApplicationSurfacePointerState {
    fn group_for(&mut self, kind: &str) -> i64 {
        match kind {
            "left_mouse_down" => {
                let group = next_click_group_id();
                self.left_click_group_id = Some(group);
                group
            }
            "left_mouse_dragged" => *self
                .left_click_group_id
                .get_or_insert_with(next_click_group_id),
            "left_mouse_up" => self
                .left_click_group_id
                .take()
                .unwrap_or_else(next_click_group_id),
            "right_mouse_down" => {
                let group = next_click_group_id();
                self.right_click_group_id = Some(group);
                group
            }
            "right_mouse_dragged" => *self
                .right_click_group_id
                .get_or_insert_with(next_click_group_id),
            "right_mouse_up" => self
                .right_click_group_id
                .take()
                .unwrap_or_else(next_click_group_id),
            "mouse_moved" => self
                .left_click_group_id
                .or(self.right_click_group_id)
                .unwrap_or_else(next_click_group_id),
            _ => next_click_group_id(),
        }
    }
}

fn next_click_group_id() -> i64 {
    static NEXT_CLICK_GROUP_ID: AtomicU64 = AtomicU64::new(1);
    (NEXT_CLICK_GROUP_ID.fetch_add(1, Ordering::Relaxed) & i64::MAX as u64) as i64
}

fn manager() -> &'static Mutex<ApplicationSurfaceManager> {
    static MANAGER: OnceLock<Mutex<ApplicationSurfaceManager>> = OnceLock::new();
    MANAGER.get_or_init(|| Mutex::new(ApplicationSurfaceManager::default()))
}

/// Enumerate shareable windows, bounded by
/// [`capture_deadline::SHAREABLE_CONTENT_BUDGET`].
fn shareable_windows() -> anyhow::Result<SCShareableContent> {
    capture_deadline::run(
        CaptureOperation::ShareableContent,
        capture_deadline::SHAREABLE_CONTENT_BUDGET,
        || {
            SCShareableContent::create()
                .with_on_screen_windows_only(false)
                .with_exclude_desktop_windows(true)
                .get()
                .map_err(|error| anyhow!("could not enumerate application windows: {error}"))
        },
    )
}

/// Stop a stream outside the caller, bounded by
/// [`capture_deadline::STREAM_STOP_BUDGET`].
fn stop_stream(stream: SCStream) {
    let result = capture_deadline::run(
        CaptureOperation::StreamStop,
        capture_deadline::STREAM_STOP_BUDGET,
        move || stream.stop_capture().map_err(|error| anyhow!("{error}")),
    );
    if let Err(error) = result {
        tracing::warn!(%error, "application surface capture did not stop cleanly");
    }
}

pub fn list_windows() -> anyhow::Result<Vec<ApplicationWindow>> {
    require_application_surface_permissions()?;
    let content = shareable_windows()?;
    let helper_pid = std::process::id() as i32;
    let mut windows = content
        .windows()
        .into_iter()
        .filter_map(|window| {
            let owner = window.owning_application()?;
            let frame = window.frame();
            if window.window_id() == 0
                || owner.process_id() == helper_pid
                || window.window_layer() != 0
                || frame.size.width < 64.0
                || frame.size.height < 64.0
            {
                return None;
            }
            let owner_name = owner.application_name();
            let title = window
                .title()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| owner_name.clone());
            Some(ApplicationWindow {
                window_id: window.window_id(),
                process_id: owner.process_id(),
                owner: owner_name,
                title,
                width: frame.size.width,
                height: frame.size.height,
            })
        })
        .collect::<Vec<_>>();
    windows.sort_by(|left, right| {
        left.owner
            .to_lowercase()
            .cmp(&right.owner.to_lowercase())
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
            .then_with(|| left.window_id.cmp(&right.window_id))
    });
    Ok(windows)
}

pub fn start(
    request: ApplicationSurfaceStartRequest,
) -> anyhow::Result<ApplicationSurfaceStartResult> {
    require_application_surface_permissions()?;
    if !(1..=120).contains(&request.frame_rate) || request.window_id == 0 || request.process_id <= 0
    {
        bail!("invalid application-surface target or frame rate");
    }

    let content = shareable_windows()?;
    let source_window = content
        .windows()
        .into_iter()
        .find(|window| {
            window.window_id() == request.window_id
                && window
                    .owning_application()
                    .is_some_and(|owner| owner.process_id() == request.process_id)
        })
        .ok_or_else(|| anyhow!("application_window_unavailable"))?;
    let source_frame = source_window.frame();
    let (width, height) = capture_pixel_size(source_frame.size.width, source_frame.size.height)?;
    let ring = SharedFrameRing::create(width, height)?;
    let frame_state = Arc::new(CaptureFrameState {
        ring: ring.clone(),
        content_rect: RwLock::new(NormalizedContentRect::default()),
        failed: AtomicBool::new(false),
    });

    let filter = SCContentFilter::create()
        .with_window(&source_window)
        .build();
    let interval = screencapturekit::cm::CMTime::new(1, request.frame_rate as i32);
    let configuration = SCStreamConfiguration::new()
        .with_width(width as u32)
        .with_height(height as u32)
        .with_minimum_frame_interval(&interval)
        .with_queue_depth(3)
        // The host already presents the local pointer over the pane. Capturing
        // the target window's synthetic pointer would render a second cursor.
        .with_shows_cursor(false)
        .with_pixel_format(PixelFormat::BGRA)
        .with_scales_to_fit(true)
        .with_preserves_aspect_ratio(true);
    let callback_state = frame_state.clone();
    let error_state = frame_state.clone();
    let delegate = StreamCallbacks::new().on_error(move |_| {
        error_state.failed.store(true, Ordering::Release);
    });
    let mut stream = SCStream::new_with_delegate(&filter, &configuration, delegate);
    stream
        .add_output_handler(
            move |sample, output_type| {
                if output_type == SCStreamOutputType::Screen {
                    callback_state.receive(sample);
                }
            },
            SCStreamOutputType::Screen,
        )
        .ok_or_else(|| anyhow!("could not attach application capture output"))?;
    // A start that answers after its deadline leaves a running stream nobody
    // owns; the late handler stops it.
    let stream = capture_deadline::bounded(
        CaptureOperation::StreamStart,
        capture_deadline::STREAM_START_BUDGET,
        &SystemCaptureClock,
        move || stream.start_capture().map(|()| stream),
        |late| {
            if let Ok(stream) = late {
                let _ = stream.stop_capture();
            }
        },
    )?
    .map_err(|error| anyhow!("could not start application capture: {error}"))?;

    let session_id = Uuid::new_v4().to_string();
    let result = ApplicationSurfaceStartResult {
        session_id: session_id.clone(),
        frame_transport: ring.descriptor(),
    };
    let mut manager = manager()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    manager.sessions.insert(
        session_id,
        ApplicationSurfaceSession {
            target_window_id: request.window_id,
            target_process_id: request.process_id,
            stream,
            frame_state,
            pointer_state: Arc::new(Mutex::new(ApplicationSurfacePointerState::default())),
        },
    );
    Ok(result)
}

fn require_application_surface_permissions() -> anyhow::Result<()> {
    let status = permissions::current_status();
    if !status.accessibility {
        bail!("accessibility_permission_required");
    }
    if !status.screen_recording {
        bail!("screen_recording_permission_required");
    }
    Ok(())
}

fn capture_pixel_size(width: f64, height: f64) -> anyhow::Result<(usize, usize)> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        bail!("application window has invalid dimensions");
    }
    let scale = 2.0_f64
        .min(4096.0 / width.max(1.0))
        .min(2304.0 / height.max(1.0));
    let pixel_width = (width * scale).round().max(1.0) as usize;
    let pixel_height = (height * scale).round().max(1.0) as usize;
    FrameLayout::new(pixel_width, pixel_height)?;
    Ok((pixel_width, pixel_height))
}

pub fn stop(session_id: &str) -> bool {
    let session = manager()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .sessions
        .remove(session_id);
    drop(session);
    true
}

pub fn stop_all() {
    let sessions = {
        let mut manager = manager()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut manager.sessions)
    };
    drop(sessions);
}

pub fn send_event(event: ApplicationSurfaceEvent) -> anyhow::Result<()> {
    if !permissions::current_status().accessibility {
        bail!("accessibility_permission_required");
    }
    let (target_window_id, target_process_id, frame_state, pointer_state) = {
        let manager = manager()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = manager
            .sessions
            .get(&event.session)
            .ok_or_else(|| anyhow!("application_surface_session_unavailable"))?;
        (
            session.target_window_id,
            session.target_process_id,
            session.frame_state.clone(),
            session.pointer_state.clone(),
        )
    };
    if frame_state.failed.load(Ordering::Acquire) {
        bail!("application_surface_capture_failed");
    }
    let target = live_target(target_window_id, target_process_id)
        .ok_or_else(|| anyhow!("application_window_unavailable"))?;
    match event.kind.as_str() {
        "mouse_moved"
        | "left_mouse_down"
        | "left_mouse_up"
        | "left_mouse_dragged"
        | "right_mouse_down"
        | "right_mouse_up"
        | "right_mouse_dragged" => {
            let content = *frame_state
                .content_rect
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (source_x, source_y) = content
                .source_point(event.x, event.y)
                .ok_or_else(|| anyhow!("application_surface_point_outside_content"))?;
            let screen_x = target.bounds.x + source_x * target.bounds.width;
            let screen_y = target.bounds.y + source_y * target.bounds.height;
            post_mouse(
                target_process_id,
                target_window_id,
                event.kind.as_str(),
                screen_x,
                screen_y,
                source_x * target.bounds.width,
                source_y * target.bounds.height,
                event.modifiers,
                event.click_count,
                &pointer_state,
            )
        }
        "scroll" => {
            let content = *frame_state
                .content_rect
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (source_x, source_y) = content
                .source_point(event.x, event.y)
                .ok_or_else(|| anyhow!("application_surface_point_outside_content"))?;
            post_scroll(
                target_process_id,
                target_window_id,
                target.bounds.x + source_x * target.bounds.width,
                target.bounds.y + source_y * target.bounds.height,
                source_x * target.bounds.width,
                source_y * target.bounds.height,
                event.delta_x,
                event.delta_y,
                event.modifiers,
                &pointer_state,
            )
        }
        "key" => post_key(
            target_process_id,
            event.key_code,
            event.key_down,
            event.modifiers,
        ),
        _ => bail!("unsupported application-surface event"),
    }
}

fn live_target(window_id: u32, process_id: i32) -> Option<windows::WindowInfo> {
    windows::all_windows().into_iter().find(|window| {
        window.window_id == window_id
            && window.pid == process_id
            && window.bounds.width > 0.0
            && window.bounds.height > 0.0
    })
}

#[allow(clippy::too_many_arguments)]
fn post_mouse(
    process_id: i32,
    window_id: u32,
    kind: &str,
    screen_x: f64,
    screen_y: f64,
    local_x: f64,
    local_y: f64,
    modifiers: u64,
    click_count: i64,
    pointer_state: &Mutex<ApplicationSurfacePointerState>,
) -> anyhow::Result<()> {
    let (event_type, button, button_number) = match kind {
        "mouse_moved" => (CGEventType::MouseMoved, CGMouseButton::Left, 0),
        "left_mouse_down" => (CGEventType::LeftMouseDown, CGMouseButton::Left, 0),
        "left_mouse_up" => (CGEventType::LeftMouseUp, CGMouseButton::Left, 0),
        "left_mouse_dragged" => (CGEventType::LeftMouseDragged, CGMouseButton::Left, 0),
        "right_mouse_down" => (CGEventType::RightMouseDown, CGMouseButton::Right, 1),
        "right_mouse_up" => (CGEventType::RightMouseUp, CGMouseButton::Right, 1),
        "right_mouse_dragged" => (CGEventType::RightMouseDragged, CGMouseButton::Right, 1),
        _ => bail!("unsupported mouse event"),
    };
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow!("could not create mouse event source"))?;
    let point = CGPoint::new(screen_x, screen_y);
    let click_group_id = pointer_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .group_for(kind);
    if matches!(kind, "left_mouse_down" | "right_mouse_down") {
        crate::input::mouse::post_mouse_moved_primer(
            process_id,
            &source,
            point,
            Some((local_x, local_y)),
            Some(window_id),
            Some(click_group_id),
        );
        std::thread::sleep(std::time::Duration::from_millis(12));
    }
    let event = CGEvent::new_mouse_event(source, event_type, point, button)
        .map_err(|_| anyhow!("could not create mouse event"))?;
    event.set_flags(CGEventFlags::from_bits_truncate(modifiers));
    let click_state = if kind == "mouse_moved" {
        0
    } else {
        click_count.clamp(1, 3)
    };
    let subtype = if matches!(kind, "left_mouse_dragged" | "right_mouse_dragged") {
        0
    } else {
        3
    };
    crate::input::mouse::post_mouse_event(
        process_id,
        &event,
        Some((local_x, local_y)),
        Some(window_id),
        Some(click_group_id),
        click_state,
        button_number,
        subtype,
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn post_scroll(
    process_id: i32,
    window_id: u32,
    screen_x: f64,
    screen_y: f64,
    local_x: f64,
    local_y: f64,
    delta_x: f64,
    delta_y: f64,
    modifiers: u64,
    pointer_state: &Mutex<ApplicationSurfacePointerState>,
) -> anyhow::Result<()> {
    let wheel_x = delta_x.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
    let wheel_y = delta_y.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
    if wheel_x == 0 && wheel_y == 0 {
        return Ok(());
    }

    post_mouse(
        process_id,
        window_id,
        "mouse_moved",
        screen_x,
        screen_y,
        local_x,
        local_y,
        modifiers,
        1,
        pointer_state,
    )?;

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow!("could not create scroll event source"))?;
    let event = CGEvent::new_scroll_event(source, ScrollEventUnit::PIXEL, 2, wheel_y, wheel_x, 0)
        .map_err(|_| anyhow!("could not create scroll event"))?;
    event.set_flags(CGEventFlags::from_bits_truncate(modifiers));
    let pointer = event.as_ptr() as *mut c_void;
    unsafe { CGEventSetLocation(pointer, screen_x, screen_y) };
    crate::input::skylight::set_window_location(pointer, local_x, local_y);
    crate::input::skylight::set_integer_field(pointer, 40, process_id as i64);
    crate::input::skylight::set_integer_field(pointer, 51, window_id as i64);
    crate::input::skylight::set_integer_field(pointer, 91, window_id as i64);
    crate::input::skylight::set_integer_field(pointer, 92, window_id as i64);
    crate::input::skylight::post_to_pid(process_id as libc::pid_t, pointer, false);
    event.post_to_pid(process_id as libc::pid_t);
    Ok(())
}

fn post_key(process_id: i32, key_code: u16, key_down: bool, modifiers: u64) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow!("could not create keyboard event source"))?;
    let event = CGEvent::new_keyboard_event(source, key_code, key_down)
        .map_err(|_| anyhow!("could not create keyboard event"))?;
    event.set_flags(CGEventFlags::from_bits_truncate(modifiers));
    let pointer = event.as_ptr() as *mut c_void;
    if !crate::input::skylight::post_to_pid(process_id as libc::pid_t, pointer, true) {
        event.post_to_pid(process_id as libc::pid_t);
    }
    Ok(())
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSetLocation(event: *mut c_void, x: f64, y: f64);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_layout_matches_brigadier_triple_ring_protocol() {
        let layout = FrameLayout::new(100, 50).unwrap();
        assert_eq!(layout.bytes_per_row, 400);
        assert_eq!(layout.slot_byte_count, 20_000);
        assert_eq!(layout.total_byte_count % 4096, 0);
        assert_eq!(layout.slot_offset(0), Some(64));
        assert_eq!(layout.slot_offset(2), Some(40_064));
        assert_eq!(layout.slot_offset(3), None);
    }

    #[test]
    fn frame_ring_can_be_created_with_macos_shm_open() {
        let ring = SharedFrameRing::create(2, 2).unwrap();
        let descriptor = ring.descriptor();
        assert_eq!(descriptor.width, 2);
        assert_eq!(descriptor.height, 2);
        assert_eq!(descriptor.slot_count, FRAME_SLOT_COUNT);

        let name = CString::new(descriptor.shared_memory_name).unwrap();
        let descriptor_handle = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
        assert!(descriptor_handle >= 0);
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        assert_eq!(
            unsafe { libc::fstat(descriptor_handle, metadata.as_mut_ptr()) },
            0
        );
        unsafe {
            libc::close(descriptor_handle);
        }
        let metadata = unsafe { metadata.assume_init() };
        assert_eq!(metadata.st_mode & 0o777, 0o600);
    }

    #[test]
    fn content_rect_rejects_letterbox_and_maps_live_content() {
        let rect =
            NormalizedContentRect::from_frame_rect(0.0, 250.0, 1000.0, 500.0, 1000, 1000).unwrap();
        assert_eq!(rect.source_point(0.5, 0.25), Some((0.5, 0.0)));
        assert_eq!(rect.source_point(0.5, 0.75), Some((0.5, 1.0)));
        assert_eq!(rect.source_point(0.5, 0.1), None);
    }

    #[test]
    fn capture_size_is_bounded_without_changing_aspect_ratio() {
        assert_eq!(capture_pixel_size(800.0, 600.0).unwrap(), (1600, 1200));
        assert_eq!(capture_pixel_size(5000.0, 2500.0).unwrap(), (4096, 2048));
        assert!(capture_pixel_size(0.0, 600.0).is_err());
    }

    #[test]
    fn pointer_click_group_survives_a_gesture_and_clears_on_release() {
        let mut state = ApplicationSurfacePointerState::default();
        let down = state.group_for("left_mouse_down");
        assert_eq!(state.group_for("left_mouse_dragged"), down);
        assert_eq!(state.group_for("mouse_moved"), down);
        assert_eq!(state.group_for("left_mouse_up"), down);
        assert!(state.left_click_group_id.is_none());

        let next_down = state.group_for("left_mouse_down");
        assert_ne!(next_down, down);
    }
}

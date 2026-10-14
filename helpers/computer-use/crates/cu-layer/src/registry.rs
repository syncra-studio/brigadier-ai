//! The one way the helper builds its tool registry: the driver's macOS
//! tools, Brigadier's own tools in place of the driver's, and the guard
//! around every tool, in that order. Every entry point of the helper (the
//! daemon, the in-process server and the one-shot CLI) uses [`build`], so
//! the order is tested once.

use std::sync::Arc;

use cua_driver_core::tool::ToolRegistry;

/// What differs between the helper's entry points.
#[derive(Default)]
pub struct Options {
    /// The agent cursor's settings. One-shot commands pass
    /// `enabled: false`.
    pub cursor: cursor_overlay::CursorConfig,
    /// The driver's compatibility mode for agents that expect a window
    /// screenshot tool.
    pub compat: bool,
    /// Bundle id of the app that hosts the helper, if any.
    pub host_bundle_id: Option<String>,
}

/// Build the registry. Call [`crate::configure_environment`] first.
///
/// Installs the focus policy (once per process), the recording hooks and
/// Brigadier's cursor motion as the saved default, and leaves out the key
/// edit after a text insert (an insert into a document's text already marks
/// it edited, and the key edit would make ⌘Z undo only itself), then
/// builds the driver's tools under the driver's own runtime scope, so
/// element tokens taken in one call resolve in the next. Brigadier's
/// `page`, `scroll`, `hotkey`, `type_text`, `get_window_state` and
/// `launch_app`, and the Finder gestures that open a file
/// ([`crate::finder_open`]), go in before the guard, which wraps every tool
/// last. A session's recording stops when the
/// session ends.
pub fn build(options: Options) -> Result<Arc<ToolRegistry>, String> {
    crate::focus_policy::install();
    crate::offspace::install();
    install_recording_hooks();
    crate::cursor_motion::seed_default();
    platform_macos::document_edit::set_key_edit_after_insert(false);
    let scope = cua_driver_core::session_authorization::configured_registry()?
        .legacy_context()?
        .runtime_scope_key();
    let registry = cua_driver_core::tool::with_runtime_scope(scope, || {
        let mut registry = platform_macos::register_tools_with_cursor(
            options.cursor,
            options.compat,
            // Brigadier shows its own permission window; the driver never
            // prompts.
            true,
            options.host_bundle_id,
        );
        crate::page::install(&mut registry);
        crate::scroll::install(&mut registry);
        crate::hotkey::install(&mut registry);
        crate::web_typing::install(&mut registry);
        crate::keep_drawn::install(&mut registry);
        crate::launch::install(&mut registry);
        crate::finder_open::install(&mut registry);
        stop_recording_when_session_ends(&mut registry);
        crate::guard::install(&mut registry);
        registry
    });
    let registry = Arc::new(registry);
    registry.init_self_weak();
    crate::finder_open::set_registry(&registry);
    Ok(registry)
}

fn stop_recording_when_session_ends(registry: &mut ToolRegistry) {
    let recording = Arc::downgrade(&registry.recording);
    let hook = cua_driver_core::session::register_scoped_fallible_session_end_hook(
        "recording",
        move |session| match recording.upgrade() {
            Some(recording) => recording
                .stop_owner(Some(session))
                .map_err(|error| error.to_string()),
            None => Ok(()),
        },
    );
    registry.retain_session_end_hook(hook);
}

/// The screenshot, click-marker, accessibility-snapshot, element-bounds and
/// video hooks recordings use. Later calls change nothing. A test process
/// ([`crate::no_tcc`]) installs none of them, so the activity journal's
/// frame after an action, which captures through the screenshot hook, never
/// makes macOS ask it for screen capture.
pub fn install_recording_hooks() {
    use cua_driver_core::recording;
    if crate::no_tcc::off() {
        return;
    }
    recording::set_screenshot_fn(|window_id, pid| {
        if let Some(window_id) = window_id {
            platform_macos::capture::screenshot_window_bytes(window_id as u32).ok()
        } else if let Some(pid) = pid {
            platform_macos::windows::resolve_main_window_id(pid as i32)
                .ok()
                .and_then(|window_id| {
                    platform_macos::capture::screenshot_window_bytes(window_id).ok()
                })
        } else {
            platform_macos::capture::screenshot_display_bytes().ok()
        }
    });
    recording::set_click_marker_fn(|png, x, y| {
        platform_macos::capture::crosshair_png_bytes(png, x, y).ok()
    });
    recording::set_budgeted_ax_snapshot_fn(|window_id, pid, budget| {
        platform_macos::recording_hooks::app_state_json_for(window_id, pid, budget)
    });
    recording::set_element_bounds_fn(|pid, args, capture_point| {
        platform_macos::recording_hooks::element_window_local_xy(pid, args, capture_point)
    });
    cua_driver_core::video::set_video_backend_factory(Box::new(
        platform_macos::video_sckit::SckitVideoBackendFactory,
    ));
}

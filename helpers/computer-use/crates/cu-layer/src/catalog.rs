//! Which driver tools agents get. Every tool the driver registers is listed
//! here as offered or withheld; a tool a newer driver adds is withheld until
//! it is reviewed and added (the `every_driver_tool_is_classified` test
//! fails until then).

/// Tools agents may call.
pub const OFFERED: &[&str] = &[
    "list_apps",
    "list_windows",
    "get_window_state",
    "verify_state",
    "launch_app",
    "kill_app",
    "invoke_menu",
    "click",
    "double_click",
    "right_click",
    "type_text",
    "press_key",
    "hotkey",
    "set_value",
    "scroll",
    "get_screen_size",
    "get_desktop_state",
    "get_cursor_position",
    "move_cursor",
    "set_agent_cursor_enabled",
    "set_agent_cursor_motion",
    "set_agent_cursor_theme",
    "get_agent_cursor_state",
    "check_permissions",
    "health_report",
    "get_config",
    "set_config",
    "get_accessibility_tree",
    "zoom",
    "page",
    "start_recording",
    "stop_recording",
    "get_recording_state",
    "replay_trajectory",
    "start_session",
    "get_session",
    "list_sessions",
    "get_session_state",
    "end_session",
    "run_steps",
];

/// Tools agents never get, with the reason they are told if they call one.
pub const WITHHELD: &[(&str, &str)] = &[
    (
        "bring_to_front",
        "it brings an app to the front, which would interrupt the user",
    ),
    (
        "drag",
        "dragging needs the app in front and moves the user's pointer",
    ),
    (
        "set_window_frame",
        "it moves and resizes the user's windows",
    ),
    ("clipboard_read", "the clipboard belongs to the user"),
    ("clipboard_write", "the clipboard belongs to the user"),
    (
        "get_browser_state",
        "use the page tool to read and act on a web page",
    ),
    (
        "browser_prepare",
        "use the page tool to read and act on a web page",
    ),
    (
        "browser_navigate",
        "use launch_app with urls, or the page tool, to open a web page",
    ),
    (
        "browser_click",
        "use the page tool or click to act on a web page",
    ),
    (
        "browser_type",
        "use the page tool or type_text to type into a web page",
    ),
    ("browser_dialog", "use the page tool to act on a web page"),
    (
        "browser_set_input_files",
        "use the page tool to act on a web page",
    ),
    ("browser_download", "use the page tool to act on a web page"),
    (
        "browser_pointer",
        "use the page tool or click to act on a web page",
    ),
    ("install_ffmpeg", "it installs software on the user's Mac"),
    ("run_script", "scripts are turned off in Brigadier"),
    (
        "escalate_session",
        "computer use in Brigadier never asks for more access during a session",
    ),
];

/// Whether agents may call `tool`.
pub fn offered(tool: &str) -> bool {
    OFFERED.contains(&tool)
}

/// The reason `tool` is withheld; unknown tools are withheld too.
pub fn withheld_reason(tool: &str) -> &'static str {
    WITHHELD
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, why)| *why)
        .unwrap_or("it is not available in Brigadier")
}

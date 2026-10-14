//! The helper's own test processes never ask about or use accessibility or
//! screen capture.
//!
//! Every build of such a process is a new unsigned identity, and macOS adds
//! an entry to its privacy lists for each identity that asks about or uses
//! either, so a process started with [`NO_TCC_FOR_TESTS_ENV`] set to `1`
//! runs only the tools in [`TCC_FREE_TOOLS`]. The guard refuses the others
//! before they run, for a single call and for each `run_steps` step.

use cua_driver_core::protocol::ToolResult;

/// Set (to `1`) by the helper's own tests on the processes they start.
pub const NO_TCC_FOR_TESTS_ENV: &str = "BRIGADIER_CU_NO_TCC_FOR_TESTS";

/// Tools that need neither accessibility nor screen capture.
pub const TCC_FREE_TOOLS: &[&str] = &[
    "get_screen_size",
    "get_cursor_position",
    "get_config",
    "set_config",
    "list_apps",
    "move_cursor",
    "get_agent_cursor_state",
    "set_agent_cursor_enabled",
    "set_agent_cursor_motion",
    "set_agent_cursor_theme",
    // Recording captures only through the screenshot and video paths, which
    // a test process skips; a replay runs each step through this list again.
    "start_recording",
    "stop_recording",
    "get_recording_state",
    "replay_trajectory",
];

/// True in a process started with [`NO_TCC_FOR_TESTS_ENV`].
pub fn off() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var(NO_TCC_FOR_TESTS_ENV).is_ok_and(|value| value == "1"))
}

/// The refusal for `tool` in such a process, if it isn't TCC-free.
pub fn refusal(tool: &str) -> Option<ToolResult> {
    refusal_when(off(), tool)
}

fn refusal_when(off: bool, tool: &str) -> Option<ToolResult> {
    (off && !TCC_FREE_TOOLS.contains(&tool)).then(|| {
        ToolResult::error(format!(
            "{tool} needs accessibility or screen capture, which this test process never uses."
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_process_runs_only_tcc_free_tools() {
        assert!(refusal_when(true, "click").is_some());
        assert!(refusal_when(true, "get_window_state").is_some());
        assert!(refusal_when(true, "get_config").is_none());
        assert!(refusal_when(true, "set_agent_cursor_theme").is_none());
        assert!(refusal_when(false, "click").is_none());
    }

    #[test]
    fn every_tcc_free_tool_is_offered() {
        for tool in TCC_FREE_TOOLS {
            assert!(crate::catalog::offered(tool), "{tool}");
        }
    }
}

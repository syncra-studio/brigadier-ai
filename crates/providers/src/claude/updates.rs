//! What Claude Code's settings allow its updater to install.

use std::path::PathBuf;

use serde_json::Value;

use crate::adapters::UpdateLimits;
use crate::cli::CliEnv;

/// Claude Code's release channel and limits, as `claude update` reads them: managed settings
/// first, then the user's (Brigadier runs the update from the home folder, outside any
/// project's settings).
pub(super) fn limits(env: &CliEnv) -> UpdateLimits {
    let read = |path: PathBuf| -> Value {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    };
    let managed = managed_settings().map(read).unwrap_or_default();
    let user = match env.var("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => read(PathBuf::from(dir).join("settings.json")),
        None => env
            .home()
            .map(|home| read(home.join(".claude").join("settings.json")))
            .unwrap_or_default(),
    };
    limits_from(
        &managed,
        &user,
        env.var("DISABLE_UPDATES").and_then(|v| v.to_str()),
    )
}

/// [`limits`] from the managed and user settings and the `DISABLE_UPDATES` variable.
fn limits_from(managed: &Value, user: &Value, disable_updates: Option<&str>) -> UpdateLimits {
    let channel = [managed, user]
        .iter()
        .find_map(|settings| settings.get("autoUpdatesChannel").and_then(Value::as_str))
        .map_or("latest", |channel| match channel {
            "stable" => "stable",
            _ => "latest",
        });
    let truthy = |value: Option<&str>| {
        value.is_some_and(|value| !value.is_empty() && value != "0" && value != "false")
    };
    let updates_off = truthy(disable_updates)
        || [managed, user].iter().any(|settings| {
            truthy(
                settings
                    .get("env")
                    .and_then(|env| env.get("DISABLE_UPDATES"))
                    .and_then(Value::as_str),
            )
        });
    UpdateLimits {
        channel,
        maximum: managed
            .get("requiredMaximumVersion")
            .and_then(Value::as_str)
            .map(str::to_owned),
        updates_off,
    }
}

fn managed_settings() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some("/Library/Application Support/ClaudeCode/managed-settings.json".into())
    } else if cfg!(windows) {
        Some(r"C:\Program Files\ClaudeCode\managed-settings.json".into())
    } else if cfg!(unix) {
        Some("/etc/claude-code/managed-settings.json".into())
    } else {
        None
    }
}

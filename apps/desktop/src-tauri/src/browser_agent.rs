//! The browser tool's commands (Q74): what the app does on a Browser tab's page for an agent.
//! The app's own code (`state/agentBrowser.ts`) takes the agent's call from the daemon, runs it
//! with these and answers it. Reading and acting go through `browser_agent.js`, run fresh in
//! the page for each call; a screenshot is WebKit's picture of the page.
//!
//! macOS only for now: Windows' pages can't be read or pictured yet, and Linux has no
//! embedded page.

use brigadier_ipc::protocol::IpcError;
#[cfg(target_os = "macos")]
use std::sync::Mutex;

/// The script, as a function of one op (see its head).
const AGENT: &str = include_str!("browser_agent.js");

/// What runs in the page for `op`: the agent script called with it.
fn script(op: &serde_json::Value) -> String {
    format!("({})({op})", AGENT.trim().trim_end_matches(';'))
}

#[cfg(not(target_os = "macos"))]
fn unsupported() -> IpcError {
    IpcError {
        code: brigadier_ipc::protocol::ErrorCode::Invalid,
        message: "agents can't use the built-in browser on this platform yet".into(),
    }
}

/// Reads tab `id`'s page or acts on it as `op` says, and returns what the page answered.
#[tauri::command]
pub async fn browser_agent(
    app: tauri::AppHandle,
    id: String,
    op: serde_json::Value,
) -> Result<serde_json::Value, IpcError> {
    #[cfg(target_os = "macos")]
    {
        let script = script(&op);
        let json = crate::browser::on_page(&app, id, move |webview, sent| {
            let sent = Mutex::new(Some(sent));
            let _ = webview.evaluate_script_with_callback(&script, move |json| {
                if let Some(sent) = sent
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                {
                    let _ = sent.send(json);
                }
            });
        })
        .await?;
        serde_json::from_str(&json).map_err(|err| IpcError {
            code: brigadier_ipc::protocol::ErrorCode::Internal,
            message: format!("the page didn't answer: {err}"),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, id, script(&op));
        Err(unsupported())
    }
}

/// Opens `url` in tab `id`'s page where it is, without moving or showing it.
#[tauri::command]
pub fn browser_agent_load(id: String, url: String) -> Result<(), IpcError> {
    crate::browser::check_enabled()?;
    crate::browser::web_page_only(&url)?;
    #[cfg(target_os = "macos")]
    {
        crate::browser::embedded::load(&id, &url)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = id;
        Err(unsupported())
    }
}

/// A PNG of tab `id`'s page as it shows.
#[tauri::command]
pub async fn browser_agent_capture(
    app: tauri::AppHandle,
    id: String,
) -> Result<tauri::ipc::Response, IpcError> {
    #[cfg(target_os = "macos")]
    {
        let png = crate::browser::on_page(&app, id, |webview, sent| {
            crate::browser_ui::snapshot_png(webview, move |png| {
                let _ = sent.send(png);
            });
        })
        .await?;
        png.map(tauri::ipc::Response::new).ok_or_else(|| IpcError {
            code: brigadier_ipc::protocol::ErrorCode::Internal,
            message: "the page's picture couldn't be taken".into(),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, id);
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_is_called_with_its_op() {
        let op = serde_json::json!({ "kind": "read", "max": 100 });
        let script = script(&op);
        assert!(
            script.starts_with("(// The browser tool's hand"),
            "{}",
            &script[..60]
        );
        assert!(
            script.ends_with(r#"})({"kind":"read","max":100})"#),
            "{}",
            &script[script.len() - 60..]
        );
    }
}

//! The nightly's latest retained CI artifact, read through the user's existing GitHub CLI.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;

const REPO: &str = "syncra-studio/brigadier-ai";
static NEXT: AtomicU64 = AtomicU64::new(0);

async fn gh(args: &[&str]) -> Result<Vec<u8>, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(45),
        tokio::process::Command::new("gh")
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "Reading the nightly artifact timed out. Try Refresh.".to_owned())?
    .map_err(|err| format!("GitHub CLI could not read the nightly artifact: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "GitHub CLI could not read the nightly artifact: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

fn latest_run(value: &Value) -> Option<u64> {
    value
        .get("artifacts")?
        .as_array()?
        .iter()
        .filter(|artifact| {
            artifact["name"] == "continuity-battery"
                && artifact["expired"] == false
                && artifact["workflow_run"]["head_branch"] == "main"
        })
        .max_by_key(|artifact| artifact["id"].as_u64().unwrap_or(0))?["workflow_run"]["id"]
        .as_u64()
}

struct Download(PathBuf);
impl Drop for Download {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// No background polling or account setup: Reports reads when opened or refreshed.
#[tauri::command]
pub async fn get_continuity_report() -> Result<Value, String> {
    let bytes = gh(&[
        "api",
        &format!("repos/{REPO}/actions/artifacts?name=continuity-battery&per_page=100"),
    ])
    .await?;
    let listing: Value = serde_json::from_slice(&bytes).map_err(|err| err.to_string())?;
    let run = latest_run(&listing).ok_or(
        "No retained continuity artifact yet. Run the Continuity battery workflow on main.",
    )?;
    let folder = std::env::temp_dir().join(format!(
        "brigadier-continuity-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&folder).map_err(|err| err.to_string())?;
    let download = Download(folder);
    gh(&[
        "run",
        "download",
        &run.to_string(),
        "--repo",
        REPO,
        "--name",
        "continuity-battery",
        "--dir",
        &download.0.to_string_lossy(),
    ])
    .await?;
    let file = download.0.join("summary.json");
    if std::fs::metadata(&file)
        .map_err(|err| err.to_string())?
        .len()
        > 2_000_000
    {
        return Err("The nightly summary is too large to display.".into());
    }
    let summary: Value =
        serde_json::from_slice(&std::fs::read(file).map_err(|err| err.to_string())?)
            .map_err(|err| err.to_string())?;
    if summary["schema_version"] != 1 || !summary["providers"].is_array() {
        return Err("This nightly summary uses an unsupported format.".into());
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn newest_retained_main_artifact_even_when_an_arm_skipped() {
        let artifact = |id, branch, expired| {
            json!({"id":id,"name":"continuity-battery","expired":expired,
            "workflow_run":{"id":id,"head_branch":branch}})
        };
        assert_eq!(
            latest_run(
                &json!({"artifacts":[artifact(3,"main",false),artifact(8,"main",true),
            artifact(10,"topic",false),artifact(5,"main",false)]})
            ),
            Some(5)
        );
        assert_eq!(latest_run(&json!({"artifacts":[]})), None);
    }
}

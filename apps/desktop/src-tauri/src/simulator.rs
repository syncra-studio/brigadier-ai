//! Open on an iOS simulator preview's card: the Simulator app comes to the front, on the
//! preview's device when it is booted (#433). Only on the user's click.

use brigadier_ipc::protocol::{ErrorCode, IpcError};

/// The booted device to show, from `xcrun simctl list devices booted -j`: the one named
/// `device`, else the first booted one.
pub fn booted_device(listing: &str, device: Option<&str>) -> Option<String> {
    let listing: serde_json::Value = serde_json::from_str(listing).ok()?;
    let booted: Vec<(&str, &str)> = listing
        .get("devices")?
        .as_object()?
        .values()
        .filter_map(|runtime| runtime.as_array())
        .flatten()
        .filter(|each| each.get("state").and_then(|state| state.as_str()) == Some("Booted"))
        .filter_map(|each| Some((each.get("name")?.as_str()?, each.get("udid")?.as_str()?)))
        .collect();
    booted
        .iter()
        .find(|(name, _)| Some(*name) == device)
        .or(booted.first())
        .map(|(_, udid)| (*udid).to_owned())
}

/// Brings Simulator to the front, opening it on the booted device named `device` (a preview's
/// target) or on any booted one.
#[tauri::command]
pub async fn open_simulator(device: Option<String>) -> Result<(), IpcError> {
    let failed = |message: String| IpcError {
        code: ErrorCode::Internal,
        message,
    };
    if !cfg!(target_os = "macos") {
        return Err(IpcError {
            code: ErrorCode::Invalid,
            message: "the iOS Simulator runs on a Mac only".to_owned(),
        });
    }
    tauri::async_runtime::spawn_blocking(move || {
        let listing = std::process::Command::new("xcrun")
            .args(["simctl", "list", "devices", "booted", "-j"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned());
        let udid = listing.and_then(|listing| booted_device(&listing, device.as_deref()));
        let mut open = std::process::Command::new("open");
        open.args(["-a", "Simulator"]);
        if let Some(udid) = udid {
            open.args(["--args", "-CurrentDeviceUDID", &udid]);
        }
        match open.status() {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(failed(format!("could not open Simulator ({status})"))),
            Err(err) => Err(failed(format!("could not open Simulator: {err}"))),
        }
    })
    .await
    .map_err(|err| failed(format!("could not open Simulator: {err}")))?
}

#[cfg(test)]
mod tests {
    use super::booted_device;

    const LISTING: &str = r#"{"devices":{
        "com.apple.CoreSimulator.SimRuntime.iOS-26-0":[
            {"name":"iPhone 16","udid":"A1","state":"Shutdown"},
            {"name":"iPhone 17","udid":"B2","state":"Booted"}],
        "com.apple.CoreSimulator.SimRuntime.iOS-27-0":[
            {"name":"iPad Air","udid":"C3","state":"Booted"}]}}"#;

    /// The preview's device when it is booted, else the first booted one; none booted, none.
    #[test]
    fn the_preview_s_booted_device_is_shown() {
        assert_eq!(
            booted_device(LISTING, Some("iPad Air")).as_deref(),
            Some("C3")
        );
        assert_eq!(
            booted_device(LISTING, Some("iPhone 16")).as_deref(),
            Some("B2")
        );
        assert_eq!(booted_device(LISTING, None).as_deref(), Some("B2"));
        assert_eq!(booted_device(r#"{"devices":{}}"#, None), None);
        assert_eq!(booted_device("not json", None), None);
    }
}

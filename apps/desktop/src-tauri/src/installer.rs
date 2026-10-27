//! The Windows installer's hooks (`installer-hooks.nsh`), checked against the configuration that
//! decides what the installer writes.

use std::collections::BTreeSet;

/// Every file the Windows installer writes beside the app other than the app itself: its
/// sidecars, as `tauri.windows.conf.json` lists them, and the uninstaller.
fn shipped_beside_the_app() -> BTreeSet<String> {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.windows.conf.json")).unwrap();
    let sidecars = config["bundle"]["externalBin"].as_array().unwrap();
    sidecars
        .iter()
        .map(|path| {
            let name = path.as_str().unwrap().rsplit('/').next().unwrap();
            format!("{name}.exe")
        })
        .chain(["uninstall.exe".to_owned()])
        .collect()
}

/// The names the preinstall hook keeps the old-main-binary cleanup from deleting.
fn kept_by_the_hook() -> BTreeSet<String> {
    include_str!("../installer-hooks.nsh")
        .lines()
        .filter_map(|line| line.split_once("$R9 == \"")?.1.split_once('"'))
        .map(|(name, _)| name.to_owned())
        .collect()
}

/// An update over a build whose app had another name deletes that name after copying the new
/// files. Had it been any file the installer now writes (brigadier.exe was the app before it
/// was brigadier-app.exe, and is now the command), the update would delete it: the hook must
/// keep every one, so a new sidecar has to join it.
#[test]
fn an_update_never_deletes_a_file_the_installer_ships() {
    assert_eq!(kept_by_the_hook(), shipped_beside_the_app());
}

#[test]
fn the_installer_runs_the_hooks() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.windows.conf.json")).unwrap();
    assert_eq!(
        config.pointer("/bundle/windows/nsis/installerHooks"),
        Some(&serde_json::json!("installer-hooks.nsh"))
    );
}

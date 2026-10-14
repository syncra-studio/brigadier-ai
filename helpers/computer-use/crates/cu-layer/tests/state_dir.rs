//! Every driver setting lives in Brigadier's helper home, never in a folder
//! of the driver's own under the user's home. Its own test binary, because
//! it sets this process's environment.
#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cua_driver_core::protocol::Content;
use serde_json::json;

fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap_or_default());
            }
        }
    }
    out
}

#[tokio::test]
async fn the_driver_reads_and_writes_only_brigadiers_home() {
    let base = std::env::temp_dir().join(format!("cu-trycua-state-{}", std::process::id()));
    let user_home = base.join("home");
    let helper_home = base.join("helper");
    let sentinel = user_home.join(".cua-driver");
    std::fs::create_dir_all(&sentinel).unwrap();
    std::fs::create_dir_all(&helper_home).unwrap();
    std::fs::write(
        sentinel.join("config.json"),
        r#"{"max_image_dimension": 111, "experimental_pip": true}"#,
    )
    .unwrap();
    std::fs::write(
        helper_home.join("config.json"),
        r#"{"max_image_dimension": 1234, "brigadier_key": "kept"}"#,
    )
    .unwrap();
    let before = tree(&user_home);

    std::env::set_var("HOME", &user_home);
    cu_layer::configure_environment(&helper_home);
    let mut registry = platform_macos::register_tools();
    cu_layer::guard::install_with(&mut registry, Arc::new(cu_layer::lookup::System));
    let registry = Arc::new(registry);
    registry.init_self_weak();

    let config = registry.invoke("get_config", json!({})).await;
    let structured = config.structured_content.clone().unwrap_or_default();
    let text: String = config
        .content
        .iter()
        .filter_map(|item| match item {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        structured.to_string().contains("1234") || text.contains("1234"),
        "get_config shows the helper's config: {structured} {text}"
    );

    let set = registry
        .invoke("set_config", json!({"experimental_pip": false}))
        .await;
    assert_ne!(set.is_error, Some(true), "{:?}", set.content);
    let written = std::fs::read_to_string(helper_home.join("config.json")).unwrap();
    assert!(written.contains("experimental_pip"), "{written}");
    assert!(
        written.contains("brigadier_key"),
        "other keys are kept: {written}"
    );

    assert_eq!(
        tree(&user_home),
        before,
        "nothing changed under the user's home"
    );
    std::fs::remove_dir_all(&base).unwrap();
}

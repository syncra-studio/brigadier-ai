use super::*;
use crate::manager::flow::{Flow, Options, Reply};
use std::sync::Arc;

async fn fixture(name: &str) -> Flow {
    Flow::start(
        name,
        Options::default(),
        Arc::new(|_| Box::pin(async { Reply::text("ok") })),
    )
    .await
}
fn server(manager: &SessionManager, name: &str) {
    let folder = manager.data_dir.join("library/servers");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(format!("{name}.json")), serde_json::json!({"mcpServers":{name:{"type":"http","url":format!("https://{name}.test/mcp"),"headers":{"Authorization":"fixture-secret"}}}}).to_string()).unwrap();
}

#[tokio::test]
async fn page_snapshot_survives_restart_without_credentials_and_refreshes_in_background() {
    let flow = fixture("plugins-page-snapshot").await;
    server(&flow.manager, "first");
    let initial = flow.manager.cached_plugin_catalog().await.unwrap();
    assert!(initial.rows.iter().any(|row| row.name == "first"));
    let bytes = std::fs::read(flow.manager.data_dir.join(FILE)).unwrap();
    assert!(!String::from_utf8(bytes).unwrap().contains("fixture-secret"));
    let restored = Cache::load(&flow.manager.data_dir);
    assert_eq!(
        restored
            .state
            .lock()
            .unwrap()
            .catalog
            .as_ref()
            .unwrap()
            .rows,
        initial.rows
    );
    server(&flow.manager, "second");
    flow.manager.invalidate_plugin_catalog();
    let stale = flow.manager.cached_plugin_catalog().await.unwrap();
    assert_eq!(
        stale.rows, initial.rows,
        "cached rows return while the new catalog builds"
    );
    flow.manager.rebuild_plugin_catalog().await.unwrap();
    assert!(
        flow.manager
            .cached_plugin_catalog()
            .await
            .unwrap()
            .rows
            .iter()
            .any(|row| row.name == "second")
    );
    flow.stop().await;
}

#[tokio::test]
async fn choices_and_sign_in_status_update_the_memory_and_disk_snapshot() {
    let flow = fixture("plugins-snapshot-choices").await;
    server(&flow.manager, "choice");
    let view = flow.manager.cached_plugin_catalog().await.unwrap();
    let row = view.rows.iter().find(|row| row.name == "choice").unwrap();
    flow.manager.set_plugin_choice(&row.id, false).unwrap();
    assert!(
        !flow
            .manager
            .cached_plugin_catalog()
            .await
            .unwrap()
            .rows
            .iter()
            .find(|item| item.id == row.id)
            .unwrap()
            .choice
            .on
    );
    flow.manager
        .cache_plugin_sign_in(&row.id, CatalogSignIn::SignedIn);
    let restored = Cache::load(&flow.manager.data_dir);
    {
        let state = restored.state.lock().unwrap();
        let saved = state
            .catalog
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .find(|item| item.id == row.id)
            .unwrap();
        assert!(!saved.choice.on);
        assert_eq!(saved.sign_in, CatalogSignIn::SignedIn);
    }
    flow.manager.cache_plugin_sign_in(
        &row.id,
        CatalogSignIn::SigningIn {
            flow_id: "transient-flow".into(),
        },
    );
    assert!(
        !std::fs::read_to_string(flow.manager.data_dir.join(FILE))
            .unwrap()
            .contains("transient-flow")
    );
    flow.stop().await;
}

#[tokio::test]
async fn corrupt_snapshot_recovers_and_concurrent_cold_reads_share_a_refresh() {
    let flow = fixture("plugins-corrupt-snapshot").await;
    server(&flow.manager, "recover");
    std::fs::write(flow.manager.data_dir.join(FILE), "broken").unwrap();
    let (one, two) = tokio::join!(
        flow.manager.cached_plugin_catalog(),
        flow.manager.cached_plugin_catalog()
    );
    assert_eq!(one.unwrap(), two.unwrap());
    assert!(
        Cache::load(&flow.manager.data_dir)
            .state
            .lock()
            .unwrap()
            .catalog
            .is_some()
    );
    flow.stop().await;
}

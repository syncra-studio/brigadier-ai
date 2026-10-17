//! The routing page's review line per author: it shows each vendor's change going to the other
//! vendor only while both vendors can take work.

use std::sync::Arc;

use brigadier_providers::ProviderKind;
use brigadier_router::TaskCategory;

use super::{Flow, Options, Reply, Script, Turn};

fn answering() -> Script {
    Arc::new(|_turn: Turn| Box::pin(async { Reply::text("Noted.") }))
}

async fn review_picks(flow: &Flow) -> usize {
    flow.manager
        .preview_routes(None, Vec::new(), None)
        .await
        .into_iter()
        .find(|preview| preview.category == TaskCategory::Review)
        .expect("a review preview")
        .by_author
        .len()
}

#[tokio::test]
async fn review_picks_per_author_need_two_vendors() {
    let flow = Flow::start("review-picks", Options::default(), answering()).await;
    assert_eq!(review_picks(&flow).await, 2);

    let mut settings = flow.core.settings();
    settings.disabled_providers.push(ProviderKind::Codex);
    flow.core.update_settings(settings).await.unwrap();
    assert_eq!(review_picks(&flow).await, 0, "Codex is off");

    let mut settings = flow.core.settings();
    settings.disabled_providers.clear();
    flow.core.update_settings(settings).await.unwrap();
    flow.behavior
        .signed_out
        .lock()
        .unwrap()
        .push(ProviderKind::Codex);
    flow.manager
        .runtime
        .refresh_providers(Some(ProviderKind::Codex));
    flow.until("Codex is checked as signed out", |_| {
        !flow.manager.runtime.provider_signed_in(ProviderKind::Codex)
    })
    .await;
    assert_eq!(review_picks(&flow).await, 0, "Codex is signed out");
    flow.stop().await;
}

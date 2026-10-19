//! Kyumei's investigation workspace, served with Topcoat.
//!
//! This crate is the only one that knows about Topcoat. It renders the
//! application layer's workspace model and turns form posts into application
//! commands; replacing or upgrading Topcoat touches nothing else.

pub mod components;
pub mod pages;
pub mod panels;
pub mod preferences;
pub mod views;

use std::sync::Arc;

use kyumei_app::InvestigationService;
use kyumei_knowledge::KnowledgeBase;
use topcoat::router::{BodyLimit, Router};

pub use pages::AppState;

/// Uploads may carry exports and screenshots.
pub const MAX_UPLOAD_BYTES: usize = 16 * 1024 * 1024;

pub fn router(
    kb: Arc<KnowledgeBase>,
    service: Arc<dyn InvestigationService>,
    mode: String,
) -> Router {
    router_for(AppState::new(kb, service, mode))
}

/// The workspace over `state`, which can carry a collector for Collect now.
pub fn router_for(state: AppState) -> Router {
    let state = Arc::new(state);
    Router::builder()
        .layout(pages::app_layout)
        .layer(
            BodyLimit::max(MAX_UPLOAD_BYTES).at("/investigations/{investigation}/evidence/upload"),
        )
        .layer(BodyLimit::max(MAX_UPLOAD_BYTES).at("/investigations/{investigation}/evidence"))
        .layer(BodyLimit::max(MAX_UPLOAD_BYTES).at("/dashboards"))
        .page(pages::landing)
        .page(pages::workspace_page)
        .page(pages::evidence_page)
        .page(pages::knowledge_index)
        .page(pages::claim_page)
        .page(pages::dashboards_page)
        .page(panels::panel_page)
        .page(panels::investigation_panel_page)
        .route(pages::create)
        .route(pages::amend_context)
        .route(pages::add_evidence)
        .route(pages::upload_evidence)
        .route(pages::decline)
        .route(pages::collect_now)
        .route(preferences::set_zone)
        .route(panels::add_panel_evidence)
        .route(pages::upload_dashboard)
        .route(pages::correct)
        .route(pages::raw_evidence)
        .route(pages::export)
        .route(pages::events)
        .route(pages::stylesheet)
        .route(pages::script)
        .route(pages::health)
        .app_context(state)
        .build()
}

/// Serve until Ctrl-C.
pub async fn serve(listener: tokio::net::TcpListener, router: Router) -> std::io::Result<()> {
    topcoat::serve(listener, router).await
}

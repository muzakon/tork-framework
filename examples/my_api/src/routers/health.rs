//! Health-check route.

use std::sync::Arc;

use tork::{api_router, get};

use crate::core::app_state::{Config, UserStore};
use crate::models::health::HealthOut;

#[api_router(tags = ["health"])]
pub mod health_router {
    use super::*;

    /// Liveness check. Demonstrates injecting resources directly into a handler.
    #[get("/health", response_model = HealthOut, summary = "Health check")]
    pub async fn health(store: UserStore, config: Arc<Config>) -> tork::Result<HealthOut> {
        // Touch the store to confirm the resource is live.
        let _ = store.get_user(1)?;
        Ok(HealthOut {
            status: "ok".to_owned(),
            service: config.app_name.clone(),
        })
    }
}

/// Builds the health router.
pub fn router() -> tork::Router {
    health_router::router()
}

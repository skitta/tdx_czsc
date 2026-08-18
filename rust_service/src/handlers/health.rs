use anyhow::Result;

use crate::proto::analysis::{HealthRequest, HealthResponse, ServiceState};
use crate::state::AppState;

pub async fn handle(state: &std::sync::Arc<AppState>, _request: HealthRequest) -> Result<HealthResponse> {
    let cache_entries = state.cache.read().await.len() as u32;

    Ok(HealthResponse {
        state: ServiceState::ServiceReady as i32,
        message: "tdx_czsc_service is ready".to_string(),
        uptime_ms: state.started_at.elapsed().as_millis() as u64,
        active_requests: state.active_requests(),
        cache_entries,
        service_version: state.config.service_version.clone(),
    })
}

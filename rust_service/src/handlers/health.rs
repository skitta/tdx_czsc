use super::Handlers;
use crate::proto::analysis::{HealthResponse, ServiceState};

impl Handlers {
    /// 健康检查：服务状态、运行时长、活跃请求与缓存条目数。
    pub async fn health(&self) -> HealthResponse {
        let state = self.state();
        let cache_entries = state.cache().read().await.len() as u32;

        HealthResponse {
            state: ServiceState::ServiceReady as i32,
            message: "tdx_czsc_service is ready".to_string(),
            uptime_ms: state.uptime_ms(),
            active_requests: state.active_requests(),
            cache_entries,
            service_version: state.config().service_version.clone(),
        }
    }
}

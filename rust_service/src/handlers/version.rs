use anyhow::Result;

use crate::proto::analysis::{VersionRequest, VersionResponse};
use crate::state::AppState;

pub async fn handle(state: &std::sync::Arc<AppState>, _request: VersionRequest) -> Result<VersionResponse> {
    Ok(VersionResponse {
        service_version: state.config.service_version.clone(),
        protocol_version: state.config.protocol_version.clone(),
        czsc_version: state.config.czsc_version.clone(),
        supported_func_groups: vec![
            "single_level_structure".to_string(),
            "single_level_signals".to_string(),
        ],
        // func_id 列表以 analyzer 的单一映射表为准（含 304 边界通道）
        supported_func_ids: crate::analyzer::SUPPORTED_FUNC_IDS.to_vec(),
    })
}

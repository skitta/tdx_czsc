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
        supported_func_ids: vec![101, 102, 201, 202, 301, 302, 303, 401, 402, 403, 404, 405, 406, 901, 902, 903],
    })
}

use super::Handlers;
use crate::analyzer::ChannelBuilder;
use crate::proto::analysis::VersionResponse;

impl Handlers {
    /// 版本与支持能力查询（能力清单以 analyzer 的映射表为准）。
    pub async fn version(&self) -> VersionResponse {
        let state = self.state();
        VersionResponse {
            service_version: state.config().service_version.clone(),
            protocol_version: state.config().protocol_version.clone(),
            czsc_version: state.config().czsc_version.clone(),
            supported_func_groups: vec![
                "single_level_structure".to_string(),
                "single_level_signals".to_string(),
            ],
            supported_func_ids: ChannelBuilder::specs()
                .iter()
                .map(|spec| spec.func_id())
                .collect(),
        }
    }
}

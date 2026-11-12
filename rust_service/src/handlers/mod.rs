//! 请求信封分发与协议状态码映射。
//!
//! [`Handlers`] 是 IPC 层的统一入口：按请求体分派到各业务 handler，并把错误
//! 映射为协议状态码（错误来自分析管线时按 `AnalysisError` 分类，其余按内部
//! 错误处理），始终返回响应信封（不向上抛错）。

use std::sync::Arc;

use crate::analyzer::AnalysisError;
use crate::config::AppConfig;
use crate::proto::analysis::request_envelope::Body as RequestBody;
use crate::proto::analysis::response_envelope::Body as ResponseBody;
use crate::proto::analysis::{
    AnalyzeResponse, HealthResponse, RequestEnvelope, ResponseEnvelope, StatusCode, VersionResponse,
};
use crate::state::AppState;
use tracing::warn;

mod analyze;
mod debug;
mod health;
mod version;

/// 业务 handler 集合：持有服务状态，按请求类型分发。
pub struct Handlers {
    state: Arc<AppState>,
}

impl Handlers {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// 分发请求信封并生成响应信封。
    ///
    /// 唯一调用方是 `#[cfg(windows)]` 的 IPC 层，非 Windows 构建下无调用点；
    /// 因此保持 `pub` 以免 `dead_code` 误报（同 crate 内无其他入口）。
    pub async fn dispatch(&self, request: RequestEnvelope) -> ResponseEnvelope {
        let envelope = Envelope::new(self.state.config(), request.request_id);
        match request.body {
            Some(body) => match self.route(body).await {
                Ok(response) => envelope.ok(response),
                Err(err) => envelope.fail(&err),
            },
            None => {
                let err =
                    anyhow::Error::new(AnalysisError::invalid_request("request body is missing"));
                envelope.fail(&err)
            }
        }
    }

    /// 按请求体分派到具体业务。
    async fn route(&self, body: RequestBody) -> anyhow::Result<ResponseBody> {
        Ok(match body {
            RequestBody::Analyze(request) => ResponseBody::from(self.analyze(request).await?),
            RequestBody::Health(_) => ResponseBody::from(self.health().await),
            RequestBody::Version(_) => ResponseBody::from(self.version().await),
        })
    }

    /// 服务状态（子模块共享）。
    pub(crate) fn state(&self) -> &Arc<AppState> {
        &self.state
    }
}

impl From<AnalyzeResponse> for ResponseBody {
    fn from(value: AnalyzeResponse) -> Self {
        Self::Analyze(value)
    }
}

impl From<HealthResponse> for ResponseBody {
    fn from(value: HealthResponse) -> Self {
        Self::Health(value)
    }
}

impl From<VersionResponse> for ResponseBody {
    fn from(value: VersionResponse) -> Self {
        Self::Version(value)
    }
}

/// 响应信封构造器：成功/失败两条路径的统一出口。
struct Envelope<'a> {
    config: &'a AppConfig,
    request_id: u64,
}

impl<'a> Envelope<'a> {
    fn new(config: &'a AppConfig, request_id: u64) -> Self {
        Self { config, request_id }
    }

    fn ok<B: Into<ResponseBody>>(self, body: B) -> ResponseEnvelope {
        ResponseEnvelope {
            protocol_version: self.config.protocol_version.clone(),
            request_id: self.request_id,
            status: StatusCode::StatusOk as i32,
            error_message: String::new(),
            body: Some(body.into()),
        }
    }

    fn fail(self, err: &anyhow::Error) -> ResponseEnvelope {
        let status = ErrorStatus::from(err);
        if status == ErrorStatus(StatusCode::StatusInternalError) {
            warn!(error = %err, "request failed with internal error");
        }
        ResponseEnvelope {
            protocol_version: self.config.protocol_version.clone(),
            request_id: self.request_id,
            status: status.code(),
            error_message: err.to_string(),
            body: None,
        }
    }
}

/// 错误 → 协议状态码：
/// - 请求/校验类（`InvalidRequest`、`UnsupportedFreq`）→ `STATUS_INVALID_REQUEST`；
/// - 数据分析类（`InvalidBarData`）→ `STATUS_ANALYZE_FAILED`；
/// - 其余（任务/运行时异常）→ `STATUS_INTERNAL_ERROR`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ErrorStatus(StatusCode);

impl ErrorStatus {
    fn code(self) -> i32 {
        self.0 as i32
    }
}

impl From<&anyhow::Error> for ErrorStatus {
    fn from(err: &anyhow::Error) -> Self {
        match err.downcast_ref::<AnalysisError>() {
            Some(AnalysisError::InvalidRequest(_) | AnalysisError::UnsupportedFreq(_)) => {
                Self(StatusCode::StatusInvalidRequest)
            }
            Some(AnalysisError::InvalidBarData { .. }) => Self(StatusCode::StatusAnalyzeFailed),
            Some(AnalysisError::Internal(_)) => Self(StatusCode::StatusInternalError),
            None => Self(StatusCode::StatusInternalError),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::proto::analysis::{AnalyzeRequest, MarketSeries};
    use crate::state::AppState;

    async fn request_with_series(series: MarketSeries) -> ResponseEnvelope {
        let request = RequestEnvelope {
            protocol_version: "1.0.0".to_string(),
            request_id: 42,
            body: Some(RequestBody::Analyze(AnalyzeRequest {
                func_group: "single_level_structure".to_string(),
                requested_func_ids: vec![101],
                freq_hint: "D".to_string(),
                data_len: series.close.len() as u32,
                input_hash: 0,
                series: Some(series),
                options: Some(crate::proto::analysis::CzscOptions {
                    max_bi_num: 50,
                    min_bi_len: 6,
                    analysis_window: 100,
                    include_objects: false,
                    enable_incremental: false,
                }),
                prefer_cached: false,
            })),
        };
        let state = AppState::new(AppConfig::default());
        Handlers::new(state).dispatch(request).await
    }

    /// 序列与 data_len 不匹配 → STATUS_INVALID_REQUEST。
    #[tokio::test]
    async fn mismatched_series_maps_to_invalid_request() {
        let series = MarketSeries {
            open: Vec::new(),
            high: vec![1.0, 2.0],
            low: vec![1.0],
            close: vec![1.0, 2.0],
            vol: Vec::new(),
            amount: Vec::new(),
            dt_unix_ms: Vec::new(),
        };
        let resp = request_with_series(series).await;
        assert_eq!(resp.status, StatusCode::StatusInvalidRequest as i32);
        assert!(resp.body.is_none());
        assert!(resp.error_message.contains("series low length"));
    }

    /// 正常请求 → STATUS_OK 且带响应体。
    #[tokio::test]
    async fn valid_request_returns_ok() {
        let series = MarketSeries {
            open: Vec::new(),
            high: vec![11.0, 12.0, 11.5, 12.5],
            low: vec![10.0, 11.0, 10.5, 11.5],
            close: vec![10.5, 11.5, 11.0, 12.0],
            vol: Vec::new(),
            amount: Vec::new(),
            dt_unix_ms: Vec::new(),
        };
        let resp = request_with_series(series).await;
        assert_eq!(resp.status, StatusCode::StatusOk as i32);
        assert!(resp.request_id == 42);
        assert!(resp.body.is_some());
    }

    /// 缺请求体 → STATUS_INVALID_REQUEST。
    #[tokio::test]
    async fn missing_body_maps_to_invalid_request() {
        let request = RequestEnvelope {
            protocol_version: "1.0.0".to_string(),
            request_id: 1,
            body: None,
        };
        let state = AppState::new(AppConfig::default());
        let resp = Handlers::new(state).dispatch(request).await;
        assert_eq!(resp.status, StatusCode::StatusInvalidRequest as i32);
        assert!(resp.error_message.contains("body is missing"));
    }

    /// 错误分类 → 协议状态码映射（含非分析错误）。
    #[test]
    fn error_status_maps_analysis_errors() {
        let invalid = anyhow::Error::new(AnalysisError::invalid_request("bad series"));
        assert_eq!(
            ErrorStatus::from(&invalid).code(),
            StatusCode::StatusInvalidRequest as i32
        );

        let freq = anyhow::Error::new(AnalysisError::UnsupportedFreq("W9".to_string()));
        assert_eq!(
            ErrorStatus::from(&freq).code(),
            StatusCode::StatusInvalidRequest as i32
        );

        let bar = anyhow::Error::new(AnalysisError::InvalidBarData {
            index: 3,
            message: "bad timestamp".to_string(),
        });
        assert_eq!(
            ErrorStatus::from(&bar).code(),
            StatusCode::StatusAnalyzeFailed as i32
        );

        let runtime = anyhow::anyhow!("analysis task join failed: cancelled");
        assert_eq!(
            ErrorStatus::from(&runtime).code(),
            StatusCode::StatusInternalError as i32
        );

        let internal = anyhow::Error::new(AnalysisError::internal("clock before epoch"));
        assert_eq!(
            ErrorStatus::from(&internal).code(),
            StatusCode::StatusInternalError as i32
        );
    }
}

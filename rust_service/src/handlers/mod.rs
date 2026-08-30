//! 请求信封分发与协议状态码映射。
//!
//! `handle_request` 是 IPC 层的统一入口：按请求体分派到各业务 handler，
//! 并把错误映射为协议状态码（错误来自分析管线时按 `AnalysisError` 分类，
//! 其余按内部错误处理），始终返回响应信封（不再向上抛错）。

use crate::analyzer::AnalysisError;
use crate::proto::analysis::request_envelope::Body as RequestBody;
use crate::proto::analysis::response_envelope::Body as ResponseBody;
use crate::proto::analysis::{RequestEnvelope, ResponseEnvelope, StatusCode};
use crate::state::AppState;

pub mod analyze;
mod debug;
pub mod health;
pub mod version;

/// 分发请求信封并生成响应信封。
pub async fn handle_request(
    state: &std::sync::Arc<AppState>,
    request: RequestEnvelope,
) -> ResponseEnvelope {
    let protocol_version = state.config.protocol_version.clone();
    let request_id = request.request_id;

    let result = match request.body {
        Some(RequestBody::Analyze(req)) => {
            analyze::handle(state, req).await.map(ResponseBody::Analyze)
        }
        Some(RequestBody::Health(req)) => {
            health::handle(state, req).await.map(ResponseBody::Health)
        }
        Some(RequestBody::Version(req)) => {
            version::handle(state, req).await.map(ResponseBody::Version)
        }
        None => Err(anyhow::Error::new(AnalysisError::invalid_request(
            "request body is missing",
        ))),
    };

    match result {
        Ok(response_body) => ResponseEnvelope {
            protocol_version,
            request_id,
            status: StatusCode::StatusOk as i32,
            error_message: String::new(),
            body: Some(response_body),
        },
        Err(err) => build_error_response(state, request_id, error_status(&err), err.to_string()),
    }
}

/// 把错误映射为协议状态码：
/// - 请求/校验类（`InvalidRequest`、`UnsupportedFreq`）→ `STATUS_INVALID_REQUEST`；
/// - 数据分析类（`InvalidBarData`）→ `STATUS_ANALYZE_FAILED`；
/// - 其余（任务/运行时异常）→ `STATUS_INTERNAL_ERROR`。
fn error_status(err: &anyhow::Error) -> StatusCode {
    match err.downcast_ref::<AnalysisError>() {
        Some(AnalysisError::InvalidRequest(_) | AnalysisError::UnsupportedFreq(_)) => {
            StatusCode::StatusInvalidRequest
        }
        Some(AnalysisError::InvalidBarData { .. }) => StatusCode::StatusAnalyzeFailed,
        None => StatusCode::StatusInternalError,
    }
}

pub fn build_error_response(
    state: &AppState,
    request_id: u64,
    status: StatusCode,
    error_message: impl Into<String>,
) -> ResponseEnvelope {
    ResponseEnvelope {
        protocol_version: state.config.protocol_version.clone(),
        request_id,
        status: status as i32,
        error_message: error_message.into(),
        body: None,
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
        handle_request(&state, request).await
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
        let resp = handle_request(&state, request).await;
        assert_eq!(resp.status, StatusCode::StatusInvalidRequest as i32);
        assert!(resp.error_message.contains("body is missing"));
    }
}

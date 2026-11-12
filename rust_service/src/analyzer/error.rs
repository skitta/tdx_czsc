//! 分析领域错误（`thiserror` 类型化）。
//!
//! 分层的约定：分析管线（analyzer 各模块）返回 `AnalysisError`，`handlers`
//! 协议层用 `anyhow` 携带上下文并把类型化错误映射为协议状态码：
//! - 请求/校验类（`InvalidRequest`、`UnsupportedFreq`）→ `STATUS_INVALID_REQUEST`；
//! - 数据分析类（`InvalidBarData`）→ `STATUS_ANALYZE_FAILED`；
//! - 内部异常（`Internal`）→ `STATUS_INTERNAL_ERROR`。
//!
//! `AnalysisError` 为 `Send + Sync + 'static`，可被
//! `anyhow::Error::downcast_ref` 从任意上下文链中取回用于状态码映射。

use thiserror::Error;

/// 分析请求/数据错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnalysisError {
    /// 请求校验失败：序列缺失、长度不齐、分析窗口为空、不支持的 func_id 等。
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// 不支持/无法解析的周期提示（`freq_hint`）。
    #[error("unsupported freq_hint: {0}")]
    UnsupportedFreq(String),

    /// 单根 K 线构造失败（非法时间戳、czsc RawBarBuilder 校验失败等）。
    #[error("failed to build bar at index {index}: {message}")]
    InvalidBarData { index: usize, message: String },

    /// 服务内部错误（时钟异常、不可恢复的运行时状态等）。
    #[error("internal analysis error: {0}")]
    Internal(String),
}

impl AnalysisError {
    pub(crate) fn invalid_request(message: impl Into<String>) -> Self {
        AnalysisError::InvalidRequest(message.into())
    }

    pub(crate) fn invalid_bar_data(index: usize, message: impl Into<String>) -> Self {
        AnalysisError::InvalidBarData {
            index,
            message: message.into(),
        }
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        AnalysisError::Internal(message.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_is_send_sync_and_displayable() {
        fn assert_traits<T: Send + Sync + 'static + std::error::Error>() {}
        assert_traits::<AnalysisError>();

        let err = AnalysisError::invalid_request("series high is empty");
        assert_eq!(err.to_string(), "invalid request: series high is empty");
    }

    #[test]
    fn downcast_through_anyhow_context() {
        let any: anyhow::Error =
            anyhow::anyhow!(AnalysisError::UnsupportedFreq("X9".into())).context("analysis failed");
        assert_eq!(
            any.downcast_ref::<AnalysisError>(),
            Some(&AnalysisError::UnsupportedFreq("X9".into()))
        );
    }
}

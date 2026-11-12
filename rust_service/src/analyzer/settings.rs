//! 分析参数、窗口与请求校验。
//!
//! 这一层把“请求选项优先、配置兜底”的解析规则集中到 [`AnalysisSettings`]，
//! 让后续的 RawBar、演化、装配阶段只消费已经校验过的设置。

use czsc_core::objects::freq::Freq;

use crate::config::AppConfig;
use crate::proto::analysis::AnalyzeRequest;

use super::error::AnalysisError;
use super::rawbars::{FreqHint, SeriesShape};

/// 分析参数：全量分析唯一的运行时输入。
///
/// 这是配置侧默认值；请求选项由 `AnalysisSettings` 统一覆盖，避免
/// handler 与 analyzer 各自解析一遍。
#[derive(Debug, Clone)]
pub struct AnalysisParams {
    /// 分析窗口（bar 数）
    pub(crate) analysis_window: u32,
    pub(crate) max_bi_num: u32,
    pub(crate) min_bi_len: u32,
    /// vol 缺失时是否派生振幅代理量（见 rawbars 量能来源链）
    pub(crate) derive_volume_proxy: bool,
    /// 分析器版本标识（写入快照 meta）
    pub(crate) czsc_version: String,
}

impl From<&AppConfig> for AnalysisParams {
    fn from(config: &AppConfig) -> Self {
        Self {
            analysis_window: config.default_analysis_window,
            max_bi_num: config.max_bi_num,
            min_bi_len: config.min_bi_len,
            derive_volume_proxy: config.derive_volume_proxy,
            czsc_version: config.czsc_version.clone(),
        }
    }
}

/// 分析窗口：`0` 表示全量，其他值表示尾部 N 根。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisWindow(u32);

impl AnalysisWindow {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub(crate) const fn bars(self) -> u32 {
        self.0
    }

    /// 窗口起点：全量从 0 开始，限窗取尾部 N 根（不足时取 0）。
    pub fn start(self, data_len: usize) -> usize {
        let window = self.0 as usize;
        if window == 0 {
            0
        } else {
            data_len.saturating_sub(window)
        }
    }
}

/// 请求选项解析后的有效参数（不含序列/频率校验，可安全用于缓存键）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolvedParams {
    pub(crate) window: AnalysisWindow,
    pub(crate) max_bi_num: u32,
    pub(crate) min_bi_len: u32,
}

impl ResolvedParams {
    pub(crate) fn from_request(request: &AnalyzeRequest, params: &AnalysisParams) -> Self {
        let options = request.options.as_ref();
        Self {
            window: AnalysisWindow::new(
                options
                    .map(|x| x.analysis_window)
                    .filter(|x| *x > 0)
                    .unwrap_or(params.analysis_window),
            ),
            max_bi_num: options
                .map(|x| x.max_bi_num)
                .filter(|x| *x > 0)
                .unwrap_or(params.max_bi_num),
            min_bi_len: options
                .map(|x| x.min_bi_len)
                .filter(|x| *x > 0)
                .unwrap_or(params.min_bi_len),
        }
    }
}

/// 单次请求的已校验设置。
pub(crate) struct AnalysisSettings<'a> {
    request: &'a AnalyzeRequest,
    params: &'a AnalysisParams,
    freq: Freq,
    analysis_window: AnalysisWindow,
    window_start: usize,
    max_bi_num: u32,
    min_bi_len: u32,
    wants_signals: bool,
    include_objects: bool,
}

impl<'a> AnalysisSettings<'a> {
    /// 解析顺序与旧实现保持一致：序列校验 → 请求选项 → 频率 → 窗口起点。
    pub(crate) fn new(
        request: &'a AnalyzeRequest,
        params: &'a AnalysisParams,
    ) -> Result<Self, AnalysisError> {
        let data_len = request.data_len as usize;
        let series = request
            .series
            .as_ref()
            .ok_or_else(|| AnalysisError::invalid_request("market series is missing"))?;
        series.validate_lengths(request.data_len)?;

        let resolved = ResolvedParams::from_request(request, params);
        let freq = FreqHint::new(&request.freq_hint).parse()?;
        let window_start = resolved.window.start(data_len);
        let wants_signals = request
            .requested_func_ids
            .iter()
            .any(|id| (401..=406).contains(id));
        let include_objects = request.options.as_ref().is_some_and(|x| x.include_objects);

        Ok(Self {
            request,
            params,
            freq,
            analysis_window: resolved.window,
            window_start,
            max_bi_num: resolved.max_bi_num,
            min_bi_len: resolved.min_bi_len,
            wants_signals,
            include_objects,
        })
    }

    pub(crate) fn request(&self) -> &'a AnalyzeRequest {
        self.request
    }

    pub(crate) fn params(&self) -> &'a AnalysisParams {
        self.params
    }

    pub(crate) fn freq(&self) -> Freq {
        self.freq
    }

    pub(crate) fn window_start(&self) -> usize {
        self.window_start
    }

    pub(crate) fn analysis_window(&self) -> AnalysisWindow {
        self.analysis_window
    }

    pub(crate) fn max_bi_num(&self) -> u32 {
        self.max_bi_num
    }

    pub(crate) fn min_bi_len(&self) -> u32 {
        self.min_bi_len
    }

    pub(crate) fn wants_signals(&self) -> bool {
        self.wants_signals
    }

    pub(crate) fn include_objects(&self) -> bool {
        self.include_objects
    }

    pub(crate) fn requested_func_ids(&self) -> &[u32] {
        &self.request.requested_func_ids
    }

    /// 快照 meta 仍记录配置侧窗口，保持旧语义：请求窗口只影响分析与通道范围。
    pub(crate) fn meta_analysis_window(&self) -> u32 {
        self.params.analysis_window
    }
}

#[cfg(test)]
mod tests {
    use super::AnalysisWindow;

    #[test]
    fn analysis_window_boundaries_match_legacy() {
        assert_eq!(AnalysisWindow::new(0).start(0), 0);
        assert_eq!(AnalysisWindow::new(0).start(10), 0);
        assert_eq!(AnalysisWindow::new(100).start(10), 0);
        assert_eq!(AnalysisWindow::new(10).start(10), 0);
        assert_eq!(AnalysisWindow::new(4).start(10), 6);
        assert_eq!(AnalysisWindow::new(4).start(0), 0);
    }
}

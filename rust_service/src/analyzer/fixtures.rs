//! 测试夹具（仅 `cfg(test)` 构建）：构造笔/请求/状态的共享工具。
//!
//! 供 analyzer 各模块的单元测试与快照测试共用，避免在多个模块里重复
//! 构造 BI / AnalyzeRequest / AppState 样板。

#![cfg(test)]

use czsc_core::objects::bi::BI;
use czsc_core::objects::direction::Direction as CzscDirection;
use czsc_core::objects::freq::Freq;
use czsc_core::objects::mark::Mark;

use chrono::{Duration, Utc};

use crate::config::AppConfig;
use crate::proto::analysis::{AnalysisSnapshot, ChannelSeries};
use crate::proto::analysis::{AnalyzeRequest, CzscOptions, MarketSeries};

use super::AnalysisParams;

/// 构造一笔（测试用）：start→end 单向运行，方向按涨跌自动判定。
///
/// fx_a/fx_b 的 dt 编码笔序号（base + idx 分钟）；端点 K 线下标编码为
/// idx / idx+1（相邻笔共享端点下标：笔 k 终点 == 笔 k+1 起点，与真实
/// 数据一致），供 `fx_mid_index` 解析出通道对齐下标。
pub(crate) fn make_series_bi(idx: i64, start: f64, end: f64) -> BI {
    let up = end > start;
    let base = Utc::now();
    let mk_fx = |price: f64, mark: Mark, dt: chrono::DateTime<chrono::Utc>, bar_id: i32| {
        let raw = czsc_core::objects::bar::RawBar {
            symbol: std::sync::Arc::from("T"),
            dt,
            freq: Freq::F30,
            open: start.min(end),
            close: start.max(end),
            high: start.max(end),
            low: start.min(end),
            vol: 0.0,
            amount: 0.0,
            id: bar_id,
        };
        let nb = |dt: chrono::DateTime<chrono::Utc>| czsc_core::objects::bar::NewBar {
            symbol: std::sync::Arc::from("T"),
            dt,
            freq: Freq::F30,
            id: bar_id,
            open: start.min(end),
            close: start.max(end),
            high: start.max(end),
            low: start.min(end),
            vol: 0.0,
            amount: 0.0,
            elements: vec![raw.clone()],
        };
        let one_min = Duration::minutes(1);
        czsc_core::objects::fx::FX {
            symbol: std::sync::Arc::from("T"),
            dt,
            mark,
            high: start.max(end),
            low: start.min(end),
            fx: price,
            elements: vec![nb(dt - one_min), nb(dt), nb(dt + one_min)],
        }
    };
    let dt_a = base + Duration::minutes(idx);
    let dt_b = base + Duration::minutes(idx + 1000);
    let (mark_a, mark_b) = if up {
        (Mark::D, Mark::G)
    } else {
        (Mark::G, Mark::D)
    };
    BI {
        symbol: std::sync::Arc::from("T"),
        fx_a: mk_fx(start, mark_a, dt_a, idx as i32),
        fx_b: mk_fx(end, mark_b, dt_b, idx as i32 + 1),
        fxs: Vec::new(),
        direction: if up {
            CzscDirection::Up
        } else {
            CzscDirection::Down
        },
        bars: Vec::new(),
    }
}

pub(crate) fn sample_close_series() -> Vec<f32> {
    vec![
        10.0, 10.2, 10.5, 10.1, 9.8, 9.6, 9.9, 10.3, 10.7, 10.4, 10.1, 9.7, 9.5, 9.9, 10.4, 10.9,
        10.6, 10.1, 9.8, 9.4, 9.7, 10.2, 10.8, 11.2, 10.9, 10.5, 10.0, 9.6, 9.9, 10.3, 10.7, 11.0,
        10.8, 10.3, 9.9, 9.5, 9.8, 10.1, 10.6, 11.1, 10.7, 10.2, 9.7, 9.3, 9.6, 10.0, 10.5, 10.9,
        10.6, 10.1, 9.8, 9.4, 9.7, 10.2, 10.6, 11.0, 10.8, 10.4, 10.0, 9.7, 10.0, 10.4, 10.9, 11.3,
    ]
}

pub(crate) fn sample_request(include_objects: bool) -> AnalyzeRequest {
    let close = sample_close_series();
    let mut high = Vec::with_capacity(close.len());
    let mut low = Vec::with_capacity(close.len());
    for (i, price) in close.iter().enumerate() {
        high.push(*price + 0.25 + (i % 3) as f32 * 0.03);
        low.push(*price - 0.22 - (i % 2) as f32 * 0.02);
    }

    AnalyzeRequest {
        func_group: "single_level_structure".to_string(),
        requested_func_ids: vec![101, 102, 201, 202, 301, 302, 303],
        symbol_hint: "000001.SZ".to_string(),
        freq_hint: "F30".to_string(),
        data_len: close.len() as u32,
        input_hash: 0xAABBCCDD,
        series: Some(MarketSeries {
            open: Vec::new(),
            high,
            low,
            close,
            vol: Vec::new(),
            amount: Vec::new(),
            dt_unix_ms: Vec::new(),
        }),
        options: Some(CzscOptions {
            max_bi_num: 50,
            min_bi_len: 3,
            analysis_window: 64,
            include_objects,
            enable_incremental: false,
        }),
        prefer_cached: false,
    }
}

/// 测试用服务配置（min_bi_len=3、窗口 64，贴近样例请求）。
pub(crate) fn test_config() -> AppConfig {
    AppConfig {
        min_bi_len: 3,
        default_analysis_window: 64,
        ..AppConfig::default()
    }
}

/// 由样例请求 + 测试配置解析出的分析参数（`build_snapshot` 输入）。
pub(crate) fn test_params(request: &AnalyzeRequest) -> AnalysisParams {
    AnalysisParams::from((request, &test_config()))
}

pub(crate) fn channel(snapshot: &AnalysisSnapshot, func_id: u32) -> &ChannelSeries {
    snapshot
        .channels
        .iter()
        .find(|x| x.func_id == func_id)
        .unwrap_or_else(|| panic!("channel {func_id} not found"))
}

pub(crate) fn nonzero_count(values: &[f32]) -> usize {
    values.iter().filter(|x| x.abs() > f32::EPSILON).count()
}

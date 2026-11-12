//! 集成测试共享设施：CSV 行情读取与 `AnalyzeRequest` 构造。
//!
//! 各回归测试文件（zs_boundary/seg/signal/divergence/incremental/atr）此前各自
//! 维护一份几乎相同的 CSV 解析与请求构造样板，这里统一收口。

use czsc_core::objects::bar::{RawBar, RawBarBuilder};
use czsc_core::objects::freq::Freq;
use tdx_czsc_service::config::AppConfig;
use tdx_czsc_service::proto::analysis::{AnalyzeRequest, CzscOptions, MarketSeries};

/// 读取仓库数据 CSV（`日期,开,高,低,收,...`），返回 (日期, high, low, close)。
/// `#[allow(dead_code)]`：各测试二进制独立编译，个别测试只用 `analyze_request`。
#[allow(dead_code)]
pub(crate) fn load_series(path: &str) -> (Vec<String>, Vec<f32>, Vec<f32>, Vec<f32>) {
    let csv = std::fs::read_to_string(path).expect("read csv");
    let mut dates = Vec::new();
    let mut high = Vec::new();
    let mut low = Vec::new();
    let mut close = Vec::new();
    for line in csv.lines().skip(1) {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 5 {
            continue;
        }
        dates.push(parts[0].to_string());
        high.push(parts[2].parse().expect("high"));
        low.push(parts[3].parse().expect("low"));
        close.push(parts[4].parse().expect("close"));
    }
    (dates, high, low, close)
}

/// CSV → RawBar（**全局 bar 下标为 id**、真实日期、freq=D）。
/// 从 `start`（全局下标）取到文件末尾；id/dt 语义与增量前缀判定、
/// 通道对齐的可复现性保持一致（此前 signal/signals_fast 各维护一份，
/// 现统一于此）。
/// `#[allow(dead_code)]`：个别测试二进制只走 `analyze_request` 路径。
#[allow(dead_code)]
pub(crate) fn load_raw_bars(path: &str, start: usize) -> Vec<RawBar> {
    let csv = std::fs::read_to_string(path).expect("read csv");
    let mut bars = Vec::new();
    for (i, line) in csv.lines().skip(1).enumerate() {
        if i < start {
            continue;
        }
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 5 {
            continue;
        }
        let dt = chrono::NaiveDate::parse_from_str(parts[0], "%Y-%m-%d")
            .expect("date")
            .and_hms_opt(0, 0, 0)
            .expect("time")
            .and_local_timezone(chrono::Utc)
            .unwrap();
        let high: f32 = parts[2].parse().expect("high");
        let low: f32 = parts[3].parse().expect("low");
        let close: f32 = parts[4].parse().expect("close");
        bars.push(
            RawBarBuilder::default()
                .symbol("002340.SZ")
                .dt(dt)
                .id(i as i32)
                .freq(Freq::D)
                .open(close as f64)
                .close(close as f64)
                .high(high as f64)
                .low(low as f64)
                .vol(0.0)
                .amount(0.0)
                .build()
                .expect("bar"),
        );
    }
    bars
}

/// 构造分析请求（无真实时间戳；窗口/笔参数显式指定）。
/// `#[allow(dead_code)]`：各测试二进制独立编译，个别二进制只用 `load_series`。
#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze_request(
    requested_func_ids: Vec<u32>,
    freq_hint: &str,
    high: Vec<f32>,
    low: Vec<f32>,
    close: Vec<f32>,
    min_bi_len: u32,
    analysis_window: u32,
    include_objects: bool,
) -> AnalyzeRequest {
    analyze_request_ex(
        requested_func_ids,
        freq_hint,
        high,
        low,
        close,
        min_bi_len,
        analysis_window,
        include_objects,
        50,
    )
}

/// `analyze_request` 的 `max_bi_num` 可定制版本（全量历史回归用；
/// 缺省接口保持 50 以兼容既有调用点）。
#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze_request_ex(
    requested_func_ids: Vec<u32>,
    freq_hint: &str,
    high: Vec<f32>,
    low: Vec<f32>,
    close: Vec<f32>,
    min_bi_len: u32,
    analysis_window: u32,
    include_objects: bool,
    max_bi_num: u32,
) -> AnalyzeRequest {
    let data_len = high.len() as u32;
    AnalyzeRequest {
        func_group: "single_level_structure".to_string(),
        requested_func_ids,
        freq_hint: freq_hint.to_string(),
        data_len,
        input_hash: 0,
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
            max_bi_num,
            min_bi_len,
            analysis_window,
            include_objects,
            enable_incremental: false,
        }),
        prefer_cached: false,
    }
}

/// 按笔长/分析窗口构造测试配置（贴近各数据文件的实际参数）。
#[allow(dead_code)]
pub(crate) fn config_for(min_bi_len: u32, analysis_window: u32) -> AppConfig {
    AppConfig {
        min_bi_len,
        default_analysis_window: analysis_window,
        ..AppConfig::default()
    }
}

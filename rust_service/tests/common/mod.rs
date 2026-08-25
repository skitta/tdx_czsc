//! 集成测试共享设施：CSV 行情读取与 `AnalyzeRequest` 构造。
//!
//! 四个回归测试文件（zs_boundary/seg/signal/divergence）此前各自维护一份
//! 几乎相同的 CSV 解析与请求构造样板，这里统一收口。

use tdx_czsc_service::config::AppConfig;
use tdx_czsc_service::proto::analysis::{AnalyzeRequest, BarBuildMode, CzscOptions, MarketSeries};

/// 读取仓库数据 CSV（`日期,开,高,低,收,...`），返回 (日期, high, low, close)。
/// `#[allow(dead_code)]`：各测试二进制独立编译，个别测试只用 `analyze_request`。
#[allow(dead_code)]
pub fn load_series(path: &str) -> (Vec<String>, Vec<f32>, Vec<f32>, Vec<f32>) {
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

/// 构造分析请求（无真实时间戳；窗口/笔参数显式指定）。
#[allow(clippy::too_many_arguments)]
pub fn analyze_request(
    symbol: &str,
    requested_func_ids: Vec<u32>,
    freq_hint: &str,
    high: Vec<f32>,
    low: Vec<f32>,
    close: Vec<f32>,
    min_bi_len: u32,
    analysis_window: u32,
    include_objects: bool,
) -> AnalyzeRequest {
    let data_len = high.len() as u32;
    AnalyzeRequest {
        func_group: "single_level_structure".to_string(),
        requested_func_ids,
        symbol_hint: symbol.to_string(),
        freq_hint: freq_hint.to_string(),
        data_len,
        input_hash: 0,
        bar_build_mode: BarBuildMode::BarBuildFromHlc3 as i32,
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
            min_bi_len,
            analysis_window,
            include_channels: true,
            include_objects,
            enable_incremental: false,
        }),
        prefer_cached: false,
        caller_instance_id: 7,
    }
}

/// 按笔长/分析窗口构造测试配置（贴近各数据文件的实际参数）。
pub fn config_for(min_bi_len: u32, analysis_window: u32) -> AppConfig {
    AppConfig {
        min_bi_len,
        default_analysis_window: analysis_window,
        ..AppConfig::default()
    }
}

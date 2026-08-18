//! 回归测试：相邻中枢共享端点K线时，304 边界通道必须输出 3（左右界共用），
//! 否则前一中枢会丢失右边框，图上只剩左侧竖线、没有上下边框和右边框。
//! 使用仓库内的真实数据 tests/data/002340_day.csv（对应报告的 002340 案例：
//! 中枢 2026-01-07、2025-11-24 等）。

use tdx_czsc_service::analyzer::build_snapshot;
use tdx_czsc_service::config::AppConfig;
use tdx_czsc_service::proto::analysis::{AnalyzeRequest, CzscOptions, MarketSeries};
use tdx_czsc_service::state::AppState;

fn load_series(path: &str) -> (Vec<String>, Vec<f32>, Vec<f32>, Vec<f32>) {
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

fn build_channels() -> (Vec<String>, tdx_czsc_service::proto::analysis::AnalysisSnapshot, Vec<f32>) {
    let (dates, high, low, close) = load_series("../tests/data/002340_day.csv");
    let data_len = high.len() as u32;

    let request = AnalyzeRequest {
        func_group: "single_level_structure".to_string(),
        requested_func_ids: vec![301, 302, 303, 304],
        symbol_hint: "002340.SZ".to_string(),
        freq_hint: "D".to_string(),
        data_len,
        input_hash: 0,
        bar_build_mode: tdx_czsc_service::proto::analysis::BarBuildMode::BarBuildFromHlc3 as i32,
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
            min_bi_len: 6,
            analysis_window: 1000,
            include_channels: true,
            include_objects: true,
            enable_incremental: false,
        }),
        prefer_cached: false,
        caller_instance_id: 7,
    };

    let mut config = AppConfig::default();
    config.min_bi_len = 6;
    config.default_analysis_window = 1000;
    let state = AppState::new(config);
    let snapshot = build_snapshot(&request, &state).expect("snapshot should build");

    let boundary = snapshot
        .channels
        .iter()
        .find(|c| c.func_id == 304)
        .expect("channel 304")
        .values
        .clone();
    (dates, snapshot, boundary)
}

/// 每个中枢都必须同时有左界（1）和右界（2）标记。
#[test]
fn every_pivot_has_both_boundary_marks_on_real_data() {
    let (_, snapshot, boundary) = build_channels();

    assert!(
        !snapshot.pivots.is_empty(),
        "002340 day data should produce pivots"
    );

    for pivot in &snapshot.pivots {
        let s = pivot.start_index as usize;
        let e = pivot.end_index as usize;
        let left = boundary[s.min(e)];
        let right = boundary[s.max(e)];
        assert!(
            (left - 1.0).abs() < 0.5 || (left - 3.0).abs() < 0.5,
            "pivot [{s},{e}] start should be marked 1 or 3, got {left}"
        );
        assert!(
            (right - 2.0).abs() < 0.5 || (right - 3.0).abs() < 0.5,
            "pivot [{s},{e}] end should be marked 2 or 3, got {right}"
        );
    }
}

/// 缠论规则：两个中枢之间必须至少间隔一笔，相邻中枢不得共享端点K线。
/// 修复前 czsc 的 get_zs_seq 把连接笔并入新中枢，相邻中枢共享边界；修复后
/// 绘制区间从每个中枢的第二笔起点开始，连接笔落在两个矩形之间。
#[test]
fn adjacent_pivots_are_separated_by_a_bi_gap_on_real_data() {
    let (dates, snapshot, boundary) = build_channels();

    assert!(
        snapshot.pivots.len() >= 2,
        "002340 day data should produce multiple pivots"
    );

    for pair in snapshot.pivots.windows(2) {
        let prev_end = pair[0].end_index as usize;
        let next_start = pair[1].start_index as usize;
        assert!(
            next_start > prev_end,
            "adjacent pivots must not share a boundary bar: prev ends at {} ({}), next starts at {} ({})",
            prev_end,
            dates.get(prev_end).cloned().unwrap_or_default(),
            next_start,
            dates.get(next_start).cloned().unwrap_or_default(),
        );
        // 间隔区间内不得有中枢填充/边界标记
        let gap_has_fill = (prev_end + 1..next_start)
            .any(|i| boundary[i].abs() > 1e-6);
        assert!(
            !gap_has_fill,
            "gap between pivots {}..{} must be free of boundary marks",
            prev_end,
            next_start,
        );
    }
}

/// 报告中的具体中枢（2025-11-24..2026-01-07 区间 [7.22..7.63]）必须有右界标记。
/// 该中枢排除离开段（2025-12-16 之后涨到 9.41 的笔）后绘制终点为 2025-12-16。
/// （按缠论间隔规则重建后，原以 2025-11-24 结尾的 [7.74..8.68] 中枢被重新
/// 识别——其首笔是前一中枢的离开/连接笔，不再单独成中枢。）
#[test]
fn reported_zs_end_dates_have_right_boundary() {
    let (dates, snapshot, boundary) = build_channels();

    let mut checked = 0usize;
    for pivot in &snapshot.pivots {
        let e = pivot.end_index as usize;
        let end_date = dates.get(e).cloned().unwrap_or_default();
        if end_date == "2025-12-16" {
            let right = boundary[e];
            assert!(
                (right - 2.0).abs() < 0.5 || (right - 3.0).abs() < 0.5,
                "中枢 ending at {end_date} (index {e}) must have right boundary, got {right}"
            );
            checked += 1;
        }
    }

    assert!(checked >= 1, "expected the 2025-12-16 中枢 to be found, found {checked}");
}

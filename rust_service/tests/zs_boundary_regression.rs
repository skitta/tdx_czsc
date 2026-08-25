//! 回归测试：原生 czsc 中枢画法（`get_zs_seq`）的通道映射——相邻中枢共享
//! 端点K线时，304 边界通道必须输出 3（左右界共用），否则前一中枢会丢失
//! 右边框，图上只剩左侧竖线、没有上下边框和右边框。
//! 使用仓库内的真实数据 tests/data/002340_day.csv。

use tdx_czsc_service::analyzer::{build_snapshot, AnalysisParams};
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
    let params = AnalysisParams::from((&request, &state.config));
    let snapshot = build_snapshot(&request, &params).expect("snapshot should build");

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

/// 前端/后端优化后，相邻中枢必须至少间隔一笔（离开/连接笔不属于任一中枢），
/// 不得共享端点K线（304 通道的 3 仅作防御性兜底）。
#[test]
fn adjacent_pivots_never_share_endpoints_on_real_data() {
    let (_, snapshot, _) = build_channels();

    assert!(
        snapshot.pivots.len() >= 2,
        "002340 day data should produce multiple pivots"
    );

    for pair in snapshot.pivots.windows(2) {
        let prev_end = pair[0].end_index as usize;
        let next_start = pair[1].start_index as usize;
        assert!(
            next_start > prev_end,
            "前中枢后端必须与后一中枢前端至少间隔一笔: prev ends at {prev_end}, next starts at {next_start}"
        );
    }
}

/// 中枢前端/后端规则（真实数据回归，覆盖三份仓库数据）：
/// - **前端**：中枢第一笔与行情趋势相反——上升行情第一笔下、下跌行情第一
///   笔上；行情方向按中枢高低位置判定（后中枢中轴高于前中枢 → 上升行情；
///   第一个中枢与窗口起点价格比较兜底）；
/// - **后端**：中枢最后一笔与行情趋势相反（离开笔已剔除，中枢为奇数笔
///   结构）。
/// 与 `build_snapshot` 的构建路径一致（同一份笔输入、同一分析窗口）。
#[test]
fn pivot_edges_follow_trend_on_real_data() {
    use czsc_core::analyze::CZSC;
    use czsc_core::objects::direction::Direction;
    use tdx_czsc_service::analyzer::{build_pivots, build_raw_bars, parse_freq};

    for (csv_path, freq_hint, min_bi_len, window) in [
        ("../tests/data/002340_day.csv", "D", 6, 1000u32),
        ("../tests/data/999999_day.csv", "D", 6, 1000),
        ("../tests/data/002340_30m.csv", "F30", 3, 1000),
    ] {
        let (_, high, low, close) = load_series(csv_path);
        let data_len = high.len() as u32;

        let request = AnalyzeRequest {
            func_group: "single_level_structure".to_string(),
            requested_func_ids: vec![],
            symbol_hint: csv_path
                .trim_start_matches("../tests/data/")
                .trim_end_matches(".csv")
                .to_string(),
            freq_hint: freq_hint.to_string(),
            data_len,
            input_hash: 0,
            bar_build_mode: tdx_czsc_service::proto::analysis::BarBuildMode::BarBuildFromHlc3
                as i32,
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
                analysis_window: window,
                include_channels: false,
                include_objects: false,
                enable_incremental: false,
            }),
            prefer_cached: false,
            caller_instance_id: 7,
        };

        let mut config = AppConfig::default();
        config.min_bi_len = min_bi_len;
        config.default_analysis_window = window;
        let state = AppState::new(config);

        let freq = parse_freq(freq_hint).expect("freq");
        let window_start = (data_len as usize).saturating_sub(window as usize);
        let bars =
            build_raw_bars(&request, freq, window_start, state.config.derive_volume_proxy)
                .expect("bars");
        let czsc = CZSC::new(bars, 50, min_bi_len as usize);
        let bis = czsc.get_finished_bis();
        let pivots = build_pivots(&bis);

        assert!(
            !pivots.is_empty(),
            "{csv_path} should produce pivots (bis={})",
            bis.len()
        );
        let mut prev_trend = Direction::Up;
        for (k, zs) in pivots.iter().enumerate() {
            // 行情趋势 = 中枢高低位置
            let trend = if k == 0 {
                if zs.zz > bis[0].fx_a.fx {
                    Direction::Up
                } else {
                    Direction::Down
                }
            } else if zs.zz > pivots[k - 1].zz {
                Direction::Up
            } else if zs.zz < pivots[k - 1].zz {
                Direction::Down
            } else {
                prev_trend
            };
            prev_trend = trend;
            let expected_sdir = match trend {
                Direction::Up => Direction::Down,
                Direction::Down => Direction::Up,
            };
            // 前端：首笔与趋势相反
            assert_eq!(
                zs.sdir, expected_sdir,
                "中枢{k}第一笔方向应为 {expected_sdir:?}（趋势 {trend:?}）: {csv_path} from {} zz={:.3}",
                zs.sdt.format("%Y-%m-%d"),
                zs.zz
            );
            // 后端：末笔与趋势相反（奇数笔结构，离开笔已剔除）
            assert_eq!(
                zs.bis.last().unwrap().direction,
                expected_sdir,
                "中枢{k}最后一笔方向应为 {expected_sdir:?}（趋势 {trend:?}）: {csv_path} from {}",
                zs.sdt.format("%Y-%m-%d")
            );
            // 相邻间隔：前中枢后端与后一中枢前端至少间隔一笔（由 snapshot
            // 通道映射校验，见 adjacent_pivots_never_share_endpoints_on_real_data）
        }
        println!("{csv_path}: bis={} pivots={}", bis.len(), pivots.len());
    }
}

/// 前端+后端优化后的中枢画法在报告案例（002340 日线）上的回归锚点：
/// 断言完整的 7 个中枢配置（区间、绘制起止日期、左右界标记）。
///
/// 规则演变（同一数据、同一窗口）：
/// - 纯原生 get_zs_seq：9 个中枢，相邻共享端点（连接笔并入后一中枢）；
/// - 仅前端优化：8 个（原生 [6.32..6.67] 仅 2 笔剔除）；
/// - 前端+后端优化（本轮）：7 个——每个中枢末笔与趋势相反（离开笔剔除，
///   奇数笔结构），相邻中枢至少间隔一笔。报告中的 [7.22..7.63]（3 笔，
///   两端均与邻居共享端点）无法满足间隔规则，整组剔除；其两侧中枢
///   [7.74..8.68] 与 [8.50..9.41] 保留并留出间隔。
#[test]
fn native_zs_configuration_on_reported_data() {
    let (dates, snapshot, boundary) = build_channels();

    let expected: Vec<(f64, f64, &str, &str)> = vec![
        (5.52, 6.84, "2024-03-28", "2024-10-08"),
        (6.72, 7.09, "2024-10-23", "2024-11-19"),
        (6.05, 6.24, "2025-01-06", "2025-07-24"),
        (7.15, 7.54, "2025-08-27", "2025-09-23"),
        (7.74, 8.68, "2025-10-14", "2025-11-24"),
        (8.50, 9.41, "2026-01-07", "2026-03-24"),
        (6.67, 7.08, "2026-06-08", "2026-08-10"),
    ];
    assert_eq!(
        snapshot.pivots.len(),
        expected.len(),
        "前端+后端优化后应识别出 {} 个中枢，实际 {}",
        expected.len(),
        snapshot.pivots.len()
    );

    for (pivot, (lower, upper, start_date, end_date)) in snapshot.pivots.iter().zip(&expected) {
        assert!((pivot.lower - lower).abs() < 1e-3, "中枢下沿应为 {lower}");
        assert!((pivot.upper - upper).abs() < 1e-3, "中枢上沿应为 {upper}");
        let s = pivot.start_index as usize;
        let e = pivot.end_index as usize;
        assert_eq!(
            dates.get(s).cloned().unwrap_or_default(),
            *start_date,
            "中枢 [{lower}..{upper}] 绘制起点应为 {start_date}"
        );
        assert_eq!(
            dates.get(e).cloned().unwrap_or_default(),
            *end_date,
            "中枢 [{lower}..{upper}] 绘制终点应为 {end_date}"
        );
        let left = boundary[s.min(e)];
        let right = boundary[s.max(e)];
        assert!(
            (left - 1.0).abs() < 0.5 || (left - 3.0).abs() < 0.5,
            "中枢 [{s},{e}] start should be marked 1 or 3, got {left}"
        );
        assert!(
            (right - 2.0).abs() < 0.5 || (right - 3.0).abs() < 0.5,
            "中枢 [{s},{e}] end should be marked 2 or 3, got {right}"
        );
    }
}

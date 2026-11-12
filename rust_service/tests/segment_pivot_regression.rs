//! 回归测试：段中枢（305~307 通道与 `segment_pivots` 对象）。
//!
//! 段中枢以**已确认段**为构件（3 段重叠成中枢），复用笔中枢的趋势判定与
//! 进入/离开构件修剪规则：
//! - 相邻段中枢不共享端点 K 线（至少间隔一段）；
//! - 上下沿 `[zd, zg]` 由中枢前 3 段的重叠决定，中轴为区间中点；
//! - 305/306 在中枢区间内恒为上下沿、区间外为 0；307 首 K 线=1、尾 K 线=2。

use tdx_czsc_service::analyzer::{AnalysisParams, SnapshotAnalyzer};
use tdx_czsc_service::proto::analysis::{AnalysisSnapshot, Segment};
use tdx_czsc_service::state::AppState;

mod common;
use common::{analyze_request_ex, config_for, load_series};

fn build(csv: &str, freq: &str, min_bi_len: u32) -> AnalysisSnapshot {
    let (_, high, low, close) = load_series(csv);
    let request = analyze_request_ex(
        vec![305, 306, 307],
        freq,
        high,
        low,
        close,
        min_bi_len,
        1000,
        true,
        1000,
    );
    let state = AppState::new(config_for(min_bi_len, 1000));
    let params = AnalysisParams::from(state.config());
    SnapshotAnalyzer::new(&params)
        .build_snapshot(&request)
        .expect("snapshot")
}

fn channel(snapshot: &AnalysisSnapshot, func_id: u32) -> &Vec<f32> {
    &snapshot
        .channels
        .iter()
        .find(|c| c.func_id == func_id)
        .unwrap_or_else(|| panic!("channel {func_id} not found"))
        .values
}

/// 30 分钟数据（110 笔 / 约 19 段）回归锚点：应识别出 3 个段中枢，
/// 且每个中枢在 305/306/307 上都有完整的上沿、下沿与左右界。
#[test]
fn intraday_segment_pivots_are_drawn() {
    let snapshot = build("../tests/data/002340_30m.csv", "F30", 3);
    let upper = channel(&snapshot, 305);
    let lower = channel(&snapshot, 306);
    let boundary = channel(&snapshot, 307);

    assert_eq!(
        snapshot.segment_pivots.len(),
        3,
        "002340 30m 应识别出 3 个段中枢"
    );

    for pivot in &snapshot.segment_pivots {
        let s = pivot.start_index as usize;
        let e = pivot.end_index as usize;
        assert!(s < e, "段中枢区间应有效: [{s},{e}]");
        assert!(pivot.upper > pivot.lower, "段中枢上沿应高于下沿: [{s},{e}]");
        assert!(
            ((pivot.upper + pivot.lower) * 0.5 - pivot.middle).abs() < 1e-4,
            "段中枢中轴应为上下沿中点: [{s},{e}]"
        );

        for i in s..=e {
            assert!(
                (upper[i] - pivot.upper as f32).abs() < 1e-4,
                "305 在段中枢区间内应为上沿: bar {i}"
            );
            assert!(
                (lower[i] - pivot.lower as f32).abs() < 1e-4,
                "306 在段中枢区间内应为下沿: bar {i}"
            );
        }
        assert!((boundary[s] - 1.0).abs() < 0.5, "307 左界应为 1");
        assert!((boundary[e] - 2.0).abs() < 0.5, "307 右界应为 2");
    }
}

/// 段中枢之间互不重叠，且严格按时间递增（前中枢后端与后一中枢前端至少
/// 间隔一段）。
#[test]
fn segment_pivots_never_overlap() {
    let snapshot = build("../tests/data/002340_30m.csv", "F30", 3);
    for pair in snapshot.segment_pivots.windows(2) {
        let prev_end = pair[0].end_index;
        let next_start = pair[1].start_index;
        assert!(
            next_start > prev_end,
            "段中枢必须按时间递增且不重叠: prev ends {prev_end}, next starts {next_start}"
        );
    }
}

/// 段中枢的端点必须落在已确认段的端点集合内：未确认段不参与段中枢。
#[test]
fn segment_pivots_use_confirmed_segments_only() {
    let snapshot = build("../tests/data/002340_30m.csv", "F30", 3);
    let confirmed: Vec<&Segment> = snapshot.segments.iter().filter(|s| s.is_sure).collect();
    assert!(confirmed.len() >= 9, "30m 窗口应至少确认 9 段");

    for pivot in &snapshot.segment_pivots {
        assert!(
            confirmed
                .iter()
                .any(|seg| seg.start_index == pivot.start_index),
            "段中枢起点 {} 必须来自已确认段",
            pivot.start_index
        );
        assert!(
            confirmed.iter().any(|seg| seg.end_index == pivot.end_index),
            "段中枢终点 {} 必须来自已确认段",
            pivot.end_index
        );
    }
}

/// 日线数据回归：段粒度较粗，段中枢数量远少于笔中枢；每个中枢结构仍须
/// 合法（不重叠、上下沿有效、通道只在区间内非零）。
#[test]
fn daily_segment_pivots_are_structurally_valid() {
    let snapshot = build("../tests/data/002340_day.csv", "D", 6);
    let upper = channel(&snapshot, 305);
    let lower = channel(&snapshot, 306);

    assert!(
        snapshot.segment_pivots.len() < snapshot.pivots.len(),
        "段中枢数量应少于笔中枢: seg={} bi={}",
        snapshot.segment_pivots.len(),
        snapshot.pivots.len()
    );

    for pivot in &snapshot.segment_pivots {
        let s = pivot.start_index as usize;
        let e = pivot.end_index as usize;
        assert!(s < e);
        assert!(pivot.upper > pivot.lower);

        for i in s..=e {
            assert!(
                (upper[i] - pivot.upper as f32).abs() < 1e-4
                    && (lower[i] - pivot.lower as f32).abs() < 1e-4,
                "305/306 在段中枢区间内应为上下沿: bar {i}"
            );
        }
    }

    // 未被任何段中枢覆盖的 K 线两路通道都应为 0（区间外不画线）
    let mut covered = vec![false; upper.len()];
    for pivot in &snapshot.segment_pivots {
        let s = pivot.start_index as usize;
        let e = pivot.end_index as usize;
        covered[s..=e].fill(true);
    }
    for (i, in_pivot) in covered.iter().enumerate() {
        if !in_pivot {
            assert!(
                upper[i].abs() < 1e-4 && lower[i].abs() < 1e-4,
                "段中枢区间外 305/306 应为 0: bar {i}"
            );
        }
    }
}

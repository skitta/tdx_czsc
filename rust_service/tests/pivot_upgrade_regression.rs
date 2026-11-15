//! 回归测试：中枢扩展/延伸/升级（九段 + 三三得九）。
//!
//! 002340 日线窗口内有三组可升级的中枢链：
//! - `[5.52..6.84]`、`[6.72..7.09]`、`[6.05..6.24]` 两两重叠，升级为
//!   `[5.58..6.12]`，绘制区间覆盖三条子中枢；
//! - `[7.74..8.68]`、`[8.50..9.41]` 重叠，升级为 `[7.43..9.21]`；
//! - 其余两个中枢保持 level-1。
//!
//! 升级后的中枢替换子中枢：只保留高一级矩形，子中枢不再单独绘制；高一级
//! 矩形在时间与价格上都包住子中枢（只加宽不收窄）。

use tdx_czsc_service::analyzer::{AnalysisParams, SnapshotAnalyzer};
use tdx_czsc_service::proto::analysis::AnalysisSnapshot;
use tdx_czsc_service::state::AppState;

mod common;
use common::{analyze_request, config_for, load_series};

fn build() -> (Vec<String>, AnalysisSnapshot) {
    let (dates, high, low, close) = load_series("../tests/data/002340_day.csv");
    let request = analyze_request(
        vec![301, 302, 303, 304, 308],
        "D",
        high,
        low,
        close,
        6,
        1000,
        true,
    );
    let state = AppState::new(config_for(6, 1000));
    let params = AnalysisParams::from(state.config());
    let snapshot = SnapshotAnalyzer::new(&params)
        .build_snapshot(&request)
        .expect("snapshot");
    (dates, snapshot)
}

fn channel(snapshot: &AnalysisSnapshot, func_id: u32) -> &Vec<f32> {
    &snapshot
        .channels
        .iter()
        .find(|c| c.func_id == func_id)
        .unwrap_or_else(|| panic!("channel {func_id} not found"))
        .values
}

/// 级别通道（308）：升级后的中枢区间输出 2，本级中枢输出 1，区间外为 0。
#[test]
fn upgraded_pivots_report_level_channel() {
    let (_, snapshot) = build();
    let level = channel(&snapshot, 308);

    let expected: [f32; 4] = [2.0, 1.0, 2.0, 1.0];
    assert_eq!(snapshot.pivots.len(), expected.len(), "前置锚点应保持不变");
    for (pivot, &expected_level) in snapshot.pivots.iter().zip(&expected) {
        for i in pivot.start_index..=pivot.end_index {
            assert_eq!(
                level[i as usize], expected_level,
                "级别通道在 [{},{}] 内应为 {expected_level}: bar {i}",
                pivot.start_index, pivot.end_index
            );
        }
    }

    let mut covered = vec![false; level.len()];
    for pivot in &snapshot.pivots {
        covered[pivot.start_index as usize..=pivot.end_index as usize].fill(true);
    }
    for (i, in_pivot) in covered.iter().enumerate() {
        if !in_pivot {
            assert_eq!(level[i], 0.0, "中枢区间外级别通道应为 0: bar {i}");
        }
    }
}

/// 升级后的中枢必须覆盖被替换的子中枢，且子中枢不再单独出现。
#[test]
fn upgraded_pivots_replace_child_pivots() {
    let (dates, snapshot) = build();
    assert_eq!(snapshot.pivots.len(), 4, "九段升级后应只剩 4 个中枢");

    let date_of = |index: u32| dates.get(index as usize).cloned().unwrap_or_default();
    let spans: Vec<(String, String)> = snapshot
        .pivots
        .iter()
        .map(|p| (date_of(p.start_index), date_of(p.end_index)))
        .collect();
    assert_eq!(
        spans,
        [
            ("2024-03-28", "2025-07-24"),
            ("2025-08-27", "2025-09-23"),
            ("2025-10-14", "2026-03-24"),
            ("2026-06-08", "2026-08-10"),
        ]
        .map(|(start, end)| (start.to_string(), end.to_string())),
        "升级中枢应覆盖被替换子中枢的完整区间"
    );

    // 子中枢不再单独出现，且升级矩形必须包住每个子中枢的价格区间
    for (lower, upper) in [
        (5.52, 6.84),
        (6.72, 7.09),
        (6.05, 6.24),
        (7.74, 8.68),
        (8.50, 9.41),
    ] {
        assert!(
            !snapshot
                .pivots
                .iter()
                .any(|p| (p.lower - lower).abs() < 1e-3 && (p.upper - upper).abs() < 1e-3),
            "子中枢 [{lower}..{upper}] 应已被升级替换"
        );
        assert!(
            snapshot
                .pivots
                .iter()
                .any(|p| p.lower <= lower + 1e-3 && p.upper >= upper - 1e-3),
            "升级中枢价格区间应包住子中枢 [{lower}..{upper}]"
        );
    }
}

/// 升级后中枢之间在时间上互不重叠，且不会共享端点 K 线。
#[test]
fn upgraded_pivots_are_disjoint_in_time() {
    let (_, snapshot) = build();
    for pair in snapshot.pivots.windows(2) {
        assert!(
            pair[1].start_index > pair[0].end_index,
            "中枢必须按时间递增且不共享端点: {:?} -> {:?}",
            (pair[0].start_index, pair[0].end_index),
            (pair[1].start_index, pair[1].end_index)
        );
    }
}

/// 升级中枢仍走既有 301/302/303/304 通道：区间内恒为上下沿/中轴，
/// 左右界标记完整，区间外为 0。
#[test]
fn upgraded_pivots_fill_existing_channels() {
    let (_, snapshot) = build();
    let upper = channel(&snapshot, 301);
    let lower = channel(&snapshot, 302);
    let middle = channel(&snapshot, 303);
    let boundary = channel(&snapshot, 304);

    let mut covered = vec![false; upper.len()];
    for pivot in &snapshot.pivots {
        let s = pivot.start_index as usize;
        let e = pivot.end_index as usize;
        covered[s..=e].fill(true);
        for i in s..=e {
            assert!((upper[i] - pivot.upper as f32).abs() < 1e-4, "301 bar {i}");
            assert!((lower[i] - pivot.lower as f32).abs() < 1e-4, "302 bar {i}");
            assert!(
                (middle[i] - pivot.middle as f32).abs() < 1e-4,
                "303 bar {i}"
            );
        }
        assert!(
            (boundary[s] - 1.0).abs() < 0.5 || (boundary[s] - 3.0).abs() < 0.5,
            "304 左界应为 1 或 3: bar {s}"
        );
        assert!(
            (boundary[e] - 2.0).abs() < 0.5 || (boundary[e] - 3.0).abs() < 0.5,
            "304 右界应为 2 或 3: bar {e}"
        );
    }
    for (i, in_pivot) in covered.iter().enumerate() {
        if !in_pivot {
            assert_eq!(upper[i], 0.0, "中枢区间外 301 应为 0: bar {i}");
            assert_eq!(lower[i], 0.0, "中枢区间外 302 应为 0: bar {i}");
        }
    }
}

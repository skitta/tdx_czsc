//! 回归测试：段（线段）识别（203/204 通道与 segments 对象）。
//!
//! 缠论规则（见 analyzer/segs.rs，特征序列分型法）：
//! - 段 = 至少 3 笔、首笔与末笔同方向；终结由特征序列分型确认；
//! - 段连续覆盖全部已完成笔（相邻段不重叠、不遗漏）；
//! - 段端点都是分型位置（笔端点）；
//! - 确认段（is_sure）必须满足方向校验（向上段起点低于终点、向下段反之）。
//!
//! 使用仓库内真实数据 tests/data/002340_day.csv（对应报告的 002340 案例）。

use tdx_czsc_service::analyzer::{AnalysisParams, build_snapshot};
use tdx_czsc_service::proto::analysis::Segment;
use tdx_czsc_service::state::AppState;

mod common;
use common::{analyze_request, config_for, load_series};

fn build() -> (
    Vec<String>,
    tdx_czsc_service::proto::analysis::AnalysisSnapshot,
) {
    let (dates, high, low, close) = load_series("../tests/data/002340_day.csv");
    let request = analyze_request(
        vec![101, 102, 201, 203, 204],
        "D",
        high,
        low,
        close,
        6,
        1000,
        true,
    );
    let state = AppState::new(config_for(6, 1000));
    let params = AnalysisParams::from((&request, &state.config));
    let snapshot = build_snapshot(&request, &params).expect("snapshot");
    (dates, snapshot)
}

fn channel(
    snapshot: &tdx_czsc_service::proto::analysis::AnalysisSnapshot,
    func_id: u32,
) -> &Vec<f32> {
    &snapshot
        .channels
        .iter()
        .find(|c| c.func_id == func_id)
        .unwrap_or_else(|| panic!("channel {func_id} not found"))
        .values
}

/// 段连续覆盖全部已完成笔：首段起点=第 0 笔，每段起点=上一段末笔+1，
/// 末段终点=最后一笔，相邻段方向交替。
#[test]
fn segments_cover_all_bis_contiguously_and_alternate() {
    let (_, snapshot) = build();
    let segs = &snapshot.segments;
    assert!(!segs.is_empty(), "002340 window 应识别出段");

    let mut prev: Option<&Segment> = None;
    for seg in segs {
        if let Some(p) = prev {
            assert_eq!(
                seg.start_bi_index,
                p.end_bi_index + 1,
                "段必须连续覆盖（不重叠、不遗漏）"
            );
            assert_ne!(seg.direction, p.direction, "相邻段方向必须交替");
        } else {
            assert_eq!(seg.start_bi_index, 0, "首段必须从窗口第一笔开始");
        }
        prev = Some(seg);
    }
    // 末段终点 = 最后一笔终点（尾部未完成段也画到最新笔）
    let last_bi_idx = (snapshot.strokes.len() - 1) as u32;
    assert_eq!(
        prev.unwrap().end_bi_index,
        last_bi_idx,
        "末段终点应等于最后一笔"
    );
}

/// 确认段（is_sure=true）至少 3 笔、首尾笔同向，且价格方向与段方向一致。
#[test]
fn sure_segments_have_at_least_3_bis_and_valid_direction() {
    let (_, snapshot) = build();
    let segs = &snapshot.segments;
    let bis = &snapshot.strokes;

    let sure: Vec<&Segment> = segs.iter().filter(|s| s.is_sure).collect();
    assert!(!sure.is_empty(), "窗口内应有已确认段");

    for seg in &sure {
        let start_bi = &bis[seg.start_bi_index as usize];
        let end_bi = &bis[seg.end_bi_index as usize];
        assert!(
            seg.end_bi_index - seg.start_bi_index + 1 >= 3,
            "确认段至少 3 笔: [{},{}]",
            seg.start_bi_index,
            seg.end_bi_index
        );
        assert_eq!(start_bi.direction, end_bi.direction, "确认段首尾笔必须同向");
        assert_eq!(seg.direction, end_bi.direction, "段方向应与末笔方向一致");
        if seg.direction == 1 {
            assert!(seg.start_price < seg.end_price, "向上段起点应低于终点");
        } else {
            assert!(seg.start_price > seg.end_price, "向下段起点应高于终点");
        }
    }
}

/// 段端点（203 非零点）必须是笔端点（201 非零点）的子集；
/// 除窗口首端截断点外，端点还必须是分型位置。
#[test]
fn segment_endpoints_are_bi_endpoints_and_fractals() {
    let (_, snapshot) = build();
    let bi_pts = channel(&snapshot, 201);
    let seg_pts = channel(&snapshot, 203);

    let seg_positions: Vec<usize> = seg_pts
        .iter()
        .enumerate()
        .filter(|(_, v)| v.abs() > 1e-6)
        .map(|(i, _)| i)
        .collect();
    assert!(seg_positions.len() >= 2);

    // 203 ⊆ 201（段端点都是笔端点）
    for &i in &seg_positions {
        assert!(bi_pts[i].abs() > 1e-6, "段端点 idx={i} 必须是笔端点（201）");
    }

    // 段端点都是分型位置：除窗口首端截断点（第一个段起点，其分型可能
    // 不在 czsc fx_list 里）外，101/102 分型通道在该位置应有值
    let fx_top = channel(&snapshot, 101);
    let fx_bottom = channel(&snapshot, 102);
    for &i in seg_positions.iter().skip(1) {
        assert!(
            fx_top[i].abs() > 1e-6 || fx_bottom[i].abs() > 1e-6,
            "段端点 idx={i} 必须是分型位置（101/102）"
        );
    }
}

/// 段方向通道（204）只覆盖**已确认段**，且与段方向一致。
///
/// 相邻确认段共享端点K线（后一段起点 = 前一段终点），204 与 202 笔向通道一样
/// 按顺序覆盖写值，共享端点K线取后一段的方向；因此断言区间取
/// [起点, 终点) 开区间，末段额外校验终点。
#[test]
fn seg_direction_channel_matches_segments() {
    let (_, snapshot) = build();
    let seg_dir = channel(&snapshot, 204);

    let sure: Vec<&Segment> = snapshot.segments.iter().filter(|s| s.is_sure).collect();
    assert!(!sure.is_empty(), "窗口内应有已确认段");

    let n = sure.len();
    for (k, seg) in sure.iter().enumerate() {
        let expect: f32 = if seg.direction == 1 { 1.0 } else { -1.0 };
        let lo = seg.start_index.min(seg.end_index) as usize;
        let hi = seg.start_index.max(seg.end_index) as usize;
        let hi_exclusive = if k + 1 < n { hi } else { hi + 1 };
        assert!(
            seg_dir[lo..hi_exclusive].iter().all(|v| *v == expect),
            "204 通道在确认段覆盖区间 {lo}..{hi_exclusive} 应为 {expect}"
        );
    }
}

/// 显示通道（203/204）只输出确认段：203 非零点必须是确认段端点；
/// 未确认段（is_sure=false）的端点不得出现在通道中（与确认段共享端点K线的
/// 位置除外——相邻段首尾笔相邻、分型位置相同）。
#[test]
fn display_channels_exclude_unconfirmed_segments() {
    use std::collections::HashSet;

    let (_, snapshot) = build();
    let seg_pts = channel(&snapshot, 203);

    // 确认段的端点必须出现在 203 通道
    for seg in snapshot.segments.iter().filter(|s| s.is_sure) {
        assert!(
            seg_pts[seg.start_index as usize].abs() > 1e-6,
            "确认段起点(bar {})必须在203通道",
            seg.start_index
        );
        assert!(
            seg_pts[seg.end_index as usize].abs() > 1e-6,
            "确认段终点(bar {})必须在203通道",
            seg.end_index
        );
    }
    // 未确认段的端点：不得出现在 203 通道，除非该位置恰是某确认段端点
    let sure_positions: HashSet<u32> = snapshot
        .segments
        .iter()
        .filter(|s| s.is_sure)
        .flat_map(|s| [s.start_index, s.end_index])
        .collect();
    for seg in snapshot.segments.iter().filter(|s| !s.is_sure) {
        for pos in [seg.start_index, seg.end_index] {
            if !sure_positions.contains(&pos) {
                assert!(
                    seg_pts[pos as usize].abs() <= 1e-6,
                    "未确认段端点(bar {pos})不应在203通道"
                );
            }
        }
    }
}

/// 已知结构回归：002340 日线（窗口 1000）最后一段确认的向上段
/// 2025-12-16 (7.09) -> 2026-01-26 (10.09)，其终点正是 一卖 锚点；
/// 且该段终点价格与 203 通道一致。
#[test]
fn last_sure_segment_ends_at_buy1_anchor() {
    let (dates, snapshot) = build();

    let seg = snapshot
        .segments
        .iter()
        .rfind(|s| s.is_sure)
        .expect("存在确认段");
    assert_eq!(dates[seg.end_index as usize], "2026-01-26");
    assert!((seg.end_price - 10.09).abs() < 1e-4);
    assert_eq!(seg.direction, 1, "应为向上段");
    assert_eq!(dates[seg.start_index as usize], "2025-12-16");
    assert!((seg.start_price - 7.09).abs() < 1e-4);

    // 203 通道在段终点处输出价格
    let seg_pts = channel(&snapshot, 203);
    assert!((seg_pts[seg.end_index as usize] - 10.09).abs() < 1e-3);
}

/// 段数量上限回归：同一数据下段数不应异常膨胀（旧式"每笔成段"）。
#[test]
fn segment_count_is_bounded() {
    use std::collections::HashMap;

    let (_, snapshot) = build();
    // 50 笔窗口内，特征序列分型法应给出个位数~十位数的段
    assert!(
        snapshot.segments.len() <= 20,
        "段数量应有限（当前 {}）",
        snapshot.segments.len()
    );
    // 每个K线位置最多被标记两次（作为某段终点一次 + 作为下段起点一次）
    let mut counts: HashMap<u32, u32> = HashMap::new();
    for seg in &snapshot.segments {
        *counts.entry(seg.start_index).or_insert(0) += 1;
        *counts.entry(seg.end_index).or_insert(0) += 1;
    }
    for (pos, c) in &counts {
        assert!(
            *c <= 2,
            "位置 {pos} 被标记 {c} 次（每位置最多一次起点+一次终点）"
        );
    }
}

//! 回归测试：买卖点（401~406）锚点与去重。
//!
//! 约定（见 analyzer/signals.rs）：
//! - 锚点必须落在**最终可见笔**的结束分型**极值K线**上（不在笔中途）；
//! - 同一锚点不得有多个买卖点（保留优先级最高者）；
//! - 信号先挂起、待锚定笔被确认后再提交，并过滤掉已被 max_bi_num 清理的
//!   早期笔上的点，避免"孤儿点"。
//!
//! 使用仓库内真实数据 tests/data/002340_day.csv。

use czsc_core::analyze::CZSC;
use czsc_core::objects::bar::RawBarBuilder;
use czsc_core::objects::freq::Freq;
use std::collections::HashSet;
use tdx_czsc_service::analyzer::compute_buy_sell_points;

mod common;
use common::{analyze_request, config_for, load_series};

fn load_bars(path: &str, window_start: usize) -> Vec<czsc_core::objects::bar::RawBar> {
    let csv = std::fs::read_to_string(path).expect("read csv");
    let mut bars = Vec::new();
    for (i, line) in csv.lines().skip(1).enumerate() {
        if i < window_start {
            continue;
        }
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 5 {
            continue;
        }
        let dt = chrono::NaiveDate::parse_from_str(parts[0], "%Y-%m-%d")
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
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
                .open(close)
                .close(close)
                .high(high)
                .low(low)
                .vol(0.0)
                .amount(0.0)
                .build()
                .unwrap(),
        );
    }
    bars
}

/// 最终可见笔的结束分型极值K线索引集合（与信号锚点同一套定位逻辑）。
fn final_bi_end_extremes(bars: &[czsc_core::objects::bar::RawBar]) -> HashSet<usize> {
    let czsc = CZSC::new(bars.to_vec(), 50, 6);
    let mut set = HashSet::new();
    for bi in &czsc.get_finished_bis() {
        if let Some(mid) = bi.fx_b.elements.get(1)
            && let Some(extreme) = mid.elements.iter().find(|b| b.dt == bi.fx_b.dt)
        {
            set.insert(extreme.id as usize);
        }
    }
    set
}

#[test]
fn signal_anchors_are_final_bi_endpoints_and_unique() {
    let bars = load_bars("../tests/data/002340_day.csv", 3933 - 1000);
    let points = compute_buy_sell_points(&bars, 50, 6);

    assert!(!points.is_empty(), "002340 window should produce signals");

    // 所有锚点必须是最终可见笔的结束分型极值K线（不在笔中途、无孤儿点）
    let endpoints = final_bi_end_extremes(&bars);
    let bad = points
        .iter()
        .filter(|p| !endpoints.contains(&p.index))
        .count();
    assert_eq!(bad, 0, "买卖点必须落在最终可见笔端点上（极值K线）");

    // 同一锚点不得有多个买卖点（优先级最高者保留）
    let unique: HashSet<usize> = points.iter().map(|p| p.index).collect();
    assert_eq!(unique.len(), points.len(), "同一锚点不得有多个买卖点");

    // 每个通道的信号数量不应超过旧实现的密集数量（97），防止回归到每笔端点重复触发
    assert!(
        points.len() < 97,
        "买卖点数量应显著低于旧的密集实现（97），当前 {}",
        points.len()
    );
}

/// 缠论关联：二买/二卖必须依附最近的一买/一卖，且为一买/一卖后的首个
/// 回调/反弹确认点（直到下一次 一买/一卖 重置）。因此任意两个相邻的
/// 401/402（重置点）之间，最多一个 403 和一个 404。
#[test]
fn second_bs_attached_to_first_bs() {
    let bars = load_bars("../tests/data/002340_day.csv", 3933 - 1000);
    let points = compute_buy_sell_points(&bars, 50, 6);

    let mut sorted: Vec<&tdx_czsc_service::analyzer::BuySellPoint> = points.iter().collect();
    sorted.sort_by_key(|p| p.index);

    let mut sell2_seen = false;
    let mut buy2_seen = false;
    for p in sorted {
        match p.func_id {
            401 | 402 => {
                sell2_seen = false;
                buy2_seen = false;
            }
            404 => {
                assert!(!sell2_seen, "相邻 401/402 之间最多一个二卖(404)");
                sell2_seen = true;
            }
            403 => {
                assert!(!buy2_seen, "相邻 401/402 之间最多一个二买(403)");
                buy2_seen = true;
            }
            _ => {}
        }
    }
}

/// 三买/三卖缠论严格化：依附中枢，每个中枢最多一个三买（405）和一个三卖（406）。
/// 走 build_snapshot 实际路径（严格化发生在中枢构建之后）。
#[test]
fn third_bs_capped_per_pivot() {
    use tdx_czsc_service::analyzer::{AnalysisParams, build_snapshot};
    use tdx_czsc_service::state::AppState;

    let (_, high, low, close) = load_series("../tests/data/002340_day.csv");
    let request = analyze_request(
        "002340.SZ",
        vec![401, 402, 403, 404, 405, 406],
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

    // 三买/三卖数量不应超过中枢数量（每个中枢最多一个）
    let n405 = snapshot.signals.iter().filter(|s| s.kind == 5).count();
    let n406 = snapshot.signals.iter().filter(|s| s.kind == 6).count();
    assert!(
        n405 <= snapshot.pivots.len(),
        "三买({n405})数量不应超过中枢数量({})",
        snapshot.pivots.len()
    );
    assert!(
        n406 <= snapshot.pivots.len(),
        "三卖({n406})数量不应超过中枢数量({})",
        snapshot.pivots.len()
    );
    // 每个三买/三卖锚点之前必须有已结束的中枢（依附中枢）
    for s in &snapshot.signals {
        if s.kind == 5 || s.kind == 6 {
            let has_prev_zs = snapshot
                .pivots
                .iter()
                .any(|p| (p.end_index as usize) < (s.index as usize));
            assert!(has_prev_zs, "三买/三卖必须依附于前置中枢");
        }
    }
}

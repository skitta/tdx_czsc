//! 严格派中枢真实数据回归：对象性质、308 编码、407/408 与旧通道不受影响。

use tdx_czsc_service::analyzer::{AnalysisParams, build_snapshot};
use tdx_czsc_service::proto::analysis::{AnalysisSnapshot, AnalyzeRequest};

mod common;
use common::{analyze_request_ex, config_for, load_series};

const CSV: &str = "../tests/data/002340_day.csv";
const MIN_BI_LEN: u32 = 6;

fn build(request: &AnalyzeRequest) -> AnalysisSnapshot {
    let params = AnalysisParams::from((request, &config_for(MIN_BI_LEN, 0)));
    build_snapshot(request, &params).expect("snapshot")
}

fn channel(snapshot: &AnalysisSnapshot, func_id: u32) -> &[f32] {
    &snapshot
        .channels
        .iter()
        .find(|c| c.func_id == func_id)
        .unwrap_or_else(|| panic!("channel {func_id}"))
        .values
}

#[test]
fn strict_objects_and_channels_are_consistent_on_real_data() {
    let (_, high, low, close) = load_series(CSV);
    let request = analyze_request_ex(
        vec![
            101, 102, 201, 202, 203, 204, 301, 302, 303, 304, 305, 306, 307, 308, 401, 402, 403,
            404, 405, 406, 407, 408,
        ],
        "D",
        high,
        low,
        close,
        MIN_BI_LEN,
        0,
        true,
        1000,
    );
    let snapshot = build(&request);

    assert!(!snapshot.strict_pivots.is_empty(), "严格中枢不应为空");
    for pivot in &snapshot.strict_pivots {
        assert!(pivot.upper >= pivot.lower, "upper < lower: {pivot:?}");
        assert!(pivot.gg >= pivot.upper, "gg < upper: {pivot:?}");
        assert!(pivot.dd <= pivot.lower, "dd > lower: {pivot:?}");
        assert!((1..=3).contains(&pivot.level), "非法级别: {pivot:?}");
        assert!((1..=3).contains(&pivot.kind), "非法 kind: {pivot:?}");
        assert!(pivot.move_count > 0);
        assert!(pivot.start_index <= pivot.end_index);
    }

    // 308: 0 或无中枢；否则 level*10+edge，负号表示虚中枢。
    for &value in channel(&snapshot, 308) {
        if value == 0.0 {
            continue;
        }
        let code = value.abs() as u32;
        let level = code / 10;
        let edge = code % 10;
        assert!((1..=3).contains(&level), "非法 308 code: {value}");
        assert!(edge <= 2, "非法 308 edge: {value}");
    }

    let strict_signals: Vec<_> = snapshot
        .signals
        .iter()
        .filter(|s| s.kind == 7 || s.kind == 8)
        .collect();
    for signal in strict_signals {
        assert!(signal.price > 0.0, "段级信号价格必须为正: {signal:?}");
    }
}

#[test]
fn strict_channels_do_not_change_existing_zs_channels() {
    let (_, high, low, close) = load_series(CSV);
    let mk = |ids: Vec<u32>| {
        analyze_request_ex(
            ids,
            "D",
            high[..1500].to_vec(),
            low[..1500].to_vec(),
            close[..1500].to_vec(),
            MIN_BI_LEN,
            0,
            false,
            1000,
        )
    };
    let old = build(&mk(vec![301, 302, 303, 304]));
    let new = build(&mk(vec![301, 302, 303, 304, 305, 306, 307, 308]));
    for func_id in [301, 302, 303, 304] {
        assert_eq!(
            channel(&old, func_id),
            channel(&new, func_id),
            "新增严格通道后 {func_id} 必须逐值不变"
        );
    }
}

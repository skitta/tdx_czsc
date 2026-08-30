//! 对照测试：`signals_fast`（P1 快速重实现）与 czsc-signals 原实现
//! 输出逐位一致（信号字符串逐字符比较）。
//!
//! 覆盖场景：
//! - 多个长度的演化终态（600/1500/3000/3933 根），同一 TaCache/
//!   MaSeriesCache **连续喂入**（模拟信号评估器的跨评估共享）——
//!   锁死 update_ma_cache 增量分支与 ma_snapshot_value 的位级等价；
//! - 修正/修剪路径：多状态连续喂入时 czsc 的 bars_raw 掐头（首笔
//!   确认后）会改变 id 前缀，覆盖"按 id 对齐旧值"分支。

use std::collections::HashMap;

use czsc_core::analyze::CZSC;
use czsc_core::objects::signal::Signal;
use czsc_signals::cxt::{cxt_second_bs_v230320, cxt_third_bs_v230318};
use czsc_signals::params::ParamView;
use czsc_signals::types::TaCache;
use serde_json::Value;
use tdx_czsc_service::analyzer::{MaSeriesCache, second_bs_fast, third_bs_fast};

mod common;
use common::load_raw_bars;

const CSV: &str = "../tests/data/002340_day.csv";

fn signal_strings(signals: &[Signal]) -> Vec<String> {
    signals.iter().map(|s| s.to_string()).collect()
}

/// 对给定演化终态运行"原实现 vs 快速实现"并逐字符比较信号输出。
fn assert_parity(czsc: &CZSC, orig_ta: &mut TaCache, fast_ma: &mut MaSeriesCache, ctx: &str) {
    let params = HashMap::<String, Value>::new();

    let view1 = ParamView::new(&params);
    let orig2 = cxt_second_bs_v230320(czsc, &view1, orig_ta);
    let view2 = ParamView::new(&params);
    let fast2 = second_bs_fast(czsc, &view2, fast_ma);
    assert_eq!(
        signal_strings(&orig2),
        signal_strings(&fast2),
        "{ctx}: cxt_second_bs_V230320 输出不一致"
    );

    let view3 = ParamView::new(&params);
    let orig3 = cxt_third_bs_v230318(czsc, &view3, orig_ta);
    let view4 = ParamView::new(&params);
    let fast3 = third_bs_fast(czsc, &view4, fast_ma);
    assert_eq!(
        signal_strings(&orig3),
        signal_strings(&fast3),
        "{ctx}: cxt_third_bs_V230318 输出不一致"
    );
}

#[test]
fn second_and_third_bs_parity_across_growing_states() {
    let all = load_raw_bars(CSV, 0);
    // 长度递增的状态序列：同一 TaCache/MaSeriesCache 连续喂入，
    // 覆盖 update_ma_cache 的 need_init / 增量 / 修剪分支
    let mut orig_ta = TaCache::new();
    let mut fast_ma = MaSeriesCache::new();

    for len in [600usize, 1500, 3000, 3933] {
        let czsc = CZSC::new(all[..len].to_vec(), 1000, 6);
        assert_parity(&czsc, &mut orig_ta, &mut fast_ma, &format!("len={len}"));
    }
}

/// 末笔价格原地修正（同 dt 更新）后重跑对照：快照重算路径
/// （ma_snapshot_value 的 close 不一致分支）。
#[test]
fn parity_after_last_bar_price_update() {
    let mut all = load_raw_bars(CSV, 0);
    all.truncate(1500);
    all[1499].close += 0.15;
    all[1499].high = all[1499].close + 0.1;

    let czsc = CZSC::new(all, 1000, 6);
    let mut orig_ta = TaCache::new();
    let mut fast_ma = MaSeriesCache::new();
    assert_parity(
        &czsc,
        &mut orig_ta,
        &mut fast_ma,
        "末笔原地修正(快照重算路径)",
    );
}

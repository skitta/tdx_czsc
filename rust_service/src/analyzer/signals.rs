//! 401~406 买卖点信号计算。
//!
//! 复用 czsc-signals `cxt` 模块的官方信号函数（一买/一卖为标准
//! BUY1/SELL1 信号，二/三类为均线辅助判定）。逐根 K 线演化
//! CZSC 并评估信号，记录每个信号首次出现的笔端点，使历史买卖点
//! 都能在图上标注出来。
//!
//! **量能数据来源**（一买/一卖的"量能力度"维度）：
//! 量能由 `build_raw_bars` 按三级来源链填充（见 `analyzer/mod.rs`）：
//! ① 请求携带的真实 vol（公式 950 量能通道融合 / CSV 工具链）；
//! ② vol 缺失时按 `AppConfig::derive_volume_proxy` 派生振幅代理量
//!    `(high-low)/close×100`（通达信公式未挂 950 通道时的兜底，
//!    恢复"价格背驰 AND（量能 OR 长度）"完整判定）；
//! ③ 全关时为 0（量能背驰恒不成立）。
//!
//! 位置与去重约定：
//! - 锚点取笔端分型的**极值K线**（`dt == fx.dt`），与分型/笔/中枢通道
//!   对齐，避免画到包含合并组的起点而落在笔的中途；
//! - 信号出现时先**挂起**，待锚定笔被后续新笔确认（不再是最末笔、未被
//!   重构）后才提交——czsc 演化中最后完成的笔可能被新 K 线破坏/合并，
//!   直接记录会把点锚在临时笔端点上，最终图上落在可见笔的中途；
//! - 信号是"状态型"分类（当前笔结构满足即返回），同一信号段（连续保持
//!   同一命中）只记录一次（命中值变化才挂起）；
//! - **缠论关联**：二买/二卖必须依附最近的一买/一卖，且为一买/一卖后的
//!   首个回调/反弹确认点；**类二买/类二卖**（二类与三类合并）——买侧
//!   （403+405）、卖侧（404+406）在同一个 一买/一卖 依附结构内各自最多
//!   记录一个点，三买/三卖因此不再堆叠；直到下一次 一买/一卖 出现才重置；
//! - 同一锚点上多个信号同时命中时，只保留优先级最高的一个（func_id 最小，
//!   一买 > 一卖 > 二买 > 二卖 > 三买 > 三卖）。

use czsc_core::analyze::CZSC;
use czsc_core::objects::bar::RawBar;
use czsc_core::objects::signal::Signal;
use czsc_signals::cxt::{
    cxt_first_buy_v221126, cxt_first_sell_v221126, cxt_second_bs_v230320, cxt_third_bs_v230318,
};
use czsc_signals::params::ParamView;
use czsc_signals::types::TaCache;
use serde_json::Value;
use std::collections::HashMap;

use crate::proto::analysis::SignalKind;

/// 一条已确认的买卖点。
#[derive(Debug)]
pub struct BuySellPoint {
    /// 对应 401~406 通道号
    pub func_id: u32,
    pub kind: SignalKind,
    /// 标注 bar 索引（最新完成笔结束分型的极值K线）
    pub index: usize,
    /// 分型价格
    pub price: f64,
    /// 完整信号串，如 `30分钟_D1B_BUY1_一买_13笔_任意_0`
    pub label: String,
}

/// 待确认的买卖点：信号出现时先挂起，待锚定笔被后续新笔确认（不再是最末笔、
/// 未被重构）后再提交。czsc 演化中最后完成的笔可能被新 K 线破坏/合并而重构，
/// 若直接记录会把点锚在临时笔端点上，最终图上落在可见笔的中途。
struct PendingPoint {
    spec_idx: usize,
    func_id: u32,
    kind: SignalKind,
    anchor: Anchor,
    label: String,
}

type SignalFn = fn(&CZSC, &ParamView, &mut TaCache) -> Vec<Signal>;

struct SignalSpec {
    func_id: u32,
    kind: SignalKind,
    hit_value: &'static str,
    func: SignalFn,
}

const SIGNAL_SPECS: [SignalSpec; 6] = [
    SignalSpec {
        func_id: 401,
        kind: SignalKind::SignalBuy1,
        hit_value: "一买",
        func: cxt_first_buy_v221126,
    },
    SignalSpec {
        func_id: 402,
        kind: SignalKind::SignalSell1,
        hit_value: "一卖",
        func: cxt_first_sell_v221126,
    },
    SignalSpec {
        func_id: 403,
        kind: SignalKind::SignalBuy2,
        hit_value: "二买",
        func: cxt_second_bs_v230320,
    },
    SignalSpec {
        func_id: 404,
        kind: SignalKind::SignalSell2,
        hit_value: "二卖",
        func: cxt_second_bs_v230320,
    },
    SignalSpec {
        func_id: 405,
        kind: SignalKind::SignalBuy3,
        hit_value: "三买",
        func: cxt_third_bs_v230318,
    },
    SignalSpec {
        func_id: 406,
        kind: SignalKind::SignalSell3,
        hit_value: "三卖",
        func: cxt_third_bs_v230318,
    },
];

/// 逐根 K 线演化 CZSC 并评估全部买卖点信号。
///
/// 信号函数共用同一个 TaCache，保证均线等中间结果一致。
///
/// 性能：所有信号函数只依赖已完成笔序列（bi_list），笔结构不变时
/// 信号状态不会变化，因此只在笔结构发生变化（数量或最后一笔端点
/// 变化）的 K 线上重新评估，避免对每根 K 线都跑 6 个信号函数。
///
/// 位置与去重约定：
/// - 锚点取笔端分型的**极值K线**（`dt == fx.dt`），与分型/笔/中枢通道对齐；
/// - 信号出现时先**挂起**，待锚定笔被后续新笔确认（不再是最末笔、未被重构）
///   后才提交——避免锚定在演化中被破坏/合并的临时笔端点（图上落在笔中途）；
/// - 信号为状态型分类，同一信号段（连续保持同一命中）只标记一次；
/// - 同一锚点上多个信号同时命中时，只保留优先级最高的一个（func_id 最小，
///   一买 > 一卖 > 二买 > 二卖 > 三买 > 三卖）。
/// 缠论买卖点关联状态：
/// - 二买/二卖（403/404）必须依附最近的一买/一卖，且为一买/一卖后的**首个**
///   回调/反弹确认点（每个 一买/一卖 结构内最多一个）；
/// - 三买/三卖（405/406）另由 `strictize_third_bs_by_zs` 按中枢去重
///   （每个中枢最多一个三买或三卖）。
struct BuySellState {
    last_buy1: Option<usize>,
    last_sell1: Option<usize>,
    last_buy2: Option<usize>,
    last_sell2: Option<usize>,
}

impl Default for BuySellState {
    fn default() -> Self {
        Self {
            last_buy1: None,
            last_sell1: None,
            last_buy2: None,
            last_sell2: None,
        }
    }
}

impl BuySellState {
    /// 缠论严格化：二买/二卖仅在依附最近一买/一卖且为首个确认点时允许记录。
    fn commit_allowed(&self, func_id: u32, anchor: usize) -> bool {
        match func_id {
            // 二买：最近一买之后、无更近一卖、且该一买后尚未记录二买
            403 => match self.last_buy1 {
                Some(b1) => {
                    anchor > b1
                        && self.last_sell1.map_or(true, |s| s < b1)
                        && self.last_buy2.map_or(true, |b2| b2 < b1)
                }
                None => false,
            },
            // 二卖：最近一卖之后、无更近一买、且该一卖后尚未记录二卖
            404 => match self.last_sell1 {
                Some(s1) => {
                    anchor > s1
                        && self.last_buy1.map_or(true, |b| b < s1)
                        && self.last_sell2.map_or(true, |s2| s2 < s1)
                }
                None => false,
            },
            _ => true,
        }
    }

    fn record(&mut self, func_id: u32, anchor: usize) {
        match func_id {
            401 => self.last_buy1 = Some(anchor),
            402 => self.last_sell1 = Some(anchor),
            403 => self.last_buy2 = Some(anchor),
            404 => self.last_sell2 = Some(anchor),
            _ => {}
        }
    }
}

pub fn compute_buy_sell_points(
    bars: &[RawBar],
    max_bi_num: usize,
    min_bi_len: usize,
) -> Vec<BuySellPoint> {
    if bars.is_empty() {
        return Vec::new();
    }

    let mut czsc = CZSC::new(vec![bars[0].clone()], max_bi_num, min_bi_len);
    let mut cache = TaCache::new();
    let params = HashMap::<String, Value>::new();
    let view = ParamView::new(&params);
    let mut points = Vec::new();
    let mut pending: Vec<PendingPoint> = Vec::new();
    // 每个通道上一次评估时的命中值：命中值变化（从无到有或换类型）才挂起新信号
    let mut prev_hit: [Option<&'static str>; SIGNAL_SPECS.len()] = [None; SIGNAL_SPECS.len()];
    let mut last_bi_state: Option<(usize, i32)> = None;
    let mut bs_state = BuySellState::default();

    let mut evaluate = |czsc: &CZSC| {
        let state = bi_state_fingerprint(czsc);
        if last_bi_state == Some(state) {
            return;
        }
        last_bi_state = Some(state);

        // 1) 提交上一轮挂起的信号：锚定笔仍存在于 bi_list（已被新笔确认）才记录；
        //    否则说明锚定笔在演化中被重构/合并，丢弃该点并复位命中状态，
        //    让同一信号段可在后续稳定笔端点上重新挂起。
        //    二买/二卖还需通过缠论关联校验（依附最近一买/一卖、且为首个确认点）。
        for p in pending.drain(..) {
            let anchor_stable = czsc
                .bi_list
                .iter()
                .any(|bi| super::objects::fx_mid_index(&bi.fx_b) == Some(p.anchor.index));
            if anchor_stable {
                if bs_state.commit_allowed(p.func_id, p.anchor.index) {
                    bs_state.record(p.func_id, p.anchor.index);
                    points.push(BuySellPoint {
                        func_id: p.func_id,
                        kind: p.kind,
                        index: p.anchor.index,
                        price: p.anchor.price,
                        label: p.label,
                    });
                }
            } else {
                prev_hit[p.spec_idx] = None;
            }
        }

        // 2) 评估当前信号：命中值相对上一次评估变化时挂起（同一信号段不重复挂起）
        for (i, spec) in SIGNAL_SPECS.iter().enumerate() {
            let signal = (spec.func)(czsc, &view, &mut cache)
                .into_iter()
                .find(|s| signal_hits(s, spec.hit_value));
            let current_hit = signal.as_ref().map(|_| spec.hit_value);
            if current_hit == prev_hit[i] {
                continue;
            }
            prev_hit[i] = current_hit;
            let Some(signal) = signal else {
                continue;
            };
            let Some(anchor) = latest_bi_end_anchor(czsc) else {
                continue;
            };
            pending.push(PendingPoint {
                spec_idx: i,
                func_id: spec.func_id,
                kind: spec.kind,
                anchor,
                label: signal.to_string(),
            });
        }
    };

    evaluate(&czsc);
    for bar in &bars[1..] {
        czsc.update_bar(bar.clone());
        evaluate(&czsc);
    }

    // 末尾：提交仍挂起的信号（锚定在最终笔结构上，天然稳定），
    // 二买/二卖同样需通过缠论关联校验。
    for p in pending {
        if bs_state.commit_allowed(p.func_id, p.anchor.index) {
            bs_state.record(p.func_id, p.anchor.index);
            points.push(BuySellPoint {
                func_id: p.func_id,
                kind: p.kind,
                index: p.anchor.index,
                price: p.anchor.price,
                label: p.label,
            });
        }
    }

    // 只保留锚定在最终可见笔端点上的点：演化早期形成的笔可能因 max_bi_num
    // 上限被清理（图上无对应笔结构），其买卖点一并丢弃，避免成为"孤儿点"。
    let final_bis = czsc.get_finished_bis();
    points.retain(|p| {
        final_bis
            .iter()
            .any(|bi| super::objects::fx_mid_index(&bi.fx_b) == Some(p.index))
    });

    // 同一锚点上多个信号同时命中时，只保留优先级最高的一个（func_id 最小）。
    let mut best_by_anchor: Vec<BuySellPoint> = Vec::new();
    for point in points {
        if let Some(existing) = best_by_anchor.iter_mut().find(|p| p.index == point.index) {
            if point.func_id < existing.func_id {
                *existing = point;
            }
        } else {
            best_by_anchor.push(point);
        }
    }
    best_by_anchor.sort_by_key(|p| p.index);
    best_by_anchor
}

/// 笔结构指纹：(笔数量, 最后一笔端点分型中间 K 线 id)。
/// 两者都不变时已完成笔序列完全一致，信号不会变化。
fn bi_state_fingerprint(czsc: &CZSC) -> (usize, i32) {
    let last_fx_b_id = czsc
        .bi_list
        .last()
        .and_then(|bi| bi.fx_b.elements.get(1))
        .map(|bar| bar.id)
        .unwrap_or(-1);
    (czsc.bi_list.len(), last_fx_b_id)
}

fn signal_hits(signal: &Signal, hit_value: &str) -> bool {
    signal.value().split('_').next() == Some(hit_value)
}

struct Anchor {
    index: usize,
    price: f64,
}

/// 最新完成笔的结束分型锚点。
///
/// 位置与分型/笔/中枢通道对齐：取分型**极值K线**（`dt == fx.dt`）所在的
/// 索引，而不是 `fx_b.elements[1].id`（包含合并组的起点），否则买卖点会
/// 画在笔端分型的合并组起点、看起来落在笔的中途。
fn latest_bi_end_anchor(czsc: &CZSC) -> Option<Anchor> {
    let bi = czsc.bi_list.last()?;
    let index = super::objects::fx_mid_index(&bi.fx_b)?;
    Some(Anchor {
        index,
        price: bi.fx_b.fx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_hits_matches_first_value_part() {
        let s: Signal = "30分钟_D1B_BUY1_一买_13笔_任意_0"
            .parse()
            .expect("valid signal string");
        assert!(signal_hits(&s, "一买"));
        assert!(!signal_hits(&s, "一卖"));
    }

    #[test]
    fn specs_cover_all_signal_channels() {
        let func_ids: Vec<u32> = SIGNAL_SPECS.iter().map(|s| s.func_id).collect();
        assert_eq!(func_ids, vec![401, 402, 403, 404, 405, 406]);
    }
}

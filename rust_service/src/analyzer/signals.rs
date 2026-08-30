//! 401~406 买卖点信号计算。
//!
//! 复用 czsc-signals `cxt` 模块的官方信号函数（一买/一卖为标准
//! BUY1/SELL1 信号，二/三类为均线辅助判定）。逐根 K 线演化
//! CZSC 并评估信号，记录每个信号首次出现的笔端点，使历史买卖点
//! 都能在图上标注出来。
//!
//! **性能（P0 单遍化）**：演化完成的 CZSC 终态与 `CZSC::new(bars)`
//! 完全一致（czsc-core 的 `new` 内部即逐根 `update_bar`），因此本函数
//! 直接返回演化终态，供结构通道（分型/笔/中枢/段）复用——同一窗口
//! 只跑一遍逐 bar 演化，不再做第二遍全量扫描。
//!
//! **为什么不做"最终态一次性评估"**：czsc-signals 的 4 个信号函数
//! （`cxt_first_buy/sell`、`cxt_second/third_bs`）都只检查 `bi_list`
//! **尾部窗口**（最近 5/21/19/... 笔）。若只在最终状态评估一次，
//! 只能发现最新笔结构当前的信号，历史买卖点（演化过程中曾命中的）
//! 会全部消失。逐根演化是保留全部历史买卖点的必要方式。
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
use czsc_signals::cxt::{cxt_first_buy_v221126, cxt_first_sell_v221126};
use czsc_signals::params::ParamView;
use czsc_signals::types::TaCache;
use serde_json::Value;
use std::collections::HashMap;

use super::signals_fast::{MaSeriesCache, second_bs_fast, third_bs_fast};
use crate::proto::analysis::SignalKind;

/// 一条已确认的买卖点。
#[derive(Debug, Clone)]
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

/// 信号函数分派：原实现（first_buy/sell，纯数学，无缓存依赖）与
/// P1 快速重实现（second/third 均线辅助，见 `signals_fast.rs`，输出与
/// czsc-signals 原实现位级一致，对照测试锁定）。
enum SpecFunc {
    Original(fn(&CZSC, &ParamView, &mut TaCache) -> Vec<Signal>),
    Fast(fn(&CZSC, &ParamView, &mut MaSeriesCache) -> Vec<Signal>),
}

struct SignalSpec {
    func_id: u32,
    kind: SignalKind,
    hit_value: &'static str,
    func: SpecFunc,
}

const SIGNAL_SPECS: [SignalSpec; 6] = [
    SignalSpec {
        func_id: 401,
        kind: SignalKind::SignalBuy1,
        hit_value: "一买",
        func: SpecFunc::Original(cxt_first_buy_v221126),
    },
    SignalSpec {
        func_id: 402,
        kind: SignalKind::SignalSell1,
        hit_value: "一卖",
        func: SpecFunc::Original(cxt_first_sell_v221126),
    },
    SignalSpec {
        func_id: 403,
        kind: SignalKind::SignalBuy2,
        hit_value: "二买",
        func: SpecFunc::Fast(second_bs_fast),
    },
    SignalSpec {
        func_id: 404,
        kind: SignalKind::SignalSell2,
        hit_value: "二卖",
        func: SpecFunc::Fast(second_bs_fast),
    },
    SignalSpec {
        func_id: 405,
        kind: SignalKind::SignalBuy3,
        hit_value: "三买",
        func: SpecFunc::Fast(third_bs_fast),
    },
    SignalSpec {
        func_id: 406,
        kind: SignalKind::SignalSell3,
        hit_value: "三卖",
        func: SpecFunc::Fast(third_bs_fast),
    },
];

/// 逐根 K 线演化 CZSC 并评估全部买卖点信号。
///
/// 返回 `(演化完成的 CZSC, 确认的买卖点)`：演化终态与
/// `CZSC::new(bars)` 等价，调用方应复用它作为结构通道输入
/// （见模块文档"P0 单遍化"）。`bars` 必须非空（由调用方保证）。
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
///
/// 缠论买卖点关联状态：
/// - 二买/二卖（403/404）必须依附最近的一买/一卖，且为一买/一卖后的**首个**
///   回调/反弹确认点（每个 一买/一卖 结构内最多一个）；
/// - 三买/三卖（405/406）另由 `strictize_third_bs_by_zs` 按中枢去重
///   （每个中枢最多一个三买或三卖）。
#[derive(Default)]
struct BuySellState {
    last_buy1: Option<usize>,
    last_sell1: Option<usize>,
    last_buy2: Option<usize>,
    last_sell2: Option<usize>,
}

impl BuySellState {
    /// 缠论严格化：二买/二卖仅在依附最近一买/一卖且为首个确认点时允许记录。
    fn commit_allowed(&self, func_id: u32, anchor: usize) -> bool {
        match func_id {
            // 二买：最近一买之后、无更近一卖、且该一买后尚未记录二买
            403 => match self.last_buy1 {
                Some(b1) => {
                    anchor > b1
                        && self.last_sell1.is_none_or(|s| s < b1)
                        && self.last_buy2.is_none_or(|b2| b2 < b1)
                }
                None => false,
            },
            // 二卖：最近一卖之后、无更近一买、且该一卖后尚未记录二卖
            404 => match self.last_sell1 {
                Some(s1) => {
                    anchor > s1
                        && self.last_buy1.is_none_or(|b| b < s1)
                        && self.last_sell2.is_none_or(|s2| s2 < s1)
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

/// 信号评估器：持有与 CZSC 演化过程同步的评估状态（挂起/提交、命中去重、
/// 缠论关联、TaCache）。
///
/// 增量路径复用：新 K 线到来时先 `czsc.update_bar` 再 `observe(&czsc)`，
/// 演化序列与全量 `compute_buy_sell_points` 逐 bar 完全一致（见
/// `evolve_bars`）；`finalize_points` 产出当前确认的买卖点，评估器保留
/// 在状态中供后续演化继续，避免全窗口重放。
pub(crate) struct SignalEvaluator {
    cache: TaCache,
    /// P1 快速均线缓存（second/third 信号重实现使用）
    ma_fast: MaSeriesCache,
    params: HashMap<String, Value>,
    points: Vec<BuySellPoint>,
    pending: Vec<PendingPoint>,
    // 每个通道上一次评估时的命中值：命中值变化（从无到有或换类型）才挂起新信号
    prev_hit: [Option<&'static str>; SIGNAL_SPECS.len()],
    last_bi_state: Option<(usize, i32)>,
    bs_state: BuySellState,
}

impl SignalEvaluator {
    pub(crate) fn new() -> Self {
        Self {
            cache: TaCache::new(),
            ma_fast: MaSeriesCache::new(),
            params: HashMap::<String, Value>::new(),
            points: Vec::new(),
            pending: Vec::new(),
            prev_hit: [None; SIGNAL_SPECS.len()],
            last_bi_state: None,
            bs_state: BuySellState::default(),
        }
    }

    /// CZSC 演化一步（`update_bar`）后调用：按笔结构指纹去重评估。
    /// 笔结构不变时跳过全部信号函数（避免对每根 K 线都跑 6 个函数）。
    pub(crate) fn observe(&mut self, czsc: &CZSC) {
        let state = bi_state_fingerprint(czsc);
        if self.last_bi_state == Some(state) {
            return;
        }
        self.last_bi_state = Some(state);

        let view = ParamView::new(&self.params);

        // 1) 提交上一轮挂起的信号：锚定笔仍存在于 bi_list（已被新笔确认）才记录；
        //    否则说明锚定笔在演化中被重构/合并，丢弃该点并复位命中状态，
        //    让同一信号段可在后续稳定笔端点上重新挂起。
        //    二买/二卖还需通过缠论关联校验（依附最近一买/一卖、且为首个确认点）。
        for p in self.pending.drain(..) {
            let anchor_stable = czsc
                .bi_list
                .iter()
                .any(|bi| super::objects::fx_mid_index(&bi.fx_b) == Some(p.anchor.index));
            if anchor_stable {
                if self.bs_state.commit_allowed(p.func_id, p.anchor.index) {
                    self.bs_state.record(p.func_id, p.anchor.index);
                    self.points.push(BuySellPoint {
                        func_id: p.func_id,
                        kind: p.kind,
                        index: p.anchor.index,
                        price: p.anchor.price,
                        label: p.label,
                    });
                }
            } else {
                self.prev_hit[p.spec_idx] = None;
            }
        }

        // 2) 评估当前信号：命中值相对上一次评估变化时挂起（同一信号段不重复挂起）
        for (i, spec) in SIGNAL_SPECS.iter().enumerate() {
            let signal = match spec.func {
                SpecFunc::Original(func) => func(czsc, &view, &mut self.cache)
                    .into_iter()
                    .find(|s| signal_hits(s, spec.hit_value)),
                SpecFunc::Fast(func) => func(czsc, &view, &mut self.ma_fast)
                    .into_iter()
                    .find(|s| signal_hits(s, spec.hit_value)),
            };
            let current_hit = signal.as_ref().map(|_| spec.hit_value);
            if current_hit == self.prev_hit[i] {
                continue;
            }
            self.prev_hit[i] = current_hit;
            let Some(signal) = signal else {
                continue;
            };
            let Some(anchor) = latest_bi_end_anchor(czsc) else {
                continue;
            };
            self.pending.push(PendingPoint {
                spec_idx: i,
                func_id: spec.func_id,
                kind: spec.kind,
                anchor,
                label: signal.to_string(),
            });
        }
    }

    /// 产出当前确认的买卖点（幂等）：末尾提交挂起信号、孤儿点过滤、同锚点去重。
    ///
    /// 与全量路径的收尾一致；评估器内部状态保持可继续演化（后续 observe 从
    /// 已提交状态继续）。
    pub(crate) fn finalize_points(&mut self, czsc: &CZSC) -> Vec<BuySellPoint> {
        // 末尾：提交仍挂起的信号（锚定在最终笔结构上，天然稳定），
        // 二买/二卖同样需通过缠论关联校验。
        for p in self.pending.drain(..) {
            if self.bs_state.commit_allowed(p.func_id, p.anchor.index) {
                self.bs_state.record(p.func_id, p.anchor.index);
                self.points.push(BuySellPoint {
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
        // （评估器内保留原始点集，后续 observe 继续演化，一致性由最终过滤保证。）
        let mut points = self.points.clone();
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
}

/// 全量演化核心：逐根演化 CZSC，并在需要时同步信号评估器。
///
/// 增量路径与全量路径共用同一演化序列：增量只对新增 K 线继续
/// `update_bar + observe`（见 `incremental.rs`），演化终态与全量一致。
/// `bars` 必须非空（由调用方保证）。
pub(crate) fn evolve_bars(
    bars: &[RawBar],
    max_bi_num: usize,
    min_bi_len: usize,
    wants_signals: bool,
) -> (CZSC, Option<SignalEvaluator>) {
    evolve_bars_profiled(bars, max_bi_num, min_bi_len, wants_signals, None)
}

/// `evolve_bars` 的剖面版（P1-1）：分离"逐根 update_bar"与"信号评估
/// observe"两段耗时，供 `SnapshotProfile` 细分（profile=None 时零开销，
/// 行为与 `evolve_bars` 完全一致）。
pub(crate) fn evolve_bars_profiled(
    bars: &[RawBar],
    max_bi_num: usize,
    min_bi_len: usize,
    wants_signals: bool,
    mut profile: Option<&mut super::SnapshotProfile>,
) -> (CZSC, Option<SignalEvaluator>) {
    let tracking = profile.is_some();
    let mut czsc = CZSC::new(vec![bars[0].clone()], max_bi_num, min_bi_len);
    let mut evaluator = wants_signals.then(SignalEvaluator::new);
    let t0 = std::time::Instant::now();
    if let Some(ev) = evaluator.as_mut() {
        ev.observe(&czsc);
    }
    if let Some(p) = profile.as_deref_mut() {
        p.evolve_observe += t0.elapsed();
    }
    for bar in &bars[1..] {
        let t1 = tracking.then(std::time::Instant::now);
        czsc.update_bar(bar.clone());
        if let (Some(p), Some(t1)) = (profile.as_deref_mut(), t1) {
            p.evolve_update += t1.elapsed();
        }
        let t2 = tracking.then(std::time::Instant::now);
        if let Some(ev) = evaluator.as_mut() {
            ev.observe(&czsc);
        }
        if let (Some(p), Some(t2)) = (profile.as_deref_mut(), t2) {
            p.evolve_observe += t2.elapsed();
        }
    }
    (czsc, evaluator)
}

pub fn compute_buy_sell_points(
    bars: &[RawBar],
    max_bi_num: usize,
    min_bi_len: usize,
) -> (CZSC, Vec<BuySellPoint>) {
    assert!(
        !bars.is_empty(),
        "compute_buy_sell_points requires non-empty bars"
    );
    let (czsc, mut evaluator) = evolve_bars(bars, max_bi_num, min_bi_len, true);
    let points = evaluator
        .as_mut()
        .expect("wants_signals=true must create evaluator")
        .finalize_points(&czsc);
    (czsc, points)
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

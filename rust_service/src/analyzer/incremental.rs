//! 增量分析（enable_incremental）：缓存已演化的 CZSC 与信号评估器，
//! 新请求的 K 线序列与缓存前缀一致时，只对增量 K 线继续演化并复用
//! 演化状态装配快照，避免全窗口重放（稳态刷新 O(新增 K 线)）。
//!
//! 适用条件（窗口首部稳定）：
//! - **全量窗口**（`data_len <= analysis_window`）：序列增长时窗口整体
//!   前移为新前缀的超集，逐根追加演化；
//! - **同长度末笔原地更新**（盘中刷新）：前缀不变、末笔同 dt 值变化，
//!   走 czsc `update_bar` 的"同 dt 延伸"分支。
//!
//! 固定尾窗口（`data_len > analysis_window`）下，序列增长会导致分析
//! 窗口整体平移（丢掉最旧 K 线），缓存序列不再是新窗口的前缀——
//! `accept` 判定不适用并返回 `NotApplicable`，调用方回退全量分析。
//!
//! 并发模型：缓存条目由调用方"取出 → 演化 → 放回"。两个请求并发取出
//! 同一条目时，后取者得到 `None` 走全量；先后放回的顺序不影响正确性
//! （`accept` 按"缺失即补"全量追加，且放入总是完整的新状态）。

use std::sync::{Mutex, PoisonError};

use czsc_core::analyze::CZSC;
use czsc_core::objects::bar::RawBar;

use super::signals::{BuySellPoint, SignalEvaluator};
use crate::lru::Lru;

/// 单根 K 线一致性判定（构造自同一 `RawBarFactory` 逻辑，f32→f64
/// 转换位级一致，逐字段精确比较即可）。
///
/// 外部 `RawBar` 未实现 `PartialEq`，比较语义由本 trait 承载，
/// 避免模块级自由比较函数。
pub(crate) trait BarIdentity {
    fn same_bar(&self, other: &Self) -> bool;
}

impl BarIdentity for RawBar {
    fn same_bar(&self, other: &Self) -> bool {
        self.dt == other.dt
            && self.open == other.open
            && self.high == other.high
            && self.low == other.low
            && self.close == other.close
            && self.vol == other.vol
            && self.amount == other.amount
    }
}

/// 增量承接结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AcceptOutcome {
    /// 已按新 K 线增量演化完成，可直接装配快照。
    Served,
    /// 序列不兼容（无缓存/窗口缩短/前缀不一致/末笔时间错位/评估器缺失），
    /// 需要全量分析；被拒绝的状态未被修改，仍可放回缓存。
    NotApplicable,
}

/// 增量分析状态：窗口 K 线（前缀校验用）+ 已演化的 CZSC + 可选的信号评估器。
///
/// **为什么自持 `bars`**：czsc-core 会按 Python 语义修剪 `czsc.bars_raw`
/// （掐头到第一笔起点 / 随笔数上限收缩），不能作为前缀比对基准；
/// 自持一份完整窗口 K 线（内存约每窗口 ×100B/根）。
pub(crate) struct IncrementalState {
    bars: Vec<RawBar>,
    pub(crate) czsc: CZSC,
    pub(crate) evaluator: Option<SignalEvaluator>,
}

impl IncrementalState {
    pub(crate) fn new(bars: Vec<RawBar>, czsc: CZSC, evaluator: Option<SignalEvaluator>) -> Self {
        Self {
            bars,
            czsc,
            evaluator,
        }
    }

    /// 以新窗口 K 线增量承接：
    ///
    /// 1. 需要信号但缓存评估器缺失（此前只算结构）→ 无法承接；
    /// 2. 窗口缩短 / 前缀不一致 → 无法承接（状态不变）；
    /// 3. 追加模式（新窗口更长）：逐根 `update_bar + observe`；
    /// 4. 原地模式（等长且末笔同 dt）：单次"同 dt 延伸"更新。
    ///
    /// 评估器存在时**无条件**随演化 observe——即使当前请求只要结构，
    /// 也要保持评估器与 CZSC 同步，供后续信号请求继续承接。
    pub(crate) fn accept(&mut self, new_bars: &[RawBar], wants_signals: bool) -> AcceptOutcome {
        if wants_signals && self.evaluator.is_none() {
            return AcceptOutcome::NotApplicable;
        }

        let old = &self.bars;
        let old_len = old.len();
        let new_len = new_bars.len();
        if old_len == 0 || new_len < old_len {
            return AcceptOutcome::NotApplicable;
        }

        let in_place_last = new_len == old_len;
        let compare_len = if in_place_last { old_len - 1 } else { old_len };
        if !new_bars[..compare_len]
            .iter()
            .zip(&old[..compare_len])
            .all(|(n, o)| n.same_bar(o))
        {
            return AcceptOutcome::NotApplicable;
        }

        if in_place_last {
            // 末笔原地更新：要求同 dt（czsc 走"同 dt 延伸"分支）；
            // dt 不同视为序列替换，回退全量。
            if new_bars[new_len - 1].dt != old[old_len - 1].dt {
                return AcceptOutcome::NotApplicable;
            }
            // 完全相同的序列：无需演化（快照缓存被逐出后仍可复用装配）。
            if new_bars[new_len - 1].same_bar(&old[old_len - 1]) {
                return AcceptOutcome::Served;
            }
            self.bars
                .last_mut()
                .unwrap()
                .clone_from(&new_bars[new_len - 1]);
            self.czsc.update_bar(new_bars[new_len - 1].clone());
            if let Some(ev) = self.evaluator.as_mut() {
                ev.observe(&self.czsc);
            }
            return AcceptOutcome::Served;
        }

        self.bars.extend_from_slice(&new_bars[old_len..]);
        for bar in &new_bars[old_len..] {
            self.czsc.update_bar(bar.clone());
            if let Some(ev) = self.evaluator.as_mut() {
                ev.observe(&self.czsc);
            }
        }
        AcceptOutcome::Served
    }

    /// 产出当前确认的买卖点（评估器缺失时为空的快照输入）。
    pub(crate) fn signals(&mut self) -> Vec<BuySellPoint> {
        let czsc = &self.czsc;
        self.evaluator
            .as_mut()
            .map(|ev| ev.finalize_points(czsc))
            .unwrap_or_default()
    }
}

/// 增量状态缓存键：与序列无关的分析参数子集。
/// 前缀一致性在匹配时逐 K 线校验（见 `take_matching`）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct IncrementalKey {
    pub freq_hint: String,
    pub max_bi_num: u32,
    pub min_bi_len: u32,
    pub analysis_window: u32,
}

/// 增量状态 LRU 缓存。
///
/// 条目持有演化中的 CZSC（含完整窗口 K 线），容量需按内存敏感配置
/// （`AppConfig::incremental_cache_capacity`，默认 8）。
pub(crate) struct IncrementalCache {
    entries: Lru<IncrementalKey, IncrementalState>,
}

impl std::fmt::Debug for IncrementalCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 条目内含不可 Debug 的演化状态（CZSC/信号评估器），只输出概要。
        f.debug_struct("IncrementalCache")
            .field("len", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl IncrementalCache {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: Lru::new(capacity),
        }
    }

    /// 取出并移除指定键的条目（供调用方演化后放回）。
    pub(crate) fn take(&mut self, key: &IncrementalKey) -> Option<IncrementalState> {
        self.entries.remove(key)
    }

    pub(crate) fn put(&mut self, key: IncrementalKey, state: IncrementalState) {
        self.entries.insert(key, state);
    }
}

/// 增量状态缓存的所有者：把锁限制在短暂的 `take/put` 区间内。
pub(crate) struct IncrementalStore {
    cache: Mutex<IncrementalCache>,
}

impl std::fmt::Debug for IncrementalStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncrementalStore")
            .field("cache", &self.cache)
            .finish_non_exhaustive()
    }
}

impl IncrementalStore {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            cache: Mutex::new(IncrementalCache::new(capacity)),
        }
    }

    pub(crate) fn lease(&self, key: IncrementalKey) -> IncrementalLease<'_> {
        let state = self
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take(&key);
        IncrementalLease {
            store: self,
            key,
            state,
        }
    }
}

/// 一次增量承接的租约：演化期间不持锁，成功 `commit` 才写回。
#[must_use = "未 commit 的 IncrementalLease 会丢弃增量状态"]
pub(crate) struct IncrementalLease<'a> {
    store: &'a IncrementalStore,
    key: IncrementalKey,
    state: Option<IncrementalState>,
}

impl<'a> IncrementalLease<'a> {
    #[cfg(test)]
    pub(crate) fn state(&self) -> Option<&IncrementalState> {
        self.state.as_ref()
    }

    pub(crate) fn state_mut(&mut self) -> Option<&mut IncrementalState> {
        self.state.as_mut()
    }

    pub(crate) fn replace(&mut self, state: IncrementalState) {
        self.state = Some(state);
    }

    pub(crate) fn commit(mut self) {
        let Some(state) = self.state.take() else {
            return;
        };
        self.store
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .put(self.key.clone(), state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::RawBarFactory;
    use crate::analyzer::fixtures::sample_request;
    use czsc_core::objects::freq::Freq;

    fn sample_bars() -> Vec<RawBar> {
        let request = sample_request(true);
        RawBarFactory::new(&request, true)
            .build(Freq::F30, 0)
            .expect("bars should build")
    }

    fn evolve(bars: &[RawBar], wants_signals: bool) -> (CZSC, Option<SignalEvaluator>) {
        let outcome = super::super::signals::EvolutionEngine::new(bars, 50, 3, wants_signals).run();
        (outcome.czsc, outcome.evaluator)
    }

    fn full_points(bars: &[RawBar]) -> (CZSC, Vec<BuySellPoint>) {
        let (czsc, mut ev) = evolve(bars, true);
        let points = ev.as_mut().expect("evaluator").finalize_points(&czsc);
        (czsc, points)
    }

    /// 增量逐块演化与全量演化输出一致（含信号）。
    #[test]
    fn accept_append_matches_full_evolution() {
        let bars = sample_bars();
        let (full_czsc, full_points) = full_points(&bars);

        let (czsc, evaluator) = evolve(&bars[..20], true);
        let mut state = IncrementalState::new(bars[..20].to_vec(), czsc, evaluator);
        for end in (20..bars.len() + 9).step_by(9) {
            let end = end.min(bars.len());
            assert_eq!(
                state.accept(&bars[..end], true),
                AcceptOutcome::Served,
                "append chunk to {end} must be served"
            );
        }

        assert_eq!(state.bars.len(), bars.len());
        assert_eq!(state.czsc.bi_list.len(), full_czsc.bi_list.len());
        let inc_points = state.signals();
        assert_eq!(inc_points.len(), full_points.len());
        for (a, b) in inc_points.iter().zip(&full_points) {
            assert_eq!(a.func_id, b.func_id);
            assert_eq!(a.index, b.index);
            assert_eq!(a.label, b.label);
        }
    }

    /// 结构-only 的承接步骤也必须保持评估器同步（供后续信号请求承接）。
    #[test]
    fn structure_only_steps_keep_evaluator_fresh() {
        let bars = sample_bars();
        let (_, full_points) = full_points(&bars);

        let (czsc, evaluator) = evolve(&bars[..20], true);
        let mut state = IncrementalState::new(bars[..20].to_vec(), czsc, evaluator);
        // 交替：结构-only（wants_signals=false）与信号请求
        for (end, wants) in [(30, false), (40, true), (50, false), (bars.len(), true)] {
            assert_eq!(state.accept(&bars[..end], wants), AcceptOutcome::Served);
        }
        let inc_points = state.signals();
        assert_eq!(inc_points.len(), full_points.len());
        assert!(
            inc_points
                .iter()
                .zip(&full_points)
                .all(|(a, b)| a.index == b.index && a.func_id == b.func_id)
        );
    }

    /// 等长末笔原地更新（同 dt 值变化）可承接，且与全量一致。
    #[test]
    fn in_place_last_bar_update_matches_full() {
        let bars = sample_bars();
        let mut perturbed = bars.clone();
        let last = perturbed.last_mut().unwrap();
        last.close += 0.3;
        last.high = last.close + 0.05;

        let (_, full_points) = full_points(&perturbed);

        let (czsc, evaluator) = evolve(&bars, true);
        let mut state = IncrementalState::new(bars.to_vec(), czsc, evaluator);
        assert_eq!(state.accept(&perturbed, true), AcceptOutcome::Served);
        assert_eq!(state.signals().len(), full_points.len());
    }

    /// 末笔 dt 不同的等长序列视为替换，拒绝承接。
    #[test]
    fn in_place_rejects_different_last_dt() {
        let bars = sample_bars();
        let mut replaced = bars.clone();
        let last = replaced.last_mut().unwrap();
        last.dt += chrono::Duration::days(1);

        let (czsc, evaluator) = evolve(&bars, true);
        let state = IncrementalState::new(bars.to_vec(), czsc, evaluator);
        let mut s = state;
        assert_eq!(s.accept(&replaced, true), AcceptOutcome::NotApplicable);
    }

    /// 前缀不一致（历史数据修正）/ 窗口缩短 → 拒绝且状态不变。
    #[test]
    fn mismatched_prefix_or_shorter_window_rejected() {
        let bars = sample_bars();
        let mut modified = bars.clone();
        modified[10].close += 0.5;

        let (czsc, evaluator) = evolve(&bars, true);
        let state = IncrementalState::new(bars.to_vec(), czsc, evaluator);
        let mut s = state;
        let bars_before = s.bars.len();
        assert_eq!(s.accept(&modified, true), AcceptOutcome::NotApplicable);
        assert_eq!(
            s.accept(&bars[..bars.len() - 5], true),
            AcceptOutcome::NotApplicable
        );
        assert_eq!(s.bars.len(), bars_before, "拒绝时状态不得被修改");
    }

    /// 需要信号但评估器缺失 → 拒绝（后续全量会重建带评估器的状态）。
    #[test]
    fn signals_request_without_evaluator_rejected() {
        let bars = sample_bars();
        let (czsc, evaluator) = evolve(&bars, false);
        assert!(evaluator.is_none());
        let mut state = IncrementalState::new(bars.to_vec(), czsc, evaluator);
        assert_eq!(state.accept(&bars, true), AcceptOutcome::NotApplicable);
        assert_eq!(state.accept(&bars, false), AcceptOutcome::Served);
    }

    /// LRU 容量与取放语义。
    #[test]
    fn cache_take_put_lru() {
        let bars = sample_bars();
        let mut cache = IncrementalCache::new(2);
        let key = |n: u32| IncrementalKey {
            freq_hint: "F30".to_string(),
            max_bi_num: 50,
            min_bi_len: 3,
            analysis_window: n,
        };
        let mk = |_n: u32| {
            let (czsc, evaluator) = evolve(&bars, false);
            IncrementalState::new(bars.to_vec(), czsc, evaluator)
        };
        cache.put(key(1), mk(1));
        cache.put(key(2), mk(2));
        cache.put(key(3), mk(3)); // 淘汰 key(1)
        assert!(cache.take(&key(1)).is_none());
        assert!(cache.take(&key(2)).is_some());
        assert!(cache.take(&key(2)).is_none(), "取出后条目移除");
        cache.put(key(2), mk(2));
        assert!(cache.take(&key(2)).is_some());
    }

    #[test]
    fn lease_commit_persists_and_drop_discards() {
        let bars = sample_bars();
        let store = IncrementalStore::new(2);
        let key = IncrementalKey {
            freq_hint: "F30".to_string(),
            max_bi_num: 50,
            min_bi_len: 3,
            analysis_window: 0,
        };
        let mk_state = || {
            let (czsc, evaluator) = evolve(&bars, false);
            IncrementalState::new(bars.to_vec(), czsc, evaluator)
        };

        let mut lease = store.lease(key.clone());
        assert!(lease.state().is_none());
        lease.replace(mk_state());
        lease.commit();

        let taken = store.lease(key.clone());
        assert!(taken.state().is_some());
        drop(taken);
        assert!(
            store.lease(key).state().is_none(),
            "未 commit 的租约必须丢弃状态"
        );
    }

    #[test]
    fn store_recovers_from_poisoned_lock() {
        let store = IncrementalStore::new(1);
        let key = IncrementalKey {
            freq_hint: "F30".to_string(),
            max_bi_num: 50,
            min_bi_len: 3,
            analysis_window: 0,
        };

        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = store.cache.lock().expect("lock");
            panic!("poison incremental store");
        }));
        assert!(poisoned.is_err());
        assert!(store.lease(key).state().is_none());
    }
}

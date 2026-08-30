//! 快速信号路径（P1）：`cxt_second_bs_V230320` / `cxt_third_bs_V230318`
//! 的 O(log N) 重实现。
//!
//! # 动机（剖面实测）
//! 信号评估占全量分析约 87%（3933 根日线 227ms/262ms 演化段）。原
//! czsc-signals 实现每次评估都：
//!
//! - `bar_index_map` 重建 O(N) 的 `id → idx` HashMap；
//! - `update_ma_cache` 增量分支重建 O(N) 的"id → 旧 MA"映射并对全序列
//!   拷贝，再重算尾部窗口。
//!
//! 本实现以 **O(1) 追加判定 + O(log N) 二分定位 + 尾部窗口重算** 替代。
//!
//! # 位级一致性保证（对照测试 `signals_fast_parity` 锁定）
//!
//! - `calc_sma_cache_style` 的 TA-Lib 累计顺序（`sum -= old; sum += new`）
//!   与**无** 4 位小数舍入——逐字复刻（`ma_cache_style_sma`）；
//! - `update_ma_cache` 的增量语义：旧值按 id 对齐到新序列、尾部
//!   `min(timeperiod+10, len)` 窗口整段重算并覆盖最后 5 值；
//! - `ma_snapshot_value` 的快照重算语义：同 id close 不一致时以"仅替换
//!   该 id close"的序列重算 MA（窗口按位置顺序求和，加法顺序与滚动
//!   累计一致→位级一致），并按 id 缓存本轮结果；
//! - 信号字符串经官方 `make_kline_signal_v1` 构造，格式逐位一致。
//!
//! 仅实现 SMA 路径（默认 `ma_type=SMA`；当前调用方参数恒为空）。

use std::collections::HashMap;

use czsc_core::analyze::CZSC;
use czsc_core::objects::direction::Direction;
use czsc_core::objects::signal::Signal;
use czsc_signals::params::ParamView;
use czsc_signals::utils::cxt::fx_raw_bars;
use czsc_signals::utils::sig::{get_sub_elements, get_usize_param, make_kline_signal_v1};

/// 单条 MA 缓存（对齐 czsc-signals `TaCache` 的 `series`/`series_ids`）。
struct MaSeries {
    /// 缓存生成时的 bars id 序列（czsc `bars_raw` 的 id 单调递增）
    ids: Vec<i32>,
    values: Vec<f64>,
}

/// MA 缓存 + 单轮快照重算缓存（对齐 `ma_snapshot_value` 的 overrides）。
pub struct MaSeriesCache {
    series: HashMap<(String, usize), MaSeries>,
    snapshot_overrides: HashMap<i32, f64>,
}

impl MaSeriesCache {
    pub fn new() -> Self {
        Self {
            series: HashMap::new(),
            snapshot_overrides: HashMap::new(),
        }
    }
}

impl Default for MaSeriesCache {
    fn default() -> Self {
        Self::new()
    }
}

impl MaSeriesCache {
    /// 对齐 `update_ma_cache`（czsc-signals `utils/ta.rs`）：
    /// 1. need_init（无缓存/长度不足/penultimate id 追不上）→ 全量重算；
    /// 2. 追加/同长（前缀 id 不变）→ O(1) 对齐旧值 + 尾部窗口整段重算、
    ///    覆盖最后 5 值（与原实现逐位一致）；
    /// 3. 修正/修剪（id 前缀变化）→ 按 id 映射旧值（罕见路径，保持正确）。
    fn ma_series(&mut self, czsc: &CZSC, ma_type: &str, timeperiod: usize) -> Option<&Vec<f64>> {
        let key = (ma_type.to_uppercase(), timeperiod);
        let now_len = czsc.bars_raw.len();
        if now_len == 0 {
            return None;
        }
        let bar_ids: Vec<i32> = czsc.bars_raw.iter().map(|b| b.id).collect();

        let need_init = match self.series.get(&key) {
            None => true,
            Some(existing) => {
                now_len < timeperiod + 15
                    || existing.ids.is_empty()
                    || !ids_contains(&existing.ids, bar_ids[now_len - 2])
            }
        };
        if need_init {
            let close: Vec<f64> = czsc.bars_raw.iter().map(|b| b.close).collect();
            let values = ma_cache_style_sma(&close, timeperiod);
            self.series.insert(
                key.clone(),
                MaSeries {
                    ids: bar_ids,
                    values,
                },
            );
            return self.series.get(&key).map(|s| &s.values);
        }

        // 增量：前缀 id 不变判定（O(1)）——追加或同长
        let existing = self.series.get_mut(&key)?;
        let prefix_same = bar_ids.len() >= existing.ids.len()
            && existing
                .ids
                .last()
                .is_some_and(|last| bar_ids.get(existing.ids.len() - 1) == Some(last));
        let mut res = if prefix_same {
            // 追加/同长：旧值直接对齐（无需 O(N) 映射拷贝）
            let mut res = existing.values.clone();
            res.resize(bar_ids.len(), f64::NAN);
            res
        } else {
            // 修正/修剪（bars_raw 被 czsc 掐头）：按 id 映射旧值（罕见）
            let old_map: HashMap<i32, usize> = existing
                .ids
                .iter()
                .enumerate()
                .map(|(i, id)| (*id, i))
                .collect();
            let mut res = Vec::with_capacity(bar_ids.len());
            for id in &bar_ids {
                res.push(
                    old_map
                        .get(id)
                        .map(|i| existing.values[*i])
                        .unwrap_or(f64::NAN),
                );
            }
            res
        };

        // 尾部 `min(timeperiod+10, len)` 窗口整段重算，覆盖最后 5 值
        let window_size = (timeperiod + 10).min(now_len);
        let window_start = now_len - window_size;
        let close: Vec<f64> = czsc.bars_raw[window_start..]
            .iter()
            .map(|b| b.close)
            .collect();
        let partial = ma_cache_style_sma(&close, timeperiod);
        for i in 1..=5.min(window_size) {
            res[now_len - i] = partial[window_size - i];
        }

        *existing = MaSeries {
            ids: bar_ids,
            values: res,
        };
        Some(&existing.values)
    }

    /// 均值读取入口：增量维护 MA 序列后按 id 二分取值（O(log N)）。
    fn ma_at(&mut self, czsc: &CZSC, ma_type: &str, timeperiod: usize, bar_id: i32) -> Option<f64> {
        let values = self.ma_series(czsc, ma_type, timeperiod)?;
        let idx = locate_bar(czsc, bar_id)?;
        values.get(idx).copied()
    }

    /// 对齐 `ma_snapshot_value` 的快照读取入口：同 id close 与当前序列
    /// 一致 → 直接取缓存值；不一致 → 以"仅替换该 id close"的序列重算
    /// 该点 MA（窗口按位置顺序求和，加法顺序与滚动累计一致→位级一致），
    /// 并缓存本轮结果。
    fn snapshot_at(
        &mut self,
        czsc: &CZSC,
        ma_type: &str,
        timeperiod: usize,
        bar_id: i32,
        snapshot_close: f64,
    ) -> Option<f64> {
        let idx = locate_bar(czsc, bar_id)?;
        {
            let values = self.ma_series(czsc, ma_type, timeperiod)?;
            let base = *values.get(idx)?;
            let current_close = czsc.bars_raw.get(idx)?.close;
            if (current_close - snapshot_close).abs() <= f64::EPSILON {
                return Some(base);
            }
        }
        if let Some(value) = self.snapshot_overrides.get(&bar_id) {
            return Some(*value);
        }

        // 快照重算：窗口内以快照 close 替换后按位置顺序求和（位级一致）
        let value = if idx + 1 < timeperiod {
            f64::NAN
        } else {
            let window = idx + 1 - timeperiod..=idx;
            let sum: f64 = window
                .map(|i| {
                    if i == idx {
                        snapshot_close
                    } else {
                        czsc.bars_raw[i].close
                    }
                })
                .sum();
            sum / timeperiod as f64
        };
        self.snapshot_overrides.insert(bar_id, value);
        Some(value)
    }
}

/// 对齐 `calc_sma_cache_style`（TA-Lib 累计顺序、无 4 位舍入）。
/// 必须与 czsc-signals `utils/ta.rs` 逐位一致（浮点加/减顺序影响边界位）。
fn ma_cache_style_sma(series: &[f64], n: usize) -> Vec<f64> {
    let len = series.len();
    let mut out = vec![f64::NAN; len];
    if len < n || n == 0 {
        return out;
    }
    let mut sum = series[..n].iter().sum::<f64>();
    out[n - 1] = sum / n as f64;
    for i in n..len {
        sum -= series[i - n];
        sum += series[i];
        out[i] = sum / n as f64;
    }
    out
}

/// `bar.id → bars_raw 下标` 的 O(log N) 定位（id 单调递增）。
fn locate_bar(czsc: &CZSC, id: i32) -> Option<usize> {
    let bars = &czsc.bars_raw;
    let idx = bars.partition_point(|b| b.id < id);
    (idx < bars.len() && bars[idx].id == id).then_some(idx)
}

/// 升序 id 序列的成员判定（O(log N)；对齐原实现 `contains` 的 O(N) 语义，
/// 位级等价——id 序列单调递增）。
fn ids_contains(ids: &[i32], id: i32) -> bool {
    let idx = ids.partition_point(|x| *x < id);
    idx < ids.len() && ids[idx] == id
}

/// 快速实现：`cxt_second_bs_V230320`（均线辅助二买/二卖，末 5 笔）。
pub fn second_bs_fast(c: &CZSC, params: &ParamView, ma: &mut MaSeriesCache) -> Vec<Signal> {
    let di = get_usize_param(params, "di", 1);
    let timeperiod = get_usize_param(params, "timeperiod", 21);
    let ma_type = params.str("ma_type", "SMA").to_uppercase();
    let k1 = c.freq.to_string();
    let k2 = format!("D{}#{}#{}", di, ma_type, timeperiod);
    let k3 = "BS2辅助V230320";
    let mut v1 = "其他";
    if c.bi_list.len() < di + 6 {
        return make_kline_signal_v1(&k1, &k2, k3, v1);
    }

    let bis = get_sub_elements(&c.bi_list, di, 5);
    if bis.len() != 5 {
        return make_kline_signal_v1(&k1, &k2, k3, v1);
    }
    let b1 = &bis[0];
    let b3 = &bis[2];
    let b5 = &bis[4];
    let b1_fx_bars = fx_raw_bars(&b1.fx_b);
    let b3_fx_bars = fx_raw_bars(&b3.fx_b);
    let b5_fx_a_bars = fx_raw_bars(&b5.fx_a);
    let b5_fx_b_bars = fx_raw_bars(&b5.fx_b);
    if b1_fx_bars.len() < 2
        || b3_fx_bars.len() < 2
        || b5_fx_a_bars.len() < 2
        || b5_fx_b_bars.len() < 2
    {
        return make_kline_signal_v1(&k1, &k2, k3, v1);
    }

    let Some(b1_ma_b) = ma.ma_at(c, &ma_type, timeperiod, b1_fx_bars[b1_fx_bars.len() - 2].id)
    else {
        return make_kline_signal_v1(&k1, &k2, k3, v1);
    };
    let Some(b3_ma_b) = ma.ma_at(c, &ma_type, timeperiod, b3_fx_bars[b3_fx_bars.len() - 2].id)
    else {
        return make_kline_signal_v1(&k1, &k2, k3, v1);
    };
    let Some(b5_ma_a) = ma.ma_at(
        c,
        &ma_type,
        timeperiod,
        b5_fx_a_bars[b5_fx_a_bars.len() - 2].id,
    ) else {
        return make_kline_signal_v1(&k1, &k2, k3, v1);
    };
    let Some(b5_ma_b) = ma.ma_at(
        c,
        &ma_type,
        timeperiod,
        b5_fx_b_bars[b5_fx_b_bars.len() - 2].id,
    ) else {
        return make_kline_signal_v1(&k1, &k2, k3, v1);
    };

    let lc1 = b1.get_low() < b1_ma_b && b3.get_low() < b3_ma_b;
    if b5.direction == Direction::Down && lc1 && b5_ma_a < b5_ma_b {
        v1 = "二买";
    }

    let sc1 = b1.get_high() > b1_ma_b && b3.get_high() > b3_ma_b;
    if b5.direction == Direction::Up && sc1 && b5_ma_a > b5_ma_b {
        v1 = "二卖";
    }
    make_kline_signal_v1(&k1, &k2, k3, v1)
}

/// 快速实现：`cxt_third_bs_V230318`（均线辅助三买/三卖，末 5 笔中枢）。
pub fn third_bs_fast(c: &CZSC, params: &ParamView, ma: &mut MaSeriesCache) -> Vec<Signal> {
    let di = get_usize_param(params, "di", 1);
    let timeperiod = get_usize_param(params, "timeperiod", 34);
    let ma_type = params.str("ma_type", "SMA").to_uppercase();
    let k1 = c.freq.to_string();
    let k2 = format!("D{}#{}#{}", di, ma_type, timeperiod);
    let k3 = "BS3辅助V230318";
    let mut v1 = "其他";

    if c.bi_list.len() < di + 6 {
        return make_kline_signal_v1(&k1, &k2, k3, v1);
    }
    let bis = get_sub_elements(&c.bi_list, di, 5);
    if bis.len() != 5 {
        return make_kline_signal_v1(&k1, &k2, k3, v1);
    }
    let b1 = &bis[0];
    let b3 = &bis[2];
    let b5 = &bis[4];
    let zs_zd = b1.get_low().max(b3.get_low());
    let zs_zg = b1.get_high().min(b3.get_high());
    if zs_zd > zs_zg {
        return make_kline_signal_v1(&k1, &k2, k3, v1);
    }

    let b1_fx = fx_raw_bars(&b1.fx_b);
    let b3_fx = fx_raw_bars(&b3.fx_b);
    let b5_fx = fx_raw_bars(&b5.fx_b);
    if b1_fx.is_empty() || b3_fx.is_empty() || b5_fx.is_empty() {
        return make_kline_signal_v1(&k1, &k2, k3, v1);
    }

    let Some(ma_1) = ma.snapshot_at(
        c,
        &ma_type,
        timeperiod,
        b1_fx[b1_fx.len() - 1].id,
        b1_fx[b1_fx.len() - 1].close,
    ) else {
        return make_kline_signal_v1(&k1, &k2, k3, v1);
    };
    let Some(ma_3) = ma.snapshot_at(
        c,
        &ma_type,
        timeperiod,
        b3_fx[b3_fx.len() - 1].id,
        b3_fx[b3_fx.len() - 1].close,
    ) else {
        return make_kline_signal_v1(&k1, &k2, k3, v1);
    };
    let Some(ma_5) = ma.snapshot_at(
        c,
        &ma_type,
        timeperiod,
        b5_fx[b5_fx.len() - 1].id,
        b5_fx[b5_fx.len() - 1].close,
    ) else {
        return make_kline_signal_v1(&k1, &k2, k3, v1);
    };

    if b5.direction == Direction::Down && b5.get_low() > zs_zg && ma_5 > ma_3 && ma_3 > ma_1 {
        v1 = "三买";
    }
    if b5.direction == Direction::Up && b5.get_high() < zs_zd && ma_5 < ma_3 && ma_3 < ma_1 {
        v1 = "三卖";
    }
    make_kline_signal_v1(&k1, &k2, k3, v1)
}

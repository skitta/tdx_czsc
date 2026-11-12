//! 回归测试：买点 ATR 动态止盈止损线的"影子实现"。
//!
//! 公式模板 `tdx_formula_examples/atr_tp_sl.tn6` 在通达信公式层实现
//! ATR 动态止盈/止损规则，但公式层无法离线单测；这里用 Rust 把同一套
//! 规则落地（影子实现），在真实行情 `tests/data/002340_day.csv` 上验证
//! 公式必须满足的不变量（这些不变量即"终止绘画/只上移/每类独立"语义的
//! 可执行规格）：
//!
//! - A1：止损线在其段内只上移（HHV 递推等价 → 严格单调不减）；
//! - A1b：止盈线同样只上移（动态目标价 = HHV(前高 + K2×ATR)，位于价格
//!   上方、随每次创新高跳升上移，不是横线也不是价格下方的保护线）；
//! - A2：活段（未触发）内止盈线始终高于止损线（上止盈下止损，不交叉）；
//! - A3：终止语义与公式的 `段内已触发 = COUNT(触发,区间)>0` 完全等价：
//!   每段至多一个首次穿线bar f，[b, f) 无任何触发（线继续画），
//!   [f, 下一同类买点) 段内已触发恒真（终止绘画）；买入 bar 自身不参与
//!   穿线判定（`距>0` 门控），保证线从买入处可见；
//! - A4：首次穿线bar f 确实满足穿线条件（止损下穿 / 止盈上穿，
//!   影线或收盘两种触发方式）；
//! - A5：一买/二买/三买**各自独立成段、互不合并**（合成行情验证）——
//!   一买段未终止时二买出现，一买段跨越二买 bar 继续存续，三类可并存。
//!
//! 每类买点单独运行同一套规则（与模板 `一买段/二买段/三买段` 三块对应）。

use tdx_czsc_service::analyzer::{AnalysisParams, SnapshotAnalyzer};
use tdx_czsc_service::state::AppState;

mod common;
use common::{analyze_request, config_for, load_series};

/// 与 `atr_tp_sl.tn6` 顶部参数区一一对应的配置。
#[derive(Clone, Copy)]
struct TpSlConfig {
    /// ATR 均值周期 N
    atr_period: usize,
    /// K1：止损倍数（止损线在"买入以来最高"下方 K1 个 ATR）
    stop_mult: f64,
    /// K2：止盈倍数（止盈线在"买入以来前高"上方 K2 个 ATR）
    tp_mult: f64,
    /// 1=影线破位(高/低价穿线) 2=收盘破位
    trigger_mode: u8,
}

impl Default for TpSlConfig {
    fn default() -> Self {
        Self {
            atr_period: 14,
            stop_mult: 2.0,
            tp_mult: 3.0,
            trigger_mode: 1,
        }
    }
}

/// 影子实现的逐 bar 结果（与公式逐行对应）。
struct Series {
    sl: Vec<f64>,
    tp: Vec<f64>,
    trigger: Vec<bool>,
    /// 每根 bar 所属该类买点段起点（None=此前无该类买点）
    buy_at: Vec<Option<usize>>,
    /// 每根 bar 所属段的首次穿线 bar：未触发段为 None，
    /// 触发后（含触发 bar 本身）为 Some(f)，直到下一同类买点重开
    terminated_at: Vec<Option<usize>>,
}

/// 复刻公式 `atr_tp_sl.tn6` 中"单类买点段"的逐 bar 计算。
fn run_shadow(high: &[f32], low: &[f32], close: &[f32], buy: &[f32], cfg: &TpSlConfig) -> Series {
    let n = high.len();
    assert_eq!(low.len(), n);
    assert_eq!(close.len(), n);
    assert_eq!(buy.len(), n);

    // ATR：TR 的窗口均值；t < N 时取 [0..=t] 累计均值（通达信 MA 从首个有效值起）
    let mut tr = vec![0.0; n];
    for t in 0..n {
        let prev_c = if t == 0 { close[0] } else { close[t - 1] };
        tr[t] = (high[t] as f64 - low[t] as f64)
            .max((high[t] as f64 - prev_c as f64).abs())
            .max((low[t] as f64 - prev_c as f64).abs());
    }
    let atr: Vec<f64> = (0..n)
        .map(|t| {
            let window_start = t + 1 - cfg.atr_period.min(t + 1);
            let win = &tr[window_start..=t];
            win.iter().sum::<f64>() / win.len() as f64
        })
        .collect();

    // last_buy[t]：距 t 最近的该类买点 bar（含 t 自身）
    let last_buy: Vec<Option<usize>> = {
        let mut out = vec![None; n];
        let mut nearest: Option<usize> = None;
        for t in 0..n {
            if buy[t] > 0.0 {
                nearest = Some(t);
            }
            out[t] = nearest;
        }
        out
    };

    let mut sl = vec![0.0; n];
    let mut tp = vec![0.0; n];
    let mut trigger = vec![false; n];
    let mut buy_at = vec![None; n];
    let mut terminated_at = vec![None; n];

    // 段内递推状态：当前段起点、首次穿线bar、HHV 运行最大值
    let mut seg_start: Option<usize> = None;
    let mut first_trigger: Option<usize> = None;
    let mut run_max_sl = f64::NEG_INFINITY;
    let mut run_max_tp = f64::NEG_INFINITY;
    let mut run_max_high = f64::NEG_INFINITY; // 买入以来最高(含当前bar)

    for t in 0..n {
        match last_buy[t] {
            None => {
                seg_start = None;
                first_trigger = None;
                run_max_sl = f64::NEG_INFINITY;
                run_max_tp = f64::NEG_INFINITY;
                run_max_high = f64::NEG_INFINITY;
            }
            Some(b) => {
                if seg_start != Some(b) {
                    // 新段（买入bar）：重置段内状态，HHV 窗口从 b 起
                    seg_start = Some(b);
                    first_trigger = None;
                    run_max_sl = high[b] as f64 - cfg.stop_mult * atr[b];
                    run_max_tp = high[b] as f64 + cfg.tp_mult * atr[b];
                    run_max_high = high[b] as f64;
                }
                // 止盈基准 = 买入以来最高(不含当前bar)：买入bar 用自身 HIGH
                let tp_base = if t == b { high[b] as f64 } else { run_max_high };
                run_max_high = run_max_high.max(high[t] as f64);
                run_max_sl = run_max_sl.max(high[t] as f64 - cfg.stop_mult * atr[t]);
                run_max_tp = run_max_tp.max(tp_base + cfg.tp_mult * atr[t]);

                // 止损线 = HHV(高 - K1*ATR, 买入以来)（价格下方，下穿离场）
                sl[t] = run_max_sl;
                // 止盈线 = HHV(前高 + K2*ATR, 买入以来)（价格上方，上穿离场）
                tp[t] = run_max_tp;

                // 穿线判定：止损下穿 / 止盈上穿；买入 bar 自身不参与（距>0 门控）
                if t > b {
                    let cross_sl = match cfg.trigger_mode {
                        1 => (low[t] as f64) < sl[t],
                        _ => (close[t] as f64) < sl[t],
                    };
                    let cross_tp = match cfg.trigger_mode {
                        1 => (high[t] as f64) > tp[t],
                        _ => (close[t] as f64) > tp[t],
                    };
                    trigger[t] = cross_sl || cross_tp;
                    if trigger[t] && first_trigger.is_none() {
                        first_trigger = Some(t);
                    }
                }

                buy_at[t] = Some(b);
                terminated_at[t] = first_trigger.filter(|&f| t >= f);
            }
        }
    }

    Series {
        sl,
        tp,
        trigger,
        buy_at,
        terminated_at,
    }
}

/// 对单类买点影子结果断言不变量（A1/A1b/A2/A3/A4）。
fn assert_invariants(series: &Series, high: &[f32], low: &[f32], close: &[f32], cfg: &TpSlConfig) {
    let n = series.sl.len();
    assert_eq!(n, series.tp.len());

    // A1/A1b：同一段内 止损线与止盈线都只上移（单调不减）
    for t in 1..n {
        if series.buy_at[t].is_some() && series.buy_at[t] == series.buy_at[t - 1] {
            assert!(
                series.sl[t] >= series.sl[t - 1] - 1e-6,
                "止损线段内回落在 {t}: {} -> {}",
                series.sl[t - 1],
                series.sl[t]
            );
            assert!(
                series.tp[t] >= series.tp[t - 1] - 1e-6,
                "止盈线段内回落在 {t}: {} -> {}",
                series.tp[t - 1],
                series.tp[t]
            );
        }
    }

    // A2：活段（未终止）内止盈线必须高于止损线（K2<K1 保证不交叉）
    for t in 0..n {
        if series.buy_at[t].is_some() && series.terminated_at[t].is_none() {
            assert!(
                series.tp[t] > series.sl[t] + 1e-6,
                "活段内止盈应高于止损, {t}: tp={} sl={}",
                series.tp[t],
                series.sl[t]
            );
        }
    }

    // A3 + A4：逐段校验终止语义与首次穿线的真实性。
    // 触发事件本身可以在首次穿线后重复出现（线已终止不画，公式的
    // `段内已触发` 恒真），要求的是"首次穿线bar 唯一"：
    //   [b, f) 无触发、无线终止；[f, 下一同类买点) 段内已触发恒真。
    let mut seg_trigger_count = 0usize;
    let mut seg_index = 0usize;
    while seg_index < n {
        let Some(b) = series.buy_at[seg_index] else {
            seg_index += 1;
            continue;
        };
        // 段尾：下一个同类买点 bar 或数据末尾
        let mut end = n;
        for t in (b + 1)..n {
            if series.buy_at[t] != Some(b) {
                end = t;
                break;
            }
        }
        let triggers: Vec<usize> = (b..end).filter(|&t| series.trigger[t]).collect();
        match triggers.first() {
            None => {
                for t in b..end {
                    assert!(
                        series.terminated_at[t].is_none(),
                        "未触发段不应有终止标记, {t}"
                    );
                }
            }
            Some(&f) => {
                assert!(f > b, "首次穿线bar {f} 必须晚于买入bar {b}(买入bar不判定)");
                for t in b..f {
                    assert!(
                        series.terminated_at[t].is_none() && !series.trigger[t],
                        "首次穿线bar {f} 之前不得有触发/终止, {t}"
                    );
                }
                for t in f..end {
                    assert_eq!(
                        series.terminated_at[t],
                        Some(f),
                        "首次穿线bar {f} 之后段内已触发应恒真(终止绘画), {t}"
                    );
                }
                assert!(
                    triggers.iter().all(|&t| t >= f),
                    "全部触发事件必须位于首次穿线bar {f} 之后(含), 实际 {triggers:?}"
                );
                // A4：首次穿线bar确实满足对应穿线条件（止损下穿 / 止盈上穿）
                let cross_sl = match cfg.trigger_mode {
                    1 => (low[f] as f64) < series.sl[f],
                    _ => (close[f] as f64) < series.sl[f],
                };
                let cross_tp = match cfg.trigger_mode {
                    1 => (high[f] as f64) > series.tp[f],
                    _ => (close[f] as f64) > series.tp[f],
                };
                assert!(
                    cross_sl || cross_tp,
                    "首次穿线bar {f} 必须满足穿线条件(按当前参数配置)"
                );
                seg_trigger_count += 1;
            }
        }
        seg_index = end;
    }

    // 真实数据必须覆盖"终止绘画"路径（存在至少一个已终止段）
    assert!(
        seg_trigger_count > 0,
        "真实行情上应存在至少一个已终止段(终止绘画路径覆盖)"
    );
}

/// A5：三类买点独立成段、互不合并（合成行情验证）。
///
/// 真实行情上段存活期短（默认参数下数根内即触发终止），难以天然出现
/// 跨类并存样本；改用**可控的平坦低波动合成行情**：一买段未触发时
/// 二买/三买相继出现，验证一买段跨越二买 bar 继续画（不被截断、不合并），
/// 且三类段可同时存续。
#[test]
fn atr_tp_sl_types_keep_independent_segments_on_synthetic_data() {
    // 平坦低波动行情: HIGH=10.02 LOW=9.98 CLOSE=10, TR=0.04 恒值
    // → ATR=0.04; 无新高 → 前高=10.02;
    //   止盈线=HHV(前高+3×0.04)=10.14 > HIGH(10.02) 不破盈(上穿不成立);
    //   止损线=HHV(10.02-2×0.04)=9.94 < LOW(9.98) 不破损(下穿不成立)
    // → 段永不触发, 适合验证段的独立性
    let n = 60usize;
    let high = vec![10.02f32; n];
    let low = vec![9.98f32; n];
    let close = vec![10.0f32; n];
    let anchor = |bar: usize| -> Vec<f32> {
        let mut buy = vec![0.0f32; n];
        buy[bar] = 10.0;
        buy
    };

    let cfg = TpSlConfig::default();
    let b1 = anchor(5);
    let b2 = anchor(15);
    let b3 = anchor(25);
    let s1 = run_shadow(&high, &low, &close, &b1, &cfg);
    let s2 = run_shadow(&high, &low, &close, &b2, &cfg);
    let s3 = run_shadow(&high, &low, &close, &b3, &cfg);

    // ① 平坦行情下三类段都活到数据末尾（未被触发终止）
    for (name, s) in [("一买", &s1), ("二买", &s2), ("三买", &s3)] {
        assert!(
            s.terminated_at.iter().all(|f| f.is_none()),
            "{name} 段在合成平坦行情上不应被触发"
        );
        let expect_tp = 10.02 + 3.0 * 0.04;
        assert!(
            (s.tp.last().copied().unwrap_or(0.0) - expect_tp).abs() < 1e-3,
            "{name} 止盈线应为 HHV(前高+3×ATR)≈{expect_tp}"
        );
    }

    // ② 一买段跨越二买/三买 bar 继续存续（不被新买点截断 = 不合并）
    for t in 15..n {
        assert_eq!(
            s1.buy_at[t],
            Some(5),
            "一买段在二买出现后应继续以自身起点存续, {t}"
        );
        assert!(s1.terminated_at[t].is_none(), "一买段不应被二买截断, {t}");
    }

    // ③ 三类段同时存续（各画各的线）
    let coexist: usize = (15..n)
        .filter(|&t| {
            s1.buy_at[t].is_some()
                && s1.terminated_at[t].is_none()
                && s2.buy_at[t].is_some()
                && s2.terminated_at[t].is_none()
                && s3.buy_at[t].is_some()
                && s3.terminated_at[t].is_none()
        })
        .count();
    assert!(coexist > 0, "三类买点段应同时存续(各画各的线)");
}

/// 分析窗口内的行情与三类买点锚点序列。
struct WindowData {
    high: Vec<f32>,
    low: Vec<f32>,
    close: Vec<f32>,
    buy1: Vec<f32>,
    buy2: Vec<f32>,
    buy3: Vec<f32>,
}

/// 真实数据上验证默认参数（影线破位）：三类各自独立。
#[test]
fn atr_tp_sl_default_config_invariants_on_real_data() {
    let w = load_window_with_buy_channels();
    let cfg = TpSlConfig::default();
    let s1 = run_shadow(&w.high, &w.low, &w.close, &w.buy1, &cfg);
    let s2 = run_shadow(&w.high, &w.low, &w.close, &w.buy2, &cfg);
    let s3 = run_shadow(&w.high, &w.low, &w.close, &w.buy3, &cfg);
    for s in [&s1, &s2, &s3] {
        assert_invariants(s, &w.high, &w.low, &w.close, &cfg);
    }
}

/// 收盘破位（触发方式2）不破坏终止语义。
#[test]
fn atr_tp_sl_close_trigger_config_invariants_on_real_data() {
    let w = load_window_with_buy_channels();
    let cfg = TpSlConfig {
        trigger_mode: 2,
        ..TpSlConfig::default()
    };
    let s1 = run_shadow(&w.high, &w.low, &w.close, &w.buy1, &cfg);
    let s2 = run_shadow(&w.high, &w.low, &w.close, &w.buy2, &cfg);
    let s3 = run_shadow(&w.high, &w.low, &w.close, &w.buy3, &cfg);
    for s in [&s1, &s2, &s3] {
        assert_invariants(s, &w.high, &w.low, &w.close, &cfg);
    }
}

/// 加载 002340 日线 → 三类买点通道（401/403/405 各自独立），
/// 只取最后 `WINDOW` 根（分析窗口内）。
fn load_window_with_buy_channels() -> WindowData {
    const WINDOW: usize = 1000;

    let (_, high, low, close) = load_series("../tests/data/002340_day.csv");
    let n = high.len();

    let request = analyze_request(
        vec![401, 403, 405],
        "D",
        high.clone(),
        low.clone(),
        close.clone(),
        6,
        WINDOW as u32,
        true,
    );
    let state = AppState::new(config_for(6, WINDOW as u32));
    let params = AnalysisParams::from(state.config());
    let snapshot = SnapshotAnalyzer::new(&params)
        .build_snapshot(&request)
        .expect("snapshot should build");

    // 通道数组与全部输入对齐；分析窗口外为 0，仅取最后 WINDOW 根
    let start = n - WINDOW;
    let slice = |v: &Vec<f32>| v[start..].to_vec();

    let take = |func_id: u32| {
        let channel = snapshot
            .channels
            .iter()
            .find(|c| c.func_id == func_id)
            .unwrap_or_else(|| panic!("channel {func_id}"));
        let values = slice(&channel.values);
        let count = values.iter().filter(|&&v| v > 0.0).count();
        assert!(
            count >= 1,
            "窗口内 {func_id} 通道应至少出现 1 个买点锚点, 实际 {count}"
        );
        values
    };

    let b1 = take(401);
    let b2 = take(403);
    let b3 = take(405);
    WindowData {
        high: slice(&high),
        low: slice(&low),
        close: slice(&close),
        buy1: b1,
        buy2: b2,
        buy3: b3,
    }
}

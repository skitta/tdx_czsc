//! 背驰（一买/一卖）黄金样本回归测试。
//!
//! 目标：验证背驰判定本体（czsc-signals `check_first_buy/sell`）与项目
//! 接入层（`compute_buy_sell_points` / 401/402 通道）在确定性构造的
//! 「趋势末端 + 力度衰减」行情上的命中行为，补上此前只有锚点/去重
//! 断言、没有背驰命中断言的缺口。
//!
//! 量能双路径：
//! - 真实 vol 注入（公式 950 量能通道融合后的形态）；
//! - vol 缺失走振幅代理（`derive_volume_proxy=true` 的兜底形态）。
//!
//! 合成行情规则（与 czsc-core `remove_include`/`check_fx`/`check_bi`
//! 语义兼容，均为确定性可复现）：
//! - 每段线性插值（段间价格连续），普通K线影线 0.15×单根位移防包含；
//! - 段末为折点K线（十字星）：底分型 low=end−2.5×位移、顶分型
//!   high=end+2.5×位移（深影线保证分型/成笔稳定，不被合并吞掉）；
//! - 序列开头 3 根前奏形成首折点分型（首段下行→顶分型，反之底分型）；
//! - 序列末尾尾巴确认末笔（缓升/缓降，不得穿越末笔起点极值）。

use chrono::TimeZone;
use czsc_core::analyze::CZSC;
use czsc_core::objects::bar::{RawBar, RawBarBuilder};
use czsc_core::objects::bi::BI;
use czsc_core::objects::direction::Direction;
use czsc_core::objects::freq::Freq;
use czsc_signals::cxt::{cxt_first_buy_v221126, cxt_first_sell_v221126};
use czsc_signals::params::ParamView;
use czsc_signals::types::TaCache;
use serde_json::Value;
use std::collections::HashMap;

use tdx_czsc_service::analyzer::{build_snapshot, compute_buy_sell_points, AnalysisParams};
use tdx_czsc_service::config::AppConfig;
use tdx_czsc_service::state::AppState;

mod common;
use common::analyze_request;

// ---------------------------------------------------------------------------
// 合成行情构造
// ---------------------------------------------------------------------------

const MIN_BI_LEN: usize = 6;

/// 构造确定性锯齿行情。
///
/// `segments`：[(start, end), ...]，段间价格连续（后段 start == 前段 end）。
/// `bars_per_seg`：每段K线数（与 segments 等长；末段应明显更长，
/// 使长度与"振幅/收盘"代理量呈衰减）。
/// `vol_per_seg`：每段普通K线的量能基准（末段应更低，使量能背驰成立）。
/// `tail`：(起点价, 终点价, 每根量) 的确认尾巴，保持与末笔起点同侧小幅度。
pub fn build_zigzag(
    segments: &[(f64, f64)],
    bars_per_seg: &[usize],
    vol_per_seg: &[f64],
    tail: Option<(f64, f64, f64)>,
) -> Vec<RawBar> {
    let mut bars: Vec<RawBar> = Vec::new();
    let mut id: i32 = 0;
    let base = chrono::Utc
        .with_ymd_and_hms(2020, 1, 1, 0, 0, 0)
        .single()
        .expect("base dt");

    let push = |open: f64,
                    close: f64,
                    high: f64,
                    low: f64,
                    vol: f64,
                    bars: &mut Vec<RawBar>,
                    id: &mut i32| {
        let bar = RawBarBuilder::default()
            .symbol("SYN")
            .dt(base + chrono::Duration::days(*id as i64))
            .id(*id)
            .freq(Freq::D)
            .open(open)
            .close(close)
            .high(high)
            .low(low)
            .vol(vol)
            .amount(0.0)
            .build()
            .expect("bar");
        bars.push(bar);
        *id += 1;
    };

    let first_seg = &segments[0];
    let s0 = (first_seg.0 - first_seg.1) / bars_per_seg[0] as f64;
    let apex = first_seg.0;
    if s0 > 0.0 {
        // 首段下行：前奏形成起点顶分型（bar1 为中间元素、全局最高）
        push(apex - 0.5, apex - 0.5, apex - 0.1, apex - 1.6, 0.0, &mut bars, &mut id);
        push(apex, apex, apex + 2.0, apex - 0.7, 0.0, &mut bars, &mut id);
        push(apex, apex - s0 * 2.0, apex - 1.0, apex - s0 * 2.0 - 0.5, 0.0, &mut bars, &mut id);
    } else {
        // 首段上行：前奏形成起点底分型（bar1 全局最低）
        push(apex + 0.5, apex + 0.5, apex + 1.6, apex + 0.1, 0.0, &mut bars, &mut id);
        push(apex, apex, apex + 0.7, apex - 2.0, 0.0, &mut bars, &mut id);
        push(apex, apex - s0.abs() * 2.0, apex - s0.abs() * 2.0 + 0.5, apex + 1.0, 0.0, &mut bars, &mut id);
    }

    let mut prev_close = apex;
    for (k, (start, end)) in segments.iter().enumerate() {
        let seg_len = bars_per_seg.get(k).copied().unwrap_or(7);
        let step = (end - start) / seg_len as f64;
        let vol_base = vol_per_seg.get(k).copied().unwrap_or(1000.0);
        for j in 0..seg_len {
            let is_vertex = j == seg_len - 1;
            let close = start + step * (j + 1) as f64;
            let open = if is_vertex { close } else { prev_close };
            let (high, low) = if is_vertex {
                let shadow = 2.5 * step.abs();
                if step < 0.0 {
                    // 底分型折点：低点压低（创新低）、高点收敛
                    (close + 0.05 * step.abs(), close - shadow)
                } else {
                    // 顶分型折点：高点抬高、低点收敛
                    (close + shadow, close - 0.05 * step.abs())
                }
            } else {
                let w = 0.15 * step.abs();
                (open.max(close) + w, open.min(close) - w)
            };
            let vol = if is_vertex { vol_base * 0.4 } else { vol_base };
            push(open, close, high, low, vol, &mut bars, &mut id);
            prev_close = close;
        }
    }

    if let Some((ts, te, tv)) = tail {
        let tail_len = 6usize;
        let step = (te - ts) / tail_len as f64;
        for j in 0..tail_len {
            let close = ts + step * (j + 1) as f64;
            let open = prev_close;
            let w = 0.15 * step.abs();
            push(open, close, open.max(close) + w, open.min(close) - w, tv, &mut bars, &mut id);
            prev_close = close;
        }
    }

    bars
}

fn describe_bis(bis: &[BI]) -> String {
    bis.iter()
        .map(|bi| {
            let dir = match bi.direction {
                Direction::Up => "U",
                Direction::Down => "D",
            };
            format!("{dir}({:.1}->{:.1})", bi.fx_a.fx, bi.fx_b.fx)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn first_buy_signal(czsc: &CZSC) -> String {
    let params = HashMap::<String, Value>::new();
    let view = ParamView::new(&params);
    let mut cache = TaCache::new();
    cxt_first_buy_v221126(czsc, &view, &mut cache)
        .first()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "<none>".to_string())
}

fn first_sell_signal(czsc: &CZSC) -> String {
    let params = HashMap::<String, Value>::new();
    let view = ParamView::new(&params);
    let mut cache = TaCache::new();
    cxt_first_sell_v221126(czsc, &view, &mut cache)
        .first()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "<none>".to_string())
}

/// 哨兵断言：合成行情必须演化出预期笔结构（DUDUDUD 或 UDUDUDU），
/// 首笔含全局最高/最低、末笔创新低/新高。任何成笔细节漂移都会在此暴露。
fn assert_zigzag_structure(czsc: &CZSC, expected_dir: Direction) {
    let bis = &czsc.bi_list;
    assert_eq!(bis.len(), 7, "bi structure: {}", describe_bis(bis));
    let expected: Vec<Direction> = (0..7)
        .map(|i| {
            if i % 2 == 0 {
                expected_dir
            } else {
                match expected_dir {
                    Direction::Up => Direction::Down,
                    Direction::Down => Direction::Up,
                }
            }
        })
        .collect();
    let actual: Vec<Direction> = bis.iter().map(|b| b.direction).collect();
    assert_eq!(actual, expected, "direction sequence: {}", describe_bis(bis));

    let max_high = bis.iter().map(|b| b.get_high()).fold(f64::NEG_INFINITY, f64::max);
    let min_low = bis.iter().map(|b| b.get_low()).fold(f64::INFINITY, f64::min);
    match expected_dir {
        Direction::Down => {
            assert_eq!(bis[0].get_high(), max_high, "首笔必须含全局最高点");
            assert_eq!(bis[6].get_low(), min_low, "末笔必须创新低");
        }
        Direction::Up => {
            assert_eq!(bis[0].get_low(), min_low, "首笔必须含全局最低点");
            assert_eq!(bis[6].get_high(), max_high, "末笔必须创新高");
        }
    }
}

// ---------------------------------------------------------------------------
// 一买黄金样本
// ---------------------------------------------------------------------------

fn first_buy_zigzag() -> Vec<RawBar> {
    // 7 段下跌趋势（D U D U D U D），每段反弹不创新高、下跌创新低。
    // 末段同长（7 根）但更窄（55→20 幅 35 明显小于前段 55~60 幅）：
    // 单根振幅 1.05 vs 前段 1.71~2.57 → 代理量总和 12.5 < 前段 17.4，
    // 且末笔长度（≈9) < 关键笔均值（≈10.3）→ 量能/长度双背驰。
    let segments = [
        (120.0, 60.0),
        (60.0, 95.0),
        (95.0, 40.0),
        (40.0, 70.0),
        (70.0, 30.0),
        (30.0, 55.0),
        (55.0, 20.0),
    ];
    let bars_per_seg = [7usize; 7];
    let vol_per_seg = [900.0, 900.0, 800.0, 800.0, 700.0, 700.0, 100.0];
    build_zigzag(&segments, &bars_per_seg, &vol_per_seg, Some((20.0, 26.0, 50.0)))
}

/// 末笔结束分型（底分型）极值K线索引：前奏3 + 前6段×7 + 末段前6根 = 51。
/// 这是 401 通道应出现价格的锚点位置。
fn first_buy_anchor_index() -> usize {
    3 + 6 * 7 + 6
}

#[test]
fn first_buy_detected_on_synthetic_downtrend() {
    let bars = first_buy_zigzag();

    // 1) 哨兵：笔结构必须如设计（DUDUDUD、首笔全局最高、末笔创新低）
    let czsc = CZSC::new(bars.clone(), 50, MIN_BI_LEN);
    assert_zigzag_structure(&czsc, Direction::Down);
    println!("bi_list: {}", describe_bis(&czsc.bi_list));

    // 2) 背驰判定本体命中
    assert_eq!(
        first_buy_signal(&czsc),
        "日线_D1B_BUY1_一买_7笔_任意_0",
        "一买信号必须命中（7笔窗口）"
    );

    // 3) 项目接入层（真实 vol 路径）：401 点锚在（某）向下笔终点分型极值K线；
    //    演化中"当前末端"创新低即可能命中（czsc 信号为状态型），
    //    因此最早命中点可能在中间向下笔（本样本为 idx=37 的段4 折点），
    //    但末段（力度最弱）必须也产生一个 401 点并锚在末笔端点。
    let points = compute_buy_sell_points(&bars, 50, MIN_BI_LEN);
    let anchor = first_buy_anchor_index();
    let buy1s: Vec<&tdx_czsc_service::analyzer::BuySellPoint> =
        points.iter().filter(|p| p.func_id == 401).collect();
    assert!(
        !buy1s.is_empty(),
        "真实量路径应产出 401 点；points={:#?}",
        points
    );
    println!("401 锚点: {:?}", buy1s.iter().map(|p| p.index).collect::<Vec<_>>());
    assert!(
        buy1s.iter().any(|p| p.index == anchor),
        "末段力度最弱，必须存在锚在末笔底分型极值K线（idx={anchor}）的 401 点"
    );
}

#[test]
fn first_buy_channel_via_proxy_on_synthetic_downtrend() {
    // 4) 项目接入层（代理量路径：vol 缺失 + derive_volume_proxy=true）
    let bars = first_buy_zigzag();
    let mut config = AppConfig::default();
    config.min_bi_len = MIN_BI_LEN as u32;
    config.default_analysis_window = bars.len() as u32;
    let state = AppState::new(config);

    let high: Vec<f32> = bars.iter().map(|b| b.high as f32).collect();
    let low: Vec<f32> = bars.iter().map(|b| b.low as f32).collect();
    let close: Vec<f32> = bars.iter().map(|b| b.close as f32).collect();
    let request = analyze_request(
        "SYN",
        vec![401],
        "D",
        high,
        low,
        close,
        MIN_BI_LEN as u32,
        bars.len() as u32,
        true,
    );

    let params = AnalysisParams::from((&request, &state.config));
    let snapshot = build_snapshot(&request, &params).expect("snapshot");
    let channel = snapshot
        .channels
        .iter()
        .find(|c| c.func_id == 401)
        .expect("401 channel");
    let anchor = first_buy_anchor_index();
    let hits: Vec<usize> = channel
        .values
        .iter()
        .enumerate()
        .filter(|(_, v)| v.abs() > f32::EPSILON)
        .map(|(i, _)| i)
        .collect();
    println!("代理路径 401 通道命中位置: {hits:?}");
    assert!(!hits.is_empty(), "代理量路径必须产出 401 命中点");
    assert!(
        hits.contains(&anchor),
        "代理量路径 401 通道应包含末笔底分型极值K线 idx={anchor}（命中 {hits:?}）"
    );
}

// ---------------------------------------------------------------------------
// 一卖镜像样本
// ---------------------------------------------------------------------------

#[test]
fn first_sell_detected_on_synthetic_uptrend() {
    // 镜像：7 段上升趋势（U D U D U D U），末段更长更窄、量能衰减。
    // 段幅须保证：折点顶分型影线（close+2.5×位移）逐段抬高但不超过
    // 末笔顶分型高度（max_high 必须落在末笔）。
    let segments = [
        (20.0, 80.0),
        (80.0, 45.0),
        (45.0, 100.0),
        (100.0, 70.0),
        (70.0, 130.0),
        (130.0, 95.0),
        (95.0, 150.0),
    ];
    let vol_per_seg = [900.0, 900.0, 800.0, 800.0, 700.0, 700.0, 100.0];
    let bars_per_seg = [7usize, 7, 7, 7, 7, 7, 20];
    let bars = build_zigzag(&segments, &bars_per_seg, &vol_per_seg, Some((150.0, 144.0, 50.0)));

    let czsc = CZSC::new(bars.clone(), 50, MIN_BI_LEN);
    assert_zigzag_structure(&czsc, Direction::Up);
    println!("bi_list: {}", describe_bis(&czsc.bi_list));

    assert_eq!(
        first_sell_signal(&czsc),
        "日线_D1B_SELL1_一卖_7笔_任意_0",
        "一卖信号必须命中（7笔窗口）"
    );

    let points = compute_buy_sell_points(&bars, 50, MIN_BI_LEN);
    let sell1 = points.iter().find(|p| p.func_id == 402);
    assert!(sell1.is_some(), "真实量路径应产出 402 点；points={:#?}", points);
}

// ---------------------------------------------------------------------------
// 负样本
// ---------------------------------------------------------------------------

/// 负样本 1：结构相同但末段量能不衰减（反而最高）→ 量能背驰不成立
/// → 判定必须为 false（验证"量能 OR 长度"维度确实参与判定）。
#[test]
fn no_first_buy_when_volume_not_weaker() {
    let segments = [
        (120.0, 60.0),
        (60.0, 95.0),
        (95.0, 40.0),
        (40.0, 70.0),
        (70.0, 30.0),
        (30.0, 55.0),
        (55.0, 20.0),
    ];
    let vol_per_seg = [800.0, 800.0, 800.0, 800.0, 800.0, 800.0, 9000.0];
    // 末段 20 根：长度（≈22）超过关键笔均值（≈10-11）→ 长度背驰也不成立，
    // 与量能一起隔离（验证背驰判定确实需要价格之外的"量能 OR 长度"维度）。
    let bars_per_seg = [7usize, 7, 7, 7, 7, 7, 20];
    let bars = build_zigzag(&segments, &bars_per_seg, &vol_per_seg, Some((20.0, 26.0, 50.0)));

    let czsc = CZSC::new(bars, 50, MIN_BI_LEN);
    assert_zigzag_structure(&czsc, Direction::Down);
    let last = czsc.bi_list.last().unwrap();
    let prev = &czsc.bi_list[4];
    println!(
        "末笔 vol={:.1} len={}  prev vol={:.1} len={}",
        last.get_power_volume(),
        last.get_length(),
        prev.get_power_volume(),
        prev.get_length(),
    );
    assert!(
        last.get_power_volume() > prev.get_power_volume(),
        "负样本前置：末笔量能必须更强"
    );
    assert_ne!(
        first_buy_signal(&czsc),
        "日线_D1B_BUY1_一买_7笔_任意_0",
        "量能不背驰时不得判定一买"
    );
}

/// 负样本 2：末段不再创新低（55→60 为反弹）→ 结构条件不成立。
#[test]
fn no_first_buy_when_low_not_broken() {
    let segments = [
        (120.0, 60.0),
        (60.0, 95.0),
        (95.0, 40.0),
        (40.0, 90.0), // 反弹过高（>95 不行；90 仍低于 95，保持顶分型序列）
        (90.0, 70.0),
        (70.0, 100.0),
        (100.0, 55.0), // 末段终点 55 > 20 —— 不再创新低
    ];
    let vol_per_seg = [800.0, 800.0, 800.0, 800.0, 800.0, 800.0, 100.0];
    let bars_per_seg = [7usize, 7, 7, 7, 7, 7, 7];
    let bars = build_zigzag(&segments, &bars_per_seg, &vol_per_seg, Some((55.0, 60.0, 50.0)));

    let czsc = CZSC::new(bars, 50, MIN_BI_LEN);
    // 哨兵：结构仍为 7 笔 DUDUDUD，但末笔低点必须不再是全局最低
    // （段4 低点 70 与段2 低点 40 均低于段6 终点 55）→ 结构条件不成立。
    let bis = &czsc.bi_list;
    assert_eq!(bis.len(), 7, "bi structure: {}", describe_bis(bis));
    let min_low = bis.iter().map(|b| b.get_low()).fold(f64::INFINITY, f64::min);
    assert!(
        bis[6].get_low() > min_low,
        "末笔必须未创新低（全局最低点应位于更早的向下笔）"
    );
    assert_ne!(
        first_buy_signal(&czsc),
        "日线_D1B_BUY1_一买_7笔_任意_0",
        "结构不满足时不得判定一买"
    );
}
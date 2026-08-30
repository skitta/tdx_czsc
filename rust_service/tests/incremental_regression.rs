//! 回归测试：增量分析（enable_incremental）与全量分析输出完全一致。
//!
//! 使用仓库内真实数据 tests/data/002340_day.csv（3933 根），全部走
//! `handlers::analyze::handle` 端到端路径（AppState + 快照缓存 + 增量
//! 状态缓存 + singleflight）：
//!
//! 场景：
//! - **序列增长（全量窗口 regime**，`analysis_window == 0` 语义 = 全量窗口）：
//!   首建全量 → 追加 N 根增量承接，输出与独立全量分析逐通道逐值一致，
//!   且总分析次数不增加；
//! - **等长末笔原地更新**（盘中刷新）：前缀不变、末笔同 dt 值变化，
//!   增量承接输出与"直接全量分析扰动后序列"一致；
//! - **固定尾窗口**（`data_len > analysis_window`）序列增长：分析窗口
//!   整体平移导致前缀不一致，增量不适用回退全量（total_analyses 计数
//!   验证），输出仍与全量一致。
//!
//! 语义保证：增量路径与全量路径共用同一演化核心（`evolve_bars`），
//! 输出位级一致（通道 f32 逐值比较 + 信号点索引/通道/label 比较）。

use tdx_czsc_service::handlers::analyze::handle;
use tdx_czsc_service::proto::analysis::{AnalysisSnapshot, AnalyzeRequest};
use tdx_czsc_service::state::AppState;

mod common;
use common::{analyze_request, analyze_request_ex, config_for, load_series};

const CSV: &str = "../tests/data/002340_day.csv";
const MIN_BI_LEN: u32 = 6;
/// 覆盖全部结构与信号通道。
const REQ_FUNC_IDS: &[u32] = &[
    101, 102, 201, 202, 203, 204, 301, 302, 303, 304, 401, 402, 403, 404, 405, 406,
];

fn snapshot_channels(snapshot: &AnalysisSnapshot) -> Vec<(u32, Vec<f32>)> {
    let mut channels: Vec<(u32, Vec<f32>)> = snapshot
        .channels
        .iter()
        .map(|c| (c.func_id, c.values.clone()))
        .collect();
    channels.sort_by_key(|(id, _)| *id);
    channels
}

fn snapshot_signals(snapshot: &AnalysisSnapshot) -> Vec<(u32, u32, String)> {
    let mut signals: Vec<(u32, u32, String)> = snapshot
        .signals
        .iter()
        .map(|s| (s.kind as u32, s.index, s.label.clone()))
        .collect();
    signals.sort_by_key(|(kind, index, _)| (*kind, *index));
    signals
}

/// 逐通道逐值 + 信号点断言两个快照完全一致（meta 的生成时间除外）。
fn assert_snapshots_equal(a: &AnalysisSnapshot, b: &AnalysisSnapshot, ctx: &str) {
    let ca = snapshot_channels(a);
    let cb = snapshot_channels(b);
    assert_eq!(
        ca.len(),
        cb.len(),
        "{ctx}: 通道数量不一致 {} vs {}",
        ca.len(),
        cb.len()
    );
    for ((ida, va), (idb, vb)) in ca.iter().zip(&cb) {
        assert_eq!(ida, idb, "{ctx}: 通道号不一致");
        assert_eq!(va.len(), vb.len(), "{ctx}: 通道 {ida} 长度不一致");
        for (i, (x, y)) in va.iter().zip(vb).enumerate() {
            assert!(
                (x - y).abs() <= f32::EPSILON,
                "{ctx}: 通道 {ida}[{i}] 不一致 {x} vs {y}"
            );
        }
    }
    assert_eq!(
        snapshot_signals(a),
        snapshot_signals(b),
        "{ctx}: 信号点不一致"
    );
}

async fn analyze_once(
    state: &std::sync::Arc<AppState>,
    request: AnalyzeRequest,
) -> AnalysisSnapshot {
    let resp = handle(state, request).await.expect("handle should succeed");
    resp.snapshot.expect("snapshot present")
}

/// 序列增长（全量窗口）：增量承接输出 == 独立全量分析输出。
#[tokio::test]
async fn growing_series_incremental_equals_full_analysis() {
    let (_, high, low, close) = load_series(CSV);
    let full_len = 1500usize;

    // 独立全量：直接分析前 1500 根
    let full_state = config_for(MIN_BI_LEN, 0);
    let full_state = AppState::new(full_state);
    let full_req = analyze_request(
        REQ_FUNC_IDS.to_vec(),
        "D",
        high[..full_len].to_vec(),
        low[..full_len].to_vec(),
        close[..full_len].to_vec(),
        MIN_BI_LEN,
        0,
        false,
    );
    let full_snap = analyze_once(&full_state, full_req).await;

    // 增量：先分析前 500 根（首建全量），再分块追加到 1500 根（增量承接）
    let inc_state = config_for(MIN_BI_LEN, 0);
    let inc_state = AppState::new(inc_state);
    let mk_req = |len: usize| {
        analyze_request(
            REQ_FUNC_IDS.to_vec(),
            "D",
            high[..len].to_vec(),
            low[..len].to_vec(),
            close[..len].to_vec(),
            MIN_BI_LEN,
            0,
            false,
        )
    };
    // 计时：首建全量 vs 后续增量承接（配合 --nocapture / release 观察稳态性能）
    let t0 = std::time::Instant::now();
    let _first = analyze_once(&inc_state, mk_req(500)).await;
    eprintln!("[timer] 首建全量(500 根) = {:?}", t0.elapsed());
    for len in [700usize, 950, 1200, 1500] {
        let t = std::time::Instant::now();
        let snap = analyze_once(&inc_state, mk_req(len)).await;
        eprintln!("[timer] 增量承接({len} 根) = {:?}", t.elapsed());
        assert_eq!(snap.channels.len(), REQ_FUNC_IDS.len());
        assert_eq!(snap.meta.as_ref().unwrap().data_len, len as u32);
    }
    let t = std::time::Instant::now();
    let inc_snap = analyze_once(&inc_state, mk_req(full_len)).await;
    eprintln!("[timer] 增量承接(1500 根, 缓存命中) = {:?}", t.elapsed());

    assert_snapshots_equal(&full_snap, &inc_snap, "序列增长(全量窗口)");
    assert_eq!(
        full_state
            .total_analyses
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "独立全量只分析一次"
    );
    assert_eq!(
        inc_state
            .total_analyses
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "增量路径除首建外不得触发全量分析"
    );

    eprintln!("ok: 序列增长 500→1500 根，4 次增量承接，输出与全量位级一致");
}

/// 等长末笔原地更新（盘中刷新）：增量承接输出 == 扰动后序列的全量分析。
#[tokio::test]
async fn in_place_last_bar_update_equals_full_analysis() {
    let (_, high, low, close) = load_series(CSV);
    let len = 1200usize;

    // 扰动末笔（同 dt 的盘中变更）：收/高/低变化
    let mut high_p = high.clone();
    let mut low_p = low.clone();
    let mut close_p = close.clone();
    // 末笔：实体上移
    close_p[len - 1] = close[len - 1] + 0.2;
    high_p[len - 1] = close_p[len - 1] + 0.1;
    low_p[len - 1] = low[len - 1] + 0.05;

    // 独立全量：扰动后序列（前 len 根）
    let full_state = config_for(MIN_BI_LEN, 0);
    let full_state = AppState::new(full_state);
    let full_req = analyze_request(
        REQ_FUNC_IDS.to_vec(),
        "D",
        high_p[..len].to_vec(),
        low_p[..len].to_vec(),
        close_p[..len].to_vec(),
        MIN_BI_LEN,
        0,
        false,
    );
    let full_snap = analyze_once(&full_state, full_req).await;

    // 增量：先分析原始序列（全量），再以同长度扰动序列请求 → 原地更新承接
    let inc_state = config_for(MIN_BI_LEN, 0);
    let inc_state = AppState::new(inc_state);
    let mk_req = |h: Vec<f32>, l: Vec<f32>, c: Vec<f32>| {
        analyze_request(
            REQ_FUNC_IDS.to_vec(),
            "D",
            h,
            l,
            c,
            MIN_BI_LEN,
            0,
            false,
        )
    };
    let _orig = analyze_once(
        &inc_state,
        mk_req(
            high[..len].to_vec(),
            low[..len].to_vec(),
            close[..len].to_vec(),
        ),
    )
    .await;
    let inc_snap = analyze_once(
        &inc_state,
        mk_req(
            high_p[..len].to_vec(),
            low_p[..len].to_vec(),
            close_p[..len].to_vec(),
        ),
    )
    .await;

    assert_snapshots_equal(&full_snap, &inc_snap, "末笔原地更新");
    assert_eq!(
        inc_state
            .total_analyses
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "原地更新必须走增量承接，不得全量重算"
    );
    eprintln!("ok: 等长末笔原地更新（盘中刷新）输出与全量位级一致");
}

/// 固定尾窗口（data_len > analysis_window）：序列增长使窗口平移，
/// 增量不适用回退全量，但输出仍正确。
#[tokio::test]
async fn tail_window_growth_falls_back_to_full_but_correct() {
    let (_, high, low, close) = load_series(CSV);
    let window: u32 = 1000;
    let len_a = 2933usize; // 窗口 bars = [1933, 2933)
    let len_b = 3933usize; // 窗口 bars = [2933, 3933) —— 整体平移

    let full_state = config_for(MIN_BI_LEN, window);
    let full_state = AppState::new(full_state);
    let full_req = analyze_request(
        REQ_FUNC_IDS.to_vec(),
        "D",
        high.clone(),
        low.clone(),
        close.clone(),
        MIN_BI_LEN,
        window,
        false,
    );
    let full_snap = analyze_once(&full_state, full_req).await;

    let inc_state = config_for(MIN_BI_LEN, window);
    let inc_state = AppState::new(inc_state);
    let mk_req = |len: usize| {
        analyze_request(
            REQ_FUNC_IDS.to_vec(),
            "D",
            high[..len].to_vec(),
            low[..len].to_vec(),
            close[..len].to_vec(),
            MIN_BI_LEN,
            window,
            false,
        )
    };
    let _a = analyze_once(&inc_state, mk_req(len_a)).await;
    let inc_snap = analyze_once(&inc_state, mk_req(len_b)).await;

    assert_snapshots_equal(&full_snap, &inc_snap, "尾窗口增长回退全量");
    assert_eq!(
        inc_state
            .total_analyses
            .load(std::sync::atomic::Ordering::Relaxed),
        2,
        "尾窗口平移时增量不适用，必须回退全量（首建 1 次 + 回退 1 次）"
    );
    eprintln!("ok: 固定尾窗口序列增长回退全量，输出正确");
}

/// 全量演进：`analysis_window = 0`（新默认）分析全部 K 线，且
/// `max_bi_num = 1000` 不再截断早期笔（旧默认 50 在 3933 根上触顶），
/// 历史结构与买卖点完整保留。
#[tokio::test]
async fn full_history_window_zero_keeps_all_bis_and_signals() {
    let (_, high, low, close) = load_series(CSV);
    let state = AppState::new(config_for(MIN_BI_LEN, 0));
    let request = analyze_request_ex(
        REQ_FUNC_IDS.to_vec(),
        "D",
        high,
        low,
        close,
        MIN_BI_LEN,
        0,
        true,
        1000,
    );
    let snap = analyze_once(&state, request).await;

    let meta = snap.meta.as_ref().expect("meta");
    assert_eq!(meta.data_len, 3933, "0 = 全量：覆盖全部 K 线");
    assert_eq!(meta.analysis_window, 0);
    assert_eq!(meta.max_bi_num, 1000);

    // 通道覆盖全部 K 线（前方历史参与分析）
    for ch in &snap.channels {
        assert_eq!(
            ch.values.len(),
            3933,
            "通道 {} 必须覆盖全部 K 线",
            ch.func_id
        );
    }
    // 早期笔不再被 50 上限截断（3933 根真实笔数应远超 50）
    assert!(
        snap.strokes.len() > 50,
        "全量历史笔数应超过旧上限 50，实际 {}",
        snap.strokes.len()
    );
    eprintln!(
        "全量(3933根): strokes={} signals={}",
        snap.strokes.len(),
        snap.signals.len()
    );
    // 结构通道在历史早期也有输出（一买/一卖信号存在）
    let buy1 = snapshot_channels(&snap)
        .into_iter()
        .find(|(id, _)| *id == 401)
        .expect("401 通道");
    assert!(
        buy1.1.iter().any(|v| v.abs() > f32::EPSILON),
        "全量历史应产出 401 一买信号"
    );
}

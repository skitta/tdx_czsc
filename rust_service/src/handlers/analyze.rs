use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};

use super::debug::{patch_debug_channels, timing_stats};
use crate::analyzer;
use crate::analyzer::{AcceptOutcome, IncrementalKey, IncrementalState};
use crate::cache::CacheKey;
use crate::proto::analysis::{AnalysisSnapshot, AnalyzeRequest, AnalyzeResponse};
use crate::state::AppState;

/// 等待同数据段（CacheKey）的在途分析完成并重查缓存。
///
/// 在途分析方无论成败都会 `notify_waiters()`；这里用 15s 超时兜底，
/// 防极端异常（分析方崩溃）导致永久挂起。缓存命中返回响应，否则 None
/// （调用方随后自行接管分析）。
async fn await_in_flight_result(
    state: &Arc<AppState>,
    cache_key: &CacheKey,
    started: Instant,
) -> Option<AnalyzeResponse> {
    let notify = state.in_flight.lock().unwrap().get(cache_key).cloned()?;
    let _ = tokio::time::timeout(Duration::from_secs(15), notify.notified()).await;

    let cache = state.cache.read().await;
    let mut snapshot = cache.get(cache_key)?;
    drop(cache);

    let elapsed_ms = started.elapsed().as_millis() as u32;
    patch_debug_channels(&mut snapshot.channels, false, elapsed_ms as f32);
    Some(AnalyzeResponse {
        snapshot: Some(snapshot),
        service_cache_hit: true,
        timing: Some(timing_stats(elapsed_ms, 0)),
    })
}

/// 撤销本请求的在途登记并唤醒所有等待者（无论分析成败，防止等待方挂死）。
fn release_in_flight(state: &Arc<AppState>, cache_key: &CacheKey) {
    if let Some(notify) = state.in_flight.lock().unwrap().remove(cache_key) {
        notify.notify_waiters();
    }
}

pub async fn handle(state: &Arc<AppState>, request: AnalyzeRequest) -> Result<AnalyzeResponse> {
    let started = Instant::now();
    let _guard = ActiveRequestGuard::new(state.clone());
    let options = request.options.unwrap_or_default();

    let cache_key = CacheKey {
        func_group: request.func_group.clone(),
        input_hash: request.input_hash,
        data_len: request.data_len,
        freq_hint: request.freq_hint.clone(),
        max_bi_num: if options.max_bi_num > 0 {
            options.max_bi_num
        } else {
            state.config.max_bi_num
        },
        min_bi_len: if options.min_bi_len > 0 {
            options.min_bi_len
        } else {
            state.config.min_bi_len
        },
        analysis_window: if options.analysis_window > 0 {
            options.analysis_window
        } else {
            state.config.default_analysis_window
        },
    };

    // 1) 已有分析结果：直接命中缓存返回
    if request.prefer_cached {
        let cache = state.cache.read().await;
        if let Some(mut snapshot) = cache.get(&cache_key) {
            drop(cache);
            patch_debug_channels(&mut snapshot.channels, true, 0.0);
            return Ok(AnalyzeResponse {
                snapshot: Some(snapshot),
                service_cache_hit: true,
                timing: Some(timing_stats(started.elapsed().as_millis() as u32, 0)),
            });
        }
    }

    // 2) singleflight：通达信多窗口/多公式对同一数据段并发首刷时，
    //    合并为一次全量分析。最多等待两轮；两轮均无果则直接接管分析。
    let mut registered = false;
    let mut attempts = 0;
    loop {
        // 仅探测是否已有同数据段在途分析；实际等待在 await_in_flight_result
        // 内部完成（拿到 Notify 后 notified() 超时等待）。
        let has_in_flight = state.in_flight.lock().unwrap().contains_key(&cache_key);
        if !has_in_flight {
            break;
        }
        attempts += 1;
        if attempts > 2 {
            break; // 双轮在途等待无果：直接分析兜底（不登记，避免泄漏）
        }
        if let Some(resp) = await_in_flight_result(state, &cache_key, started).await {
            return Ok(resp);
        }
    }
    if !registered {
        // 双检登记：等待期间可能已有请求抢先登记
        let mut lock = state.in_flight.lock().unwrap();
        if !lock.contains_key(&cache_key) {
            lock.insert(cache_key.clone(), Arc::new(tokio::sync::Notify::new()));
            registered = true;
        }
        drop(lock);
    }

    // 3) 并发上限：等待可用分析槽（防止多窗口同时全量分析打满 CPU）
    let _slot = state
        .analysis_slots
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| anyhow!("analysis semaphore closed"))?;

    // 4) 复查缓存：等槽期间在途分析可能已完成（本请求无需重复分析）
    {
        let cache = state.cache.read().await;
        if let Some(mut snapshot) = cache.get(&cache_key) {
            drop(cache);
            if registered {
                release_in_flight(state, &cache_key);
            }
            let elapsed_ms = started.elapsed().as_millis() as u32;
            patch_debug_channels(&mut snapshot.channels, false, elapsed_ms as f32);
            return Ok(AnalyzeResponse {
                snapshot: Some(snapshot),
                service_cache_hit: true,
                timing: Some(timing_stats(elapsed_ms, 0)),
            });
        }
    }

    // 5) 分析（CPU 密集）在阻塞线程池执行，不占用 runtime worker；
    //    开启增量时优先增量承接（前缀一致只演化新增 K 线，稳态 O(新增)），
    //    未命中回退全量；分析参数由请求 + 配置解析。
    let analyze_request = request.clone();
    let analyze_state = state.clone();
    let analyze = tokio::task::spawn_blocking(move || -> Result<_> {
        let params = analyzer::AnalysisParams::from((&analyze_request, &analyze_state.config));
        analyze_snapshot(&analyze_request, &params, &analyze_state)
    })
    .await
    .map_err(|err| anyhow!("analysis task join failed: {err}"))?;

    let mut snapshot = match analyze {
        Ok((snapshot, is_full)) => {
            if is_full {
                state.total_analyses.fetch_add(1, Ordering::Relaxed);
            }
            snapshot
        }
        Err(err) => {
            if registered {
                release_in_flight(state, &cache_key);
            }
            return Err(err);
        }
    };

    // 6) 写缓存并唤醒同数据段的等待者（无论如何都通知）
    if registered {
        release_in_flight(state, &cache_key);
    }
    {
        let mut cache = state.cache.write().await;
        cache.insert(cache_key, snapshot.clone());
    }

    let elapsed_ms = started.elapsed().as_millis() as u32;
    patch_debug_channels(&mut snapshot.channels, false, elapsed_ms as f32);

    Ok(AnalyzeResponse {
        snapshot: Some(snapshot),
        service_cache_hit: false,
        timing: Some(timing_stats(elapsed_ms, elapsed_ms)),
    })
}

/// 增量或全量分析入口（在 spawn_blocking 中执行）。返回 (快照, 是否全量)。
///
/// 增量路径（`config.enable_incremental`（默认开）或请求显式开启）：
/// 先构建窗口 K 线（O(窗口)，数百微秒量级），从 `IncrementalCache`
/// 取出匹配状态并 `accept`：命中且前缀一致则只演化新增 K 线（或等长
/// 末笔原地更新），随后装配快照并把演化后的状态放回缓存；
/// 无法承接（无状态/前缀不一致/评估器缺失）时执行全量演化，同时
/// 建立增量状态供后续请求承接。输出与全量路径完全一致。
///
/// 非增量路径保持 `build_snapshot` 原样调用。
fn analyze_snapshot(
    request: &AnalyzeRequest,
    params: &analyzer::AnalysisParams,
    state: &AppState,
) -> Result<(AnalysisSnapshot, bool)> {
    let incremental_active = state.config.enable_incremental
        || request
            .options
            .as_ref()
            .is_some_and(|o| o.enable_incremental);
    if !incremental_active {
        let snapshot = analyzer::build_snapshot(request, params)?;
        return Ok((snapshot, true));
    }

    // —— 增量路径（默认开启）——
    analyzer::validate_series_lengths(
        request.data_len,
        request
            .series
            .as_ref()
            .ok_or_else(|| anyhow!("market series is missing"))?,
    )?;
    let freq = analyzer::parse_freq(&request.freq_hint)?;
    let data_len = request.data_len as usize;
    let window_start = analyzer::window_start_of(data_len, params.analysis_window);
    let window_bars =
        analyzer::build_raw_bars(request, freq, window_start, params.derive_volume_proxy)?;
    if window_bars.is_empty() {
        return Err(anyhow!("bars are empty after applying analysis window"));
    }

    let wants_signals = request
        .requested_func_ids
        .iter()
        .any(|id| (401..=406).contains(id));
    let max_bi_num = params.max_bi_num as usize;
    let min_bi_len = params.min_bi_len as usize;

    let key = IncrementalKey {
        freq_hint: request.freq_hint.clone(),
        max_bi_num: params.max_bi_num,
        min_bi_len: params.min_bi_len,
        analysis_window: params.analysis_window,
    };

    // 1) 尝试增量承接：取出 →（锁外演化）→ 放回。
    //    并发取出同一条目时后取者得到 None 走全量，放入总是完整状态。
    //    注意：take 的 MutexGuard 是临时值，必须独立成语句，否则其生命周期
    //    会延伸到整个 if-let（内部再 lock() 导致 std Mutex 自死锁）。
    let taken = state.incremental.lock().unwrap().take(&key);
    if let Some(mut st) = taken {
        if st.accept(&window_bars, wants_signals) == AcceptOutcome::Served {
            let points = st.signals();
            let snapshot = analyzer::assemble_snapshot(
                request, params, freq, max_bi_num, min_bi_len, &st.czsc, points,
            )?;
            state.incremental.lock().unwrap().put(key.clone(), st);
            return Ok((snapshot, false));
        }
        // 不适用：放回缓存（后续可承接的请求仍可复用）
        state.incremental.lock().unwrap().put(key.clone(), st);
    }

    // 2) 全量演化（首建或无法承接），同时建立增量状态供后续承接。
    let (czsc, mut evaluator) =
        analyzer::evolve_bars(&window_bars, max_bi_num, min_bi_len, wants_signals);
    let buy_sell_points = match evaluator.as_mut() {
        Some(ev) => ev.finalize_points(&czsc),
        None => Vec::new(),
    };
    let snapshot = analyzer::assemble_snapshot(
        request,
        params,
        freq,
        max_bi_num,
        min_bi_len,
        &czsc,
        buy_sell_points,
    )?;
    state
        .incremental
        .lock()
        .unwrap()
        .put(key, IncrementalState::new(window_bars, czsc, evaluator));
    Ok((snapshot, true))
}

struct ActiveRequestGuard {
    state: std::sync::Arc<AppState>,
}

impl ActiveRequestGuard {
    fn new(state: std::sync::Arc<AppState>) -> Self {
        state
            .active_requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self { state }
    }
}

impl Drop for ActiveRequestGuard {
    fn drop(&mut self) {
        self.state
            .active_requests
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::proto::analysis::{AnalyzeRequest, CzscOptions, MarketSeries};
    use crate::state::AppState;

    fn sample_request(prefer_cached: bool) -> AnalyzeRequest {
        let close = vec![
            10.0, 10.2, 10.5, 10.1, 9.8, 9.6, 9.9, 10.3, 10.7, 10.4, 10.1, 9.7, 9.5, 9.9, 10.4,
            10.9, 10.6, 10.1, 9.8, 9.4, 9.7, 10.2, 10.8, 11.2, 10.9, 10.5, 10.0, 9.6, 9.9, 10.3,
            10.7, 11.0, 10.8, 10.3, 9.9, 9.5, 9.8, 10.1, 10.6, 11.1, 10.7, 10.2, 9.7, 9.3, 9.6,
            10.0, 10.5, 10.9, 10.6, 10.1, 9.8, 9.4, 9.7, 10.2, 10.6, 11.0, 10.8, 10.4, 10.0, 9.7,
            10.0, 10.4, 10.9, 11.3,
        ];
        let mut high = Vec::with_capacity(close.len());
        let mut low = Vec::with_capacity(close.len());
        for (i, price) in close.iter().enumerate() {
            high.push(*price + 0.25 + (i % 3) as f32 * 0.03);
            low.push(*price - 0.22 - (i % 2) as f32 * 0.02);
        }

        AnalyzeRequest {
            func_group: "single_level_structure".to_string(),
            requested_func_ids: vec![101, 901, 902, 903],
            symbol_hint: "000001.SZ".to_string(),
            freq_hint: "F30".to_string(),
            data_len: close.len() as u32,
            input_hash: 0x22334455,
            bar_build_mode: crate::proto::analysis::BarBuildMode::BarBuildFromHlc3 as i32,
            series: Some(MarketSeries {
                open: Vec::new(),
                high,
                low,
                close,
                vol: Vec::new(),
                amount: Vec::new(),
                dt_unix_ms: Vec::new(),
            }),
            options: Some(CzscOptions {
                max_bi_num: 50,
                min_bi_len: 3,
                analysis_window: 64,
                include_channels: true,
                include_objects: false,
                enable_incremental: false,
            }),
            prefer_cached,
            caller_instance_id: 99,
        }
    }

    fn state() -> std::sync::Arc<AppState> {
        let config = AppConfig {
            min_bi_len: 3,
            default_analysis_window: 64,
            ..AppConfig::default()
        };
        AppState::new(config)
    }

    fn channel_values(response: &AnalyzeResponse, func_id: u32) -> &Vec<f32> {
        &response
            .snapshot
            .as_ref()
            .unwrap()
            .channels
            .iter()
            .find(|x| x.func_id == func_id)
            .unwrap_or_else(|| panic!("channel {func_id} not found"))
            .values
    }

    #[tokio::test]
    async fn handle_patches_debug_channels_and_hits_cache() {
        let state = state();

        let first = handle(&state, sample_request(false))
            .await
            .expect("first analyze should succeed");
        assert!(!first.service_cache_hit);
        assert!(channel_values(&first, 901).iter().all(|x| *x == 0.0));
        assert!(channel_values(&first, 902).iter().all(|x| *x == 1.0));
        assert!(channel_values(&first, 903).iter().any(|x| *x >= 0.0));

        let second = handle(&state, sample_request(true))
            .await
            .expect("second analyze should succeed");
        assert!(second.service_cache_hit);
        assert!(channel_values(&second, 901).iter().all(|x| *x == 1.0));
        assert!(channel_values(&second, 902).iter().all(|x| *x == 1.0));
    }

    /// 并发去重：同一数据段（同 CacheKey）的并发首刷必须合并为一次全量分析。
    #[tokio::test]
    async fn concurrent_same_key_requests_share_single_analysis() {
        let state = state();
        let before = state.total_analyses.load(Ordering::Relaxed);

        let mut tasks = Vec::new();
        for _ in 0..4 {
            let st = state.clone();
            let req = sample_request(false);
            tasks.push(tokio::spawn(async move { handle(&st, req).await }));
        }
        for task in tasks {
            let resp = task.await.expect("task join").expect("handle ok");
            assert!(resp.service_cache_hit || !resp.snapshot.as_ref().unwrap().channels.is_empty());
        }

        let after = state.total_analyses.load(Ordering::Relaxed);
        assert_eq!(
            after - before,
            1,
            "同数据段并发首刷必须只执行一次全量分析（singleflight）"
        );
    }

    /// 并发去重后再来请求命中缓存（不重复分析）。
    #[tokio::test]
    async fn subsequent_request_hits_cache_after_concurrent_burst() {
        let state = state();
        let reqs = (0..2).map(|_| sample_request(false)).collect::<Vec<_>>();
        let mut tasks = Vec::new();
        for req in reqs {
            let st = state.clone();
            tasks.push(tokio::spawn(async move { handle(&st, req).await }));
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }

        let before = state.total_analyses.load(Ordering::Relaxed);
        let resp = handle(&state, sample_request(true))
            .await
            .expect("cached request should succeed");
        assert!(resp.service_cache_hit);
        assert_eq!(
            state.total_analyses.load(Ordering::Relaxed),
            before,
            "缓存命中不得触发新的全量分析"
        );
    }
}

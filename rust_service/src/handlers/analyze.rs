use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use tokio::sync::OwnedSemaphorePermit;

use super::Handlers;
use super::debug::{CacheState, SnapshotPatch, Timing};
use crate::analyzer::{AnalysisParams, IncrementalMode, ResolvedParams};
use crate::cache::CacheKey;
use crate::proto::analysis::{AnalysisSnapshot, AnalyzeRequest, AnalyzeResponse};
use crate::state::AppState;

impl Handlers {
    /// 分析请求入口：缓存 → singleflight → 并发槽 → 复查缓存 → 阻塞计算。
    pub async fn analyze(&self, request: AnalyzeRequest) -> Result<AnalyzeResponse> {
        AnalyzeRun::new(self.state(), request).execute().await
    }
}

/// 单次分析请求的编排状态机。
///
/// 每个阶段方法只做一件事，命中时提前给出响应；[`execute`](Self::execute)
/// 按序串接各阶段，避免把缓存、并发与计算混在一个函数体里。
struct AnalyzeRun<'a> {
    state: &'a Arc<AppState>,
    request: Arc<AnalyzeRequest>,
    params: Arc<AnalysisParams>,
    cache_key: CacheKey,
    started: Instant,
    _active: ActiveRequestGuard,
    in_flight: Option<InFlightSlot>,
}

impl<'a> AnalyzeRun<'a> {
    fn new(state: &'a Arc<AppState>, request: AnalyzeRequest) -> Self {
        let params = Arc::new(AnalysisParams::from(state.config()));
        let cache_key = AnalyzeKey::new(&request, &params).cache_key();
        Self {
            state,
            _active: ActiveRequestGuard::new(Arc::clone(state)),
            request: Arc::new(request),
            params,
            cache_key,
            started: Instant::now(),
            in_flight: None,
        }
    }

    /// 主流程：任一阶段命中即返回，否则落到阻塞计算。
    async fn execute(mut self) -> Result<AnalyzeResponse> {
        if let Some(response) = self.try_cached().await {
            return Ok(response);
        }
        if let Some(response) = self.await_in_flight().await {
            return Ok(response);
        }
        self.register_in_flight();

        let slot = self.acquire_slot().await?;
        if let Some(response) = self.recheck_cache().await {
            return Ok(response);
        }
        self.compute(slot).await
    }

    /// 阶段 1：`prefer_cached` 请求直接读缓存。
    async fn try_cached(&self) -> Option<AnalyzeResponse> {
        if !self.request.prefer_cached {
            return None;
        }
        let snapshot = self.state.cache().read().await.get(&self.cache_key)?;
        Some(Self::cached_response(snapshot, CacheState::Hit, 0))
    }

    /// 阶段 2：singleflight。同数据段并发首刷合并为一次分析；最多等待两轮，
    /// 两轮无果直接接管（不登记，避免泄漏）。
    async fn await_in_flight(&self) -> Option<AnalyzeResponse> {
        let mut attempts = 0;
        while self.has_in_flight() {
            attempts += 1;
            if attempts > 2 {
                return None;
            }
            if let Some(response) = self.wait_once().await {
                return Some(response);
            }
        }
        None
    }

    fn has_in_flight(&self) -> bool {
        self.state.in_flight().lock().contains_key(&self.cache_key)
    }

    /// 等待同数据段在途分析并重查缓存；15s 超时兜底，防分析方崩溃导致挂死。
    async fn wait_once(&self) -> Option<AnalyzeResponse> {
        let notify = self
            .state
            .in_flight()
            .lock()
            .get(&self.cache_key)
            .cloned()?;
        let _ = tokio::time::timeout(Duration::from_secs(15), notify.notified()).await;

        let snapshot = self.state.cache().read().await.get(&self.cache_key)?;
        Some(Self::cached_response(
            snapshot,
            CacheState::Miss,
            self.elapsed_ms(),
        ))
    }

    /// 阶段 3：登记在途（双检：等待期间可能已被其它请求抢先登记）。
    fn register_in_flight(&mut self) {
        self.in_flight = Some(InFlightSlot::register(self.state, &self.cache_key));
    }

    /// 阶段 4：取得并发分析槽，防多窗口同时全量分析打满 CPU。
    async fn acquire_slot(&self) -> Result<OwnedSemaphorePermit> {
        self.state
            .analysis_slots()
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow!("analysis semaphore closed"))
    }

    /// 阶段 5：等槽期间在途分析可能已完成，复查缓存避免重复计算。
    async fn recheck_cache(&self) -> Option<AnalyzeResponse> {
        let snapshot = self.state.cache().read().await.get(&self.cache_key)?;
        Some(Self::cached_response(
            snapshot,
            CacheState::Miss,
            self.elapsed_ms(),
        ))
    }

    /// 阶段 6：在阻塞线程池执行分析；先写缓存再唤醒等待者。
    async fn compute(mut self, _slot: OwnedSemaphorePermit) -> Result<AnalyzeResponse> {
        let request = Arc::clone(&self.request);
        let params = Arc::clone(&self.params);
        let state = Arc::clone(self.state);
        let mode = IncrementalMode::resolve(
            self.state.config().enable_incremental,
            self.request.options.as_ref(),
        );

        let outcome =
            tokio::task::spawn_blocking(move || state.analysis().analyze(&request, &params, mode))
                .await
                .map_err(|err| anyhow!("analysis task join failed: {err}"))??;

        if outcome.evolution.is_full() {
            self.state.record_full_analysis();
        }

        let snapshot = Arc::new(outcome.snapshot);
        {
            let mut cache = self.state.cache().write().await;
            cache.insert(self.cache_key.clone(), Arc::clone(&snapshot));
        }
        if let Some(slot) = self.in_flight.as_mut() {
            slot.release();
        }

        Ok(Self::computed_response(
            Arc::unwrap_or_clone(snapshot),
            self.elapsed_ms(),
        ))
    }

    fn elapsed_ms(&self) -> u32 {
        u32::try_from(self.started.elapsed().as_millis()).unwrap_or(u32::MAX)
    }

    /// 缓存响应：901 由 `cache` 决定（只有未经等待/计算的直接命中才是 1）。
    fn cached_response(
        snapshot: Arc<AnalysisSnapshot>,
        cache: CacheState,
        latency_ms: u32,
    ) -> AnalyzeResponse {
        let mut snapshot = Arc::unwrap_or_clone(snapshot);
        SnapshotPatch {
            cache,
            latency_ms: latency_ms as f32,
        }
        .apply(&mut snapshot);
        AnalyzeResponse {
            snapshot: Some(snapshot),
            service_cache_hit: true,
            timing: Some(Timing::cached(latency_ms)),
        }
    }

    /// 计算响应：901 恒 0，耗时统计两段相等。
    fn computed_response(mut snapshot: AnalysisSnapshot, latency_ms: u32) -> AnalyzeResponse {
        SnapshotPatch {
            cache: CacheState::Miss,
            latency_ms: latency_ms as f32,
        }
        .apply(&mut snapshot);
        AnalyzeResponse {
            snapshot: Some(snapshot),
            service_cache_hit: false,
            timing: Some(Timing::computed(latency_ms)),
        }
    }
}

/// 缓存键构造：请求选项优先、配置兜底（与 [`AnalysisParams`] 同一解析规则）。
struct AnalyzeKey<'a> {
    request: &'a AnalyzeRequest,
    params: &'a AnalysisParams,
}

impl<'a> AnalyzeKey<'a> {
    fn new(request: &'a AnalyzeRequest, params: &'a AnalysisParams) -> Self {
        Self { request, params }
    }

    fn cache_key(&self) -> CacheKey {
        let resolved = ResolvedParams::from_request(self.request, self.params);
        let mut requested_func_ids = self.request.requested_func_ids.clone();
        requested_func_ids.sort_unstable();
        requested_func_ids.dedup();
        CacheKey {
            func_group: self.request.func_group.clone(),
            input_hash: self.request.input_hash,
            data_len: self.request.data_len,
            freq_hint: self.request.freq_hint.clone(),
            max_bi_num: resolved.max_bi_num,
            min_bi_len: resolved.min_bi_len,
            analysis_window: resolved.window.bars(),
            requested_func_ids,
            include_objects: self
                .request
                .options
                .as_ref()
                .is_some_and(|options| options.include_objects),
        }
    }
}

/// 在途分析登记（RAII）：析构即移除登记并唤醒等待者。
///
/// `key` 为 `None` 表示等待期间已被其它请求抢先登记：本请求不持有登记，
/// 析构时无操作。
struct InFlightSlot {
    state: Arc<AppState>,
    key: Option<CacheKey>,
}

impl InFlightSlot {
    /// 双检登记：已存在同键在途分析时不登记（避免覆盖在途方的 `Notify`）。
    fn register(state: &Arc<AppState>, key: &CacheKey) -> Self {
        let key = {
            let mut slots = state.in_flight().lock();
            match slots.entry(key.clone()) {
                std::collections::hash_map::Entry::Occupied(_) => None,
                std::collections::hash_map::Entry::Vacant(slot) => {
                    let key = slot.key().clone();
                    slot.insert(Arc::new(tokio::sync::Notify::new()));
                    Some(key)
                }
            }
        };
        Self {
            state: Arc::clone(state),
            key,
        }
    }

    /// 释放登记并唤醒等待者（幂等）；应在缓存写入之后调用。
    fn release(&mut self) {
        let Some(key) = self.key.take() else {
            return;
        };
        if let Some(notify) = self.state.in_flight().lock().remove(&key) {
            // `notify_waiters` 不保存许可：同时存一个许可，兜住已克隆
            // `Notify` 但尚未首次 poll 的等待者，避免其空等 15s 超时。
            notify.notify_waiters();
            notify.notify_one();
        }
    }
}

impl Drop for InFlightSlot {
    fn drop(&mut self) {
        self.release();
    }
}

/// 在途请求计数守卫（健康检查的 `active_requests`）。
struct ActiveRequestGuard {
    state: std::sync::Arc<AppState>,
}

impl ActiveRequestGuard {
    fn new(state: std::sync::Arc<AppState>) -> Self {
        state.begin_request();
        Self { state }
    }
}

impl Drop for ActiveRequestGuard {
    fn drop(&mut self) {
        self.state.end_request();
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
            freq_hint: "F30".to_string(),
            data_len: close.len() as u32,
            input_hash: 0x22334455,
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
                include_objects: false,
                enable_incremental: false,
            }),
            prefer_cached,
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

    async fn analyze(
        state: &std::sync::Arc<AppState>,
        request: AnalyzeRequest,
    ) -> anyhow::Result<AnalyzeResponse> {
        Handlers::new(state.clone()).analyze(request).await
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

        let first = analyze(&state, sample_request(false))
            .await
            .expect("first analyze should succeed");
        assert!(!first.service_cache_hit);
        assert!(channel_values(&first, 901).iter().all(|x| *x == 0.0));
        assert!(channel_values(&first, 902).iter().all(|x| *x == 1.0));
        assert!(channel_values(&first, 903).iter().any(|x| *x >= 0.0));

        let second = analyze(&state, sample_request(true))
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
        let before = state.total_analyses();

        let mut tasks = Vec::new();
        for _ in 0..4 {
            let st = state.clone();
            let req = sample_request(false);
            tasks.push(tokio::spawn(async move { analyze(&st, req).await }));
        }
        for task in tasks {
            let resp = task.await.expect("task join").expect("analyze ok");
            assert!(resp.service_cache_hit || !resp.snapshot.as_ref().unwrap().channels.is_empty());
        }

        let after = state.total_analyses();
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
            tasks.push(tokio::spawn(async move { analyze(&st, req).await }));
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }

        let before = state.total_analyses();
        let resp = analyze(&state, sample_request(true))
            .await
            .expect("cached request should succeed");
        assert!(resp.service_cache_hit);
        assert_eq!(
            state.total_analyses(),
            before,
            "缓存命中不得触发新的全量分析"
        );
    }

    /// 缓存键解析：请求选项 > 0 时优先，否则回退配置默认值。
    #[test]
    fn cache_key_prefers_request_options_over_config() {
        let state = state();

        let custom = sample_request(false);
        let params = AnalysisParams::from(state.config());
        let key = AnalyzeKey::new(&custom, &params).cache_key();
        assert_eq!(key.max_bi_num, 50);
        assert_eq!(key.min_bi_len, 3);
        assert_eq!(key.analysis_window, 64);

        let mut defaulted = sample_request(false);
        defaulted.options = Some(crate::proto::analysis::CzscOptions::default());
        let params = AnalysisParams::from(state.config());
        let key = AnalyzeKey::new(&defaulted, &params).cache_key();
        assert_eq!(key.max_bi_num, state.config().max_bi_num);
        assert_eq!(key.min_bi_len, state.config().min_bi_len);
        assert_eq!(key.analysis_window, state.config().default_analysis_window);
    }

    /// 在途登记：同键只登记一次，析构时移除并唤醒等待者。
    #[tokio::test]
    async fn in_flight_slot_registers_once_and_wakes_waiters_on_drop() {
        let state = state();
        let key = CacheKey {
            func_group: "single_level_structure".to_string(),
            input_hash: 1,
            data_len: 64,
            freq_hint: "F30".to_string(),
            max_bi_num: 50,
            min_bi_len: 3,
            analysis_window: 64,
            requested_func_ids: vec![101, 901, 902, 903],
            include_objects: false,
        };

        let first = InFlightSlot::register(&state, &key);
        assert!(first.key.is_some());
        let second = InFlightSlot::register(&state, &key);
        assert!(second.key.is_none(), "同键不应重复登记");

        let notify = state.in_flight().lock().get(&key).cloned().expect("已登记");

        // `join!` 先轮询等待方（完成 `Notify` 注册），再释放登记，避免通知丢失。
        tokio::join!(
            async {
                tokio::time::timeout(Duration::from_secs(5), notify.notified())
                    .await
                    .expect("析构应唤醒等待者");
            },
            async {
                drop(second);
                assert!(
                    state.in_flight().lock().contains_key(&key),
                    "非持有者析构不得移除他人登记"
                );
                drop(first);
                assert!(!state.in_flight().lock().contains_key(&key));
            }
        );
    }

    /// 迟到的等待者：`notify_waiters` 不存许可，`release` 必须额外存一个许可，
    /// 否则已克隆 `Notify` 但尚未首次 poll 的等待者会跑满 15s 超时。
    #[tokio::test]
    async fn release_stores_permit_for_late_waiters() {
        let state = state();
        let key = CacheKey {
            func_group: "single_level_structure".to_string(),
            input_hash: 1,
            data_len: 64,
            freq_hint: "F30".to_string(),
            max_bi_num: 50,
            min_bi_len: 3,
            analysis_window: 64,
            requested_func_ids: vec![101, 901, 902, 903],
            include_objects: false,
        };

        let slot = InFlightSlot::register(&state, &key);
        let notify = state.in_flight().lock().get(&key).cloned().expect("已登记");
        drop(slot);

        tokio::time::timeout(Duration::from_millis(50), notify.notified())
            .await
            .expect("迟到的等待者应立即被唤醒");
    }
}

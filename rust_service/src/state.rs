use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use tokio::sync::{Notify, RwLock, Semaphore};

use crate::analyzer::AnalysisEngine;
use crate::cache::{CacheKey, SnapshotCache};
use crate::config::{AppConfig, ConfigError};

/// 在途分析登记表：统一处理锁中毒，避免各处 `.unwrap()` 策略不一致。
#[derive(Debug, Default)]
pub(crate) struct InFlight {
    entries: Mutex<HashMap<CacheKey, Arc<Notify>>>,
}

impl InFlight {
    pub(crate) fn lock(&self) -> MutexGuard<'_, HashMap<CacheKey, Arc<Notify>>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned: PoisonError<_>| {
                tracing::error!("in_flight mutex poisoned; recovering");
                poisoned.into_inner()
            })
    }
}

/// 服务端同时执行的全量分析并发上限由 `AppConfig::analysis_parallelism`
/// （默认 4）决定。`build_snapshot` 在 spawn_blocking 线程池运行，是 CPU
/// 密集计算；限制并发数避免多窗口同时首刷把 CPU 打满后全部超时
/// （配合 DLL 侧 request_timeout_ms 与同数据段合并）。
///
/// 增量承接（`IncrementalCache` 命中）与 singleflight 等待不占槽位。

#[derive(Debug)]
pub struct AppState {
    config: AppConfig,
    started_at: Instant,
    active_requests: AtomicU32,
    cache: RwLock<SnapshotCache>,
    /// 分析用例服务：持有增量状态缓存，决定本次请求走增量还是全量。
    analysis: AnalysisEngine,
    /// 同 CacheKey 进行中的分析任务表（singleflight）：
    /// 已登记 key 的后续请求等待其 Notify，完成后重查缓存取结果。
    in_flight: InFlight,
    /// 全量分析并发上限信号量。
    analysis_slots: Arc<Semaphore>,
    /// 累计真正执行的全量分析次数（诊断/测试用）。
    total_analyses: AtomicU64,
}

impl AppState {
    /// 构造前校验配置，避免 0 并发等非法值进入运行时。
    pub fn try_new(config: AppConfig) -> Result<Arc<Self>, ConfigError> {
        config.validate()?;
        let analysis_parallelism = config.analysis_parallelism;
        Ok(Arc::new(Self {
            cache: RwLock::new(SnapshotCache::new(config.cache_capacity)),
            analysis: AnalysisEngine::new(config.incremental_cache_capacity),
            config,
            started_at: Instant::now(),
            active_requests: AtomicU32::new(0),
            in_flight: InFlight::default(),
            analysis_slots: Arc::new(Semaphore::new(analysis_parallelism)),
            total_analyses: AtomicU64::new(0),
        }))
    }

    /// 测试/内部便捷构造：调用方必须提供已校验配置。
    pub fn new(config: AppConfig) -> Arc<Self> {
        Self::try_new(config).expect("AppState requires a valid AppConfig")
    }

    pub(crate) fn active_requests(&self) -> u32 {
        self.active_requests.load(Ordering::Relaxed)
    }

    pub(crate) fn begin_request(&self) {
        self.active_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn end_request(&self) {
        self.active_requests.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// 快照缓存（handler 占位；不变量仍由 `SnapshotCache` 自身维持）。
    pub(crate) fn cache(&self) -> &RwLock<SnapshotCache> {
        &self.cache
    }

    /// 分析用例服务（增量承接与全量回退的入口）。
    pub(crate) fn analysis(&self) -> &AnalysisEngine {
        &self.analysis
    }

    pub(crate) fn in_flight(&self) -> &InFlight {
        &self.in_flight
    }

    pub(crate) fn analysis_slots(&self) -> &Arc<Semaphore> {
        &self.analysis_slots
    }

    pub(crate) fn record_full_analysis(&self) {
        self.total_analyses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn total_analyses(&self) -> u64 {
        self.total_analyses.load(Ordering::Relaxed)
    }

    pub(crate) fn uptime_ms(&self) -> u64 {
        u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

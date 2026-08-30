use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use tokio::sync::{Notify, RwLock, Semaphore};

use crate::analyzer::IncrementalCache;
use crate::cache::{CacheKey, SnapshotCache};
use crate::config::AppConfig;

/// 服务端同时执行的全量分析并发上限由 `AppConfig::analysis_parallelism`
/// （默认 4）决定。`build_snapshot` 在 spawn_blocking 线程池运行，是 CPU
/// 密集计算；限制并发数避免多窗口同时首刷把 CPU 打满后全部超时
/// （配合 DLL 侧 request_timeout_ms 与同数据段合并）。
///
/// 增量承接（`IncrementalCache` 命中）与 singleflight 等待不占槽位。

#[derive(Debug)]
pub struct AppState {
    pub config: AppConfig,
    pub started_at: Instant,
    pub active_requests: AtomicU32,
    pub cache: RwLock<SnapshotCache>,
    /// 增量分析状态缓存（CZSC + 信号评估器）：由分析线程取出→演化→放回
    /// （std Mutex，分析在 spawn_blocking 中执行，无 await 需求）。
    pub(crate) incremental: Mutex<IncrementalCache>,
    /// 同 CacheKey 进行中的分析任务表（singleflight）：
    /// 已登记 key 的后续请求等待其 Notify，完成后重查缓存取结果。
    pub in_flight: Mutex<HashMap<CacheKey, Arc<Notify>>>,
    /// 全量分析并发上限信号量。
    pub analysis_slots: Arc<Semaphore>,
    /// 累计真正执行的全量分析次数（诊断/测试用）。
    pub total_analyses: AtomicU64,
}

impl AppState {
    pub fn new(config: AppConfig) -> Arc<Self> {
        let analysis_parallelism = config.analysis_parallelism;
        Arc::new(Self {
            cache: RwLock::new(SnapshotCache::new(config.cache_capacity)),
            incremental: Mutex::new(IncrementalCache::new(config.incremental_cache_capacity)),
            config,
            started_at: Instant::now(),
            active_requests: AtomicU32::new(0),
            in_flight: Mutex::new(HashMap::new()),
            analysis_slots: Arc::new(Semaphore::new(analysis_parallelism)),
            total_analyses: AtomicU64::new(0),
        })
    }

    pub fn active_requests(&self) -> u32 {
        self.active_requests.load(Ordering::Relaxed)
    }
}

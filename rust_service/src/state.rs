use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use tokio::sync::{Notify, RwLock, Semaphore};

use crate::cache::{CacheKey, SnapshotCache};
use crate::config::AppConfig;

/// 服务端同时执行的全量分析数上限。
///
/// `build_snapshot` 在 spawn_blocking 线程池运行，是 CPU 密集计算；
/// 限制并发数避免多窗口同时首刷把 CPU 打满后全部超时
/// （配合 DLL 侧 request_timeout_ms 与同数据段合并）。
pub const MAX_CONCURRENT_ANALYSES: usize = 2;

#[derive(Debug)]
pub struct AppState {
    pub config: AppConfig,
    pub started_at: Instant,
    pub active_requests: AtomicU32,
    pub cache: RwLock<SnapshotCache>,
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
        Arc::new(Self {
            cache: RwLock::new(SnapshotCache::new(config.cache_capacity)),
            config,
            started_at: Instant::now(),
            active_requests: AtomicU32::new(0),
            in_flight: Mutex::new(HashMap::new()),
            analysis_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_ANALYSES)),
            total_analyses: AtomicU64::new(0),
        })
    }

    pub fn active_requests(&self) -> u32 {
        self.active_requests.load(Ordering::Relaxed)
    }
}

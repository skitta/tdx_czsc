use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use tokio::sync::RwLock;

use crate::cache::SnapshotCache;
use crate::config::AppConfig;

#[derive(Debug)]
pub struct AppState {
    pub config: AppConfig,
    pub started_at: Instant,
    pub active_requests: AtomicU32,
    pub cache: RwLock<SnapshotCache>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Arc<Self> {
        Arc::new(Self {
            cache: RwLock::new(SnapshotCache::new(config.cache_capacity)),
            config,
            started_at: Instant::now(),
            active_requests: AtomicU32::new(0),
        })
    }

    pub fn active_requests(&self) -> u32 {
        self.active_requests.load(Ordering::Relaxed)
    }
}

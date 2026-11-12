use std::sync::Arc;

use crate::lru::Lru;
use crate::proto::analysis::AnalysisSnapshot;

/// 快照缓存键：请求中影响分析结果的参数子集（与 DLL 侧指纹约定一致）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CacheKey {
    pub(crate) func_group: String,
    pub(crate) input_hash: u64,
    pub(crate) data_len: u32,
    pub(crate) freq_hint: String,
    pub(crate) max_bi_num: u32,
    pub(crate) min_bi_len: u32,
    pub(crate) analysis_window: u32,
    /// 请求显式要求的通道集合（规范化排序后参与缓存键）。
    pub(crate) requested_func_ids: Vec<u32>,
    /// 是否要求输出复杂对象；影响快照内容，必须参与缓存键。
    pub(crate) include_objects: bool,
}

/// LRU 快照缓存。
///
/// 不变量：缓存中保存的是**未经调试通道（901/902/903）打补丁**的快照；
/// 每个响应在返回前都会基于请求自身的命中/耗时信息重新打补丁，
/// 因此每次出缓存都必须产生一份独立快照（`Arc::unwrap_or_clone`）。
/// 内部用 `Arc` 保存是为了让读取方在**释放读锁之后**再克隆，
/// 避免长时间持有读锁阻塞写入。
#[derive(Debug)]
pub(crate) struct SnapshotCache {
    entries: Lru<CacheKey, Arc<AnalysisSnapshot>>,
}

impl SnapshotCache {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: Lru::new(capacity),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn get(&self, key: &CacheKey) -> Option<Arc<AnalysisSnapshot>> {
        self.entries.get(key).map(Arc::clone)
    }

    pub(crate) fn insert(&mut self, key: CacheKey, snapshot: Arc<AnalysisSnapshot>) {
        self.entries.insert(key, snapshot);
    }
}

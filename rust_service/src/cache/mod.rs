use std::collections::{HashMap, VecDeque};

use crate::proto::analysis::AnalysisSnapshot;

/// 快照缓存键：请求中影响分析结果的参数子集（与 DLL 侧指纹约定一致）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub input_hash: u64,
    pub data_len: u32,
    pub freq_hint: String,
    pub max_bi_num: u32,
    pub min_bi_len: u32,
    pub analysis_window: u32,
    /// 排序去重后的请求通道集合；不同子集不能共用快照。
    pub requested_func_ids: Vec<u32>,
    /// include_objects 会改变快照对象字段，必须参与缓存键。
    pub include_objects: bool,
}

#[derive(Debug)]
pub struct SnapshotCache {
    capacity: usize,
    order: VecDeque<CacheKey>,
    entries: HashMap<CacheKey, AnalysisSnapshot>,
}

impl SnapshotCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::new(),
            entries: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &CacheKey) -> Option<AnalysisSnapshot> {
        self.entries.get(key).cloned()
    }

    pub fn insert(&mut self, key: CacheKey, snapshot: AnalysisSnapshot) {
        // capacity == 0 语义是"不缓存"：空队列时 pop_front() 返回 None，
        // 否则会照样插入一条。
        if self.capacity == 0 {
            return;
        }
        if self.entries.contains_key(&key) {
            self.entries.insert(key.clone(), snapshot);
            self.order.retain(|k| k != &key);
            self.order.push_back(key);
            return;
        }

        if self.entries.len() >= self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.entries.remove(&oldest);
        }

        self.order.push_back(key.clone());
        self.entries.insert(key, snapshot);
    }
}

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

use crate::proto::analysis::AnalysisSnapshot;

#[derive(Debug, Clone, Eq)]
pub struct CacheKey {
    pub func_group: String,
    pub input_hash: u64,
    pub data_len: u32,
    pub freq_hint: String,
    pub max_bi_num: u32,
    pub min_bi_len: u32,
    pub analysis_window: u32,
}

impl PartialEq for CacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.func_group == other.func_group
            && self.input_hash == other.input_hash
            && self.data_len == other.data_len
            && self.freq_hint == other.freq_hint
            && self.max_bi_num == other.max_bi_num
            && self.min_bi_len == other.min_bi_len
            && self.analysis_window == other.analysis_window
    }
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.func_group.hash(state);
        self.input_hash.hash(state);
        self.data_len.hash(state);
        self.freq_hint.hash(state);
        self.max_bi_num.hash(state);
        self.min_bi_len.hash(state);
        self.analysis_window.hash(state);
    }
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

    pub fn get(&self, key: &CacheKey) -> Option<AnalysisSnapshot> {
        self.entries.get(key).cloned()
    }

    pub fn insert(&mut self, key: CacheKey, snapshot: AnalysisSnapshot) {
        if self.entries.contains_key(&key) {
            self.entries.insert(key.clone(), snapshot);
            self.order.retain(|k| k != &key);
            self.order.push_back(key);
            return;
        }

        if self.entries.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }

        self.order.push_back(key.clone());
        self.entries.insert(key, snapshot);
    }
}

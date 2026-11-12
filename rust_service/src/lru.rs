//! 通用 LRU 容器：容量固定，超限时淘汰最早写入的条目。
//!
//! 语义约定（与两个调用方原实现逐位一致）：
//! - `capacity == 0` 表示禁用缓存，`insert` 直接丢弃；
//! - 覆盖已存在键时把该键移到队尾（视为最近写入）；
//! - `get` 不改变淘汰顺序（调用方在读锁内取用，不应因读取产生写行为）。

use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

#[derive(Debug)]
pub(crate) struct Lru<K, V> {
    capacity: usize,
    entries: HashMap<K, V>,
    order: VecDeque<K>,
}

impl<K: Eq + Hash + Clone, V> Lru<K, V> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn get(&self, key: &K) -> Option<&V> {
        self.entries.get(key)
    }

    /// 写入或覆盖条目；满了先淘汰队首最旧条目。
    pub(crate) fn insert(&mut self, key: K, value: V) {
        if self.capacity == 0 {
            return;
        }
        // 淘汰必须在 `entry` 之前完成：`Vacant` 分支持有 `self.entries`
        // 的可变借用，期间无法再删除最旧条目。
        if self.entries.len() >= self.capacity
            && !self.entries.contains_key(&key)
            && let Some(oldest) = self.order.pop_front()
        {
            self.entries.remove(&oldest);
        }
        match self.entries.entry(key) {
            Entry::Occupied(mut slot) => {
                let key = slot.key().clone();
                slot.insert(value);
                self.order.retain(|k| k != &key);
                self.order.push_back(key);
            }
            Entry::Vacant(slot) => {
                self.order.push_back(slot.key().clone());
                slot.insert(value);
            }
        }
    }

    /// 取出并移除条目（`take`/`remove` 语义）。
    pub(crate) fn remove(&mut self, key: &K) -> Option<V> {
        let value = self.entries.remove(key)?;
        self.order.retain(|k| k != key);
        Some(value)
    }
}

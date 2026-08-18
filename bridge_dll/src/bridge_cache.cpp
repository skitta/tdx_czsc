#include "bridge_cache.h"

#include <algorithm>
#include <functional>

namespace tdx_czsc {

namespace {

template <typename T>
void HashCombine(std::size_t* seed, const T& value) {
    std::hash<T> hasher;
    *seed ^= hasher(value) + 0x9e3779b9 + (*seed << 6U) + (*seed >> 2U);
}

}  // namespace

bool CacheKey::operator==(const CacheKey& other) const {
    return func_group == other.func_group && input_hash == other.input_hash &&
           data_len == other.data_len && freq_hint == other.freq_hint &&
           max_bi_num == other.max_bi_num && min_bi_len == other.min_bi_len &&
           analysis_window == other.analysis_window;
}

std::size_t CacheKeyHasher::operator()(const CacheKey& key) const {
    std::size_t seed = 0;
    HashCombine(&seed, key.func_group);
    HashCombine(&seed, key.input_hash);
    HashCombine(&seed, key.data_len);
    HashCombine(&seed, key.freq_hint);
    HashCombine(&seed, key.max_bi_num);
    HashCombine(&seed, key.min_bi_len);
    HashCombine(&seed, key.analysis_window);
    return seed;
}

SnapshotCache::SnapshotCache(std::size_t capacity) : capacity_(capacity) {}

bool SnapshotCache::TryGet(const CacheKey& key, AnalysisSnapshotCpp* out_snapshot) {
    std::lock_guard<std::mutex> lock(mutex_);
    const auto it = entries_.find(key);
    if (it == entries_.end()) {
        return false;
    }

    if (out_snapshot != nullptr) {
        *out_snapshot = it->second;
    }
    TouchLocked(key);
    return true;
}

void SnapshotCache::Put(const CacheKey& key, const AnalysisSnapshotCpp& snapshot) {
    std::lock_guard<std::mutex> lock(mutex_);
    entries_[key] = snapshot;
    TouchLocked(key);
    EvictIfNeededLocked();
}

void SnapshotCache::TouchLocked(const CacheKey& key) {
    order_.erase(std::remove(order_.begin(), order_.end(), key), order_.end());
    order_.push_back(key);
}

void SnapshotCache::EvictIfNeededLocked() {
    while (entries_.size() > capacity_ && !order_.empty()) {
        const CacheKey oldest = order_.front();
        order_.pop_front();
        entries_.erase(oldest);
    }
}

}  // namespace tdx_czsc

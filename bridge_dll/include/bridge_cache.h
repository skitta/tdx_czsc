#pragma once

#include <cstdint>
#include <deque>
#include <mutex>
#include <optional>
#include <string>
#include <unordered_map>

#include "bridge_service_protocol.h"

namespace tdx_czsc {

struct CacheKey {
    std::string func_group;
    uint64_t input_hash = 0;
    uint32_t data_len = 0;
    std::string freq_hint;
    uint32_t max_bi_num = 50;
    uint32_t min_bi_len = 6;
    uint32_t analysis_window = 1000;

    bool operator==(const CacheKey& other) const;
};

struct CacheKeyHasher {
    std::size_t operator()(const CacheKey& key) const;
};

class SnapshotCache {
public:
    explicit SnapshotCache(std::size_t capacity = 4);

    bool TryGet(const CacheKey& key, AnalysisSnapshotCpp* out_snapshot);
    void Put(const CacheKey& key, const AnalysisSnapshotCpp& snapshot);

private:
    void TouchLocked(const CacheKey& key);
    void EvictIfNeededLocked();

    std::size_t capacity_;
    std::deque<CacheKey> order_;
    std::unordered_map<CacheKey, AnalysisSnapshotCpp, CacheKeyHasher> entries_;
    std::mutex mutex_;
};

}  // namespace tdx_czsc

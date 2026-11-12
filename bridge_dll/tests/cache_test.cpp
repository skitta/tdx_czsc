#include <cstdint>
#include <cstdio>
#include <string>
#include <vector>

#include "bridge_cache.h"

namespace {

int failures = 0;

void Check(bool cond, const char* what) {
    if (!cond) {
        std::printf("FAIL: %s\n", what);
        ++failures;
    }
}

tdx_czsc::CacheKey MakeKey(std::uint64_t hash, std::uint32_t len) {
    tdx_czsc::CacheKey key;
    key.func_group = "single_level_structure";
    key.input_hash = hash;
    key.data_len = len;
    key.freq_hint = "F30";
    return key;
}

tdx_czsc::AnalysisSnapshotCpp MakeSnapshot(std::uint64_t hash, float value) {
    tdx_czsc::AnalysisSnapshotCpp snapshot;
    snapshot.input_hash = hash;
    snapshot.data_len = 2;
    snapshot.channels[101] = {value, value};
    return snapshot;
}

}  // namespace

int main() {
    using tdx_czsc::AnalysisSnapshotCpp;
    using tdx_czsc::CacheKey;
    using tdx_czsc::SnapshotCache;

    // 未命中时不修改输出。
    {
        SnapshotCache cache(2);
        AnalysisSnapshotCpp out;
        out.input_hash = 42;
        Check(!cache.TryGet(MakeKey(1, 2), &out), "empty cache should miss");
        Check(out.input_hash == 42, "miss must not touch out_snapshot");
    }

    // 命中返回拷贝，且与写入内容一致。
    {
        SnapshotCache cache(2);
        const CacheKey key = MakeKey(7, 2);
        cache.Put(key, MakeSnapshot(7, 1.5f));
        AnalysisSnapshotCpp out;
        Check(cache.TryGet(key, &out), "fresh put should hit");
        Check(out.input_hash == 7 && out.data_len == 2, "hit returns meta");
        Check(out.channels.at(101).at(1) == 1.5f, "hit returns channel values");
    }

    // 键的每个字段都必须参与区分（不能只比哈希）。
    {
        SnapshotCache cache(8);
        const CacheKey key = MakeKey(9, 2);
        cache.Put(key, MakeSnapshot(9, 2.0f));

        AnalysisSnapshotCpp out;
        CacheKey other = key;
        other.freq_hint = "F5";
        Check(!cache.TryGet(other, &out), "freq_hint differentiates keys");
        other = key;
        other.data_len = 3;
        Check(!cache.TryGet(other, &out), "data_len differentiates keys");
        other = key;
        other.analysis_window = 64;
        Check(!cache.TryGet(other, &out), "analysis_window differentiates keys");
        Check(cache.TryGet(key, &out), "original key still hits");
    }

    // 容量满时淘汰最久未用；命中会刷新 LRU 顺序。
    {
        SnapshotCache cache(2);
        const CacheKey k1 = MakeKey(1, 2);
        const CacheKey k2 = MakeKey(2, 2);
        const CacheKey k3 = MakeKey(3, 2);
        cache.Put(k1, MakeSnapshot(1, 1.0f));
        cache.Put(k2, MakeSnapshot(2, 2.0f));

        AnalysisSnapshotCpp out;
        Check(cache.TryGet(k1, &out), "k1 should hit");

        cache.Put(k3, MakeSnapshot(3, 3.0f));
        Check(cache.TryGet(k1, &out), "k1 refreshed, should survive eviction");
        Check(!cache.TryGet(k2, &out), "k2 was least recently used, should be evicted");
        Check(cache.TryGet(k3, &out), "k3 should hit");
    }

    if (failures != 0) {
        std::printf("cache_test: %d failure(s)\n", failures);
        return 1;
    }
    std::printf("cache_test: ok\n");
    return 0;
}

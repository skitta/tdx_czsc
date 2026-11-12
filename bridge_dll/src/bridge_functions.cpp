#include "bridge_functions.h"

#include <array>
#include <atomic>
#include <deque>
#include <exception>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>

#include "bridge_cache.h"
#include "bridge_config.h"
#include "bridge_func_ids.h"
#include "bridge_ipc_client.h"
#include "bridge_logging.h"
#include "bridge_output_mapper.h"
#include "bridge_service_protocol.h"

namespace tdx_czsc {
namespace {

// 量能输入通道：公式以 TDXDLL1(950, HIGH, LOW, VOL) 调用，
// DLL 把第三路（VOL）按"数据段指纹"缓存，供后续 101/401 等分析请求融合。
constexpr uint32_t kVolumeInputFuncId = 950;
// 量能缓存条数上限（超出清除最早插入，FIFO）。
constexpr size_t kVolumeCacheMaxEntries = 16;

struct BridgeRuntime {
    explicit BridgeRuntime(BridgeConfig cfg)
        : config(std::move(cfg)), cache(config.dll_cache_capacity), client(config) {}

    BridgeConfig config;
    SnapshotCache cache;
    BridgeIpcClient client{};

    // 量能缓存：段指纹 -> VOL 序列。跨公式共享（同一通达信进程），
    // 由 volume_cache_mutex 保护；通达信退出即清空，无持久状态。
    std::mutex volume_cache_mutex;
    std::unordered_map<uint64_t, std::vector<float>> volume_cache;
    std::deque<uint64_t> volume_cache_order;

    std::atomic_uint64_t request_counter{1};
};

BridgeRuntime& Runtime() {
    static BridgeRuntime runtime(LoadBridgeConfig());
    return runtime;
}

/// 数据段指纹：只依赖 (data_len, 前两路输入 HIGH/LOW)。
/// 公式对同一图表的多个 TDXDLLn 调用传入相同的 HIGH/LOW 序列，
/// 因此 950 调用与分析请求（101/401 等）的指纹一致，VOL 才能被互相配对；
/// 第三路（CLOSE 或 VOL）不参与指纹。
uint64_t ComputeSegmentKey(int data_len, float* in_high, float* in_low) {
    uint64_t hash = 1469598103934665603ULL;
    const auto mix = [&hash](float value) {
        const uint32_t raw = *reinterpret_cast<const uint32_t*>(&value);
        hash ^= static_cast<uint64_t>(raw);
        hash *= 1099511628211ULL;
    };

    if (data_len <= 0 || in_high == nullptr || in_low == nullptr) {
        return hash;
    }

    mix(static_cast<float>(data_len));
    const int sample_count = data_len < 32 ? data_len : 32;
    for (int i = 0; i < sample_count; ++i) {
        mix(in_high[i]);
        mix(in_low[i]);
    }
    return hash;
}

/// 缓存 950 通道传入的 VOL 序列（按数据段指纹键控）。
void CacheVolume(uint64_t segment_key, std::vector<float> vol) {
    std::lock_guard<std::mutex> lock(Runtime().volume_cache_mutex);
    auto& cache = Runtime().volume_cache;
    auto& order = Runtime().volume_cache_order;
    if (cache.find(segment_key) != cache.end()) {
        cache[segment_key] = std::move(vol);
        return;
    }
    cache.emplace(segment_key, std::move(vol));
    order.push_back(segment_key);
    while (cache.size() > kVolumeCacheMaxEntries) {
        const uint64_t oldest = order.front();
        order.pop_front();
        cache.erase(oldest);
    }
}

/// 拷贝出某数据段的 VOL 序列；未命中或长度不符返回 false。
/// 必须在锁内完成拷贝：把缓存内部指针返回给锁外使用，并发的 CacheVolume 会使其悬垂。
bool TryLookupVolume(uint64_t segment_key, std::size_t expected_size, std::vector<float>* out) {
    std::lock_guard<std::mutex> lock(Runtime().volume_cache_mutex);
    const auto it = Runtime().volume_cache.find(segment_key);
    if (it == Runtime().volume_cache.end() || it->second.size() != expected_size) {
        return false;
    }
    *out = it->second;
    return true;
}

uint64_t ComputeInputHash(int data_len,
                          float* in_high,
                          float* in_low,
                          float* in_close,
                          const float* in_vol = nullptr,
                          int vol_len = 0) {
    uint64_t hash = 1469598103934665603ULL;
    const auto mix = [&hash](float value) {
        const uint32_t raw = *reinterpret_cast<const uint32_t*>(&value);
        hash ^= static_cast<uint64_t>(raw);
        hash *= 1099511628211ULL;
    };

    if (data_len <= 0 || in_high == nullptr || in_low == nullptr || in_close == nullptr) {
        return hash;
    }

    const int sample_count = data_len < 32 ? data_len : 32;
    for (int i = 0; i < sample_count; ++i) {
        mix(in_high[i]);
        mix(in_low[i]);
        mix(in_close[i]);
    }
    // 量能已融合时纳入哈希：带/不带量能的请求在服务端缓存中互不污染
    if (in_vol != nullptr && vol_len > 0) {
        const int vol_sample_count = vol_len < 32 ? vol_len : 32;
        for (int i = 0; i < vol_sample_count; ++i) {
            mix(in_vol[i]);
        }
    }
    // 与原实现一致：末根K线再次参与哈希
    mix(in_high[data_len - 1]);
    mix(in_low[data_len - 1]);
    mix(in_close[data_len - 1]);
    return hash;
}

AnalyzeRequestCpp BuildRequest(uint32_t func_id,
                               int data_len,
                               float* in_high,
                               float* in_low,
                               float* in_close) {
    AnalyzeRequestCpp request;
    request.request_id = Runtime().request_counter.fetch_add(1, std::memory_order_relaxed);
#define TDX_CZSC_FUNC_ID(ID) ID,
    request.requested_func_ids = {TDX_CZSC_ANALYSIS_FUNCS(TDX_CZSC_FUNC_ID)};
#undef TDX_CZSC_FUNC_ID
    request.data_len = data_len > 0 ? static_cast<uint32_t>(data_len) : 0;
    request.input_hash = ComputeInputHash(data_len, in_high, in_low, in_close);

    // 量能融合：公式先以 950 通道传入 VOL（按数据段指纹缓存），
    // 分析请求按同一指纹取回并附带发送；哈希同时纳入量能，
    // 使带/不带量能的请求在 DLL 与服务端缓存中互不污染。
    if (data_len > 0) {
        const uint64_t segment_key = ComputeSegmentKey(data_len, in_high, in_low);
        std::vector<float> cached_vol;
        if (TryLookupVolume(segment_key, static_cast<std::size_t>(data_len), &cached_vol)) {
            request.vol = std::move(cached_vol);
            request.input_hash = ComputeInputHash(data_len, in_high, in_low, in_close,
                                                  request.vol.data(),
                                                  static_cast<int>(request.vol.size()));
        }
    }

    // czsc 参数来自配置（INI / 环境变量 / 默认值）。
    const BridgeConfig& config = Runtime().config;
    request.options.max_bi_num = config.max_bi_num;
    request.options.min_bi_len = config.min_bi_len;
    request.options.analysis_window = config.analysis_window;

    if (data_len > 0 && in_high != nullptr && in_low != nullptr && in_close != nullptr) {
        request.high.assign(in_high, in_high + data_len);
        request.low.assign(in_low, in_low + data_len);
        request.close.assign(in_close, in_close + data_len);
    }

    // 调试: 记录通达信传入的数据长度与首尾价格, 用于排查对齐/偏移问题。
    // 热路径（每次通道调用都会走到）：默认不拼消息，TDX_CZSC_LOG_DEBUG=1 才输出。
    if (DebugLoggingEnabled()) {
        LogDebug("BuildRequest func_id=" + std::to_string(func_id) + " data_len=" +
                 std::to_string(data_len) + " first_close=" +
                 (data_len > 0 ? std::to_string(in_close[0]) : "?") + " last_close=" +
                 (data_len > 0 ? std::to_string(in_close[data_len - 1]) : "?") +
                 " first_low=" + (data_len > 0 ? std::to_string(in_low[0]) : "?") +
                 " last_low=" + (data_len > 0 ? std::to_string(in_low[data_len - 1]) : "?") +
                 " vol_fused=" + std::to_string(request.vol.size()) +
                 (request.vol.empty()
                      ? ""
                      : (" first_vol=" + std::to_string(request.vol.front()) +
                         " last_vol=" + std::to_string(request.vol.back()))));
    }

    // 目前所有 func_id 都归属 single_level_structure。
    // 后续有真实买卖点时，可拆出 signal group。
    request.func_group = (func_id >= 400 && func_id < 500) ? "single_level_signals" : "single_level_structure";
    return request;
}

CacheKey BuildCacheKey(const AnalyzeRequestCpp& request) {
    CacheKey key;
    key.func_group = request.func_group;
    key.input_hash = request.input_hash;
    key.data_len = request.data_len;
    key.freq_hint = request.freq_hint;
    key.max_bi_num = request.options.max_bi_num;
    key.min_bi_len = request.options.min_bi_len;
    key.analysis_window = request.options.analysis_window;
    return key;
}

#define TDX_CZSC_DECLARE_WRAPPER(ID)                                               \
    void Func##ID(int data_len, float* out_values, float* in_high, float* in_low,   \
                  float* in_close) {                                               \
        HandleFunc(ID, data_len, out_values, in_high, in_low, in_close);            \
    }

TDX_CZSC_ANALYSIS_FUNCS(TDX_CZSC_DECLARE_WRAPPER)

#undef TDX_CZSC_DECLARE_WRAPPER

/// 量能输入通道（func_id=950）：不发起分析请求，只把第三路输入
/// （公式传 VOL 序列）按"数据段指纹"缓存，供后续分析请求融合；
/// 输出原样回填第三路（公式层可作量能线显示或直接忽略）。
void Func950(int data_len, float* out_values, float* in_high, float* in_low, float* in_close) {
    if (out_values == nullptr || data_len <= 0) {
        LogWarn("Func950 received invalid output buffer or data_len <= 0");
        return;
    }
    if (in_high == nullptr || in_low == nullptr || in_close == nullptr) {
        LogWarn("Func950 received null input series pointer");
        FillZeros(data_len, out_values);
        return;
    }

    std::vector<float> vol(in_close, in_close + data_len);
    const uint64_t segment_key = ComputeSegmentKey(data_len, in_high, in_low);
    CacheVolume(segment_key, vol);
    std::copy(in_close, in_close + data_len, out_values);

    LogInfo("Func950 cached volume data_len=" + std::to_string(data_len) +
            " seg_key=" + std::to_string(segment_key) +
            " first_vol=" + std::to_string(in_close[0]) +
            " last_vol=" + std::to_string(in_close[data_len - 1]));
}

#define TDX_CZSC_REGISTRY_ENTRY(ID) {ID, &Func##ID},

PluginTCalcFuncInfo g_calc_funcs[] = {
    TDX_CZSC_ANALYSIS_FUNCS(TDX_CZSC_REGISTRY_ENTRY)
    {kVolumeInputFuncId, &Func950},
    {0, nullptr},
};

#undef TDX_CZSC_REGISTRY_ENTRY

}  // namespace

void HandleFunc(uint32_t func_id,
                int data_len,
                float* out_values,
                float* in_high,
                float* in_low,
                float* in_close) {
    if (out_values == nullptr || data_len <= 0) {
        LogWarn("HandleFunc received invalid output buffer or data_len <= 0");
        return;
    }

    // 异常不得穿透 DLL 边界：任何失败（含 bad_alloc）都必须降级为零输出。
    try {
        FillZeros(data_len, out_values);

        if (in_high == nullptr || in_low == nullptr || in_close == nullptr) {
            LogWarn("HandleFunc received null input series pointer");
            return;
        }

        const AnalyzeRequestCpp request = BuildRequest(func_id, data_len, in_high, in_low, in_close);
        const CacheKey cache_key = BuildCacheKey(request);

        AnalysisSnapshotCpp snapshot;
        if (Runtime().cache.TryGet(cache_key, &snapshot)) {
            if (!FillFromSnapshot(snapshot, func_id, data_len, out_values)) {
                LogDebug("cached snapshot has no channel func_id=" + std::to_string(func_id) +
                         " data_len=" + std::to_string(data_len) + " channels=" +
                         std::to_string(snapshot.channels.size()));
            }
            return;
        }

        std::string error;
        if (Runtime().client.Analyze(request, &snapshot, &error)) {
            Runtime().cache.Put(cache_key, snapshot);
            if (!FillFromSnapshot(snapshot, func_id, data_len, out_values)) {
                LogDebug("snapshot has no channel func_id=" + std::to_string(func_id) +
                         " data_len=" + std::to_string(data_len) + " channels=" +
                         std::to_string(snapshot.channels.size()));
            }
            return;
        }

        LogError("BridgeIpcClient::Analyze failed for func_id=" + std::to_string(func_id) +
                 " data_len=" + std::to_string(data_len) + " error=" + error);
        FillZeros(data_len, out_values);
    } catch (const std::exception& e) {
        try {
            LogError(std::string("HandleFunc exception: ") + e.what());
        } catch (...) {
        }
        FillZeros(data_len, out_values);
    } catch (...) {
        try {
            LogError("HandleFunc caught unknown exception");
        } catch (...) {
        }
        FillZeros(data_len, out_values);
    }
}

PluginTCalcFuncInfo* GetFunctionRegistry() {
    return g_calc_funcs;
}

}  // namespace tdx_czsc

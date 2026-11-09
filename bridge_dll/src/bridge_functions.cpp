#include "bridge_functions.h"

#include <array>
#include <atomic>
#include <cmath>
#include <deque>
#include <mutex>
#include <optional>
#include <string>
#include <unordered_map>
#include <vector>

#include "bridge_cache.h"
#include "bridge_config.h"
#include "bridge_ipc_client.h"
#include "bridge_logging.h"
#include "bridge_output_mapper.h"
#include "bridge_service_protocol.h"

namespace tdx_czsc {
namespace {

// 量能输入通道：公式以 TDXDLL1(950, HIGH, LOW, VOL) 调用，
// DLL 把第三路（VOL）按"数据段指纹"缓存，供后续 101/401 等分析请求融合。
constexpr uint32_t kVolumeInputFuncId = 950;
// 周期输入通道：公式以 TDXDLL1(951, HIGH, LOW, PERIOD+0*CLOSE) 调用，
// DLL 把第三路（周期码）按"数据段指纹"缓存，供后续请求设置 freq_hint。
constexpr uint32_t kPeriodInputFuncId = 951;
// 量能缓存条数上限（超出清除最早插入，FIFO）。
constexpr size_t kVolumeCacheMaxEntries = 16;
constexpr size_t kPeriodCacheMaxEntries = 16;

struct BridgeRuntime {
    explicit BridgeRuntime(BridgeConfig cfg)
        : config(std::move(cfg)), cache(config.dll_cache_capacity), client(config) {}

    BridgeConfig config;
    SnapshotCache cache{4};
    BridgeIpcClient client{};

    // 量能缓存：段指纹 -> VOL 序列。跨公式共享（同一通达信进程），
    // 由 volume_cache_mutex 保护；通达信退出即清空，无持久状态。
    std::mutex volume_cache_mutex;
    std::unordered_map<uint64_t, std::vector<float>> volume_cache;
    std::deque<uint64_t> volume_cache_order;

    // 周期缓存：段指纹 -> "F30"/"D"/...，由 period_cache_mutex 保护。
    std::mutex period_cache_mutex;
    std::unordered_map<uint64_t, std::string> period_cache;
    std::deque<uint64_t> period_cache_order;

    std::atomic_uint64_t request_counter{1};
};

BridgeRuntime& Runtime() {
    static BridgeRuntime runtime(LoadBridgeConfig());
    return runtime;
}

/// 数据段指纹：依赖 (data_len, 全量 HIGH/LOW)。
/// 公式对同一图表的多个 TDXDLLn 调用传入相同的 HIGH/LOW 序列，
/// 因此 950 调用与分析请求（101/401 等）的指纹一致，VOL 才能被互相配对；
/// 第三路（CLOSE 或 VOL）不参与指纹。必须遍历全量数据：只采样首尾会让
/// 历史中间 K 线的修正漏过缓存键。
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
    for (int i = 0; i < data_len; ++i) {
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

/// 查询某数据段的 VOL 序列；未命中返回 nullopt。
/// 按值返回，避免调用方在锁外持有 unordered_map 内部元素的悬空指针。
std::optional<std::vector<float>> LookupVolume(uint64_t segment_key) {
    std::lock_guard<std::mutex> lock(Runtime().volume_cache_mutex);
    const auto it = Runtime().volume_cache.find(segment_key);
    if (it == Runtime().volume_cache.end()) {
        return std::nullopt;
    }
    return it->second;
}

/// 通达信 PERIOD 码 -> 服务端可解析的 freq_hint。
///
/// 常见映射：0=1分钟、1=5分钟、2=15分钟、3=30分钟、4=60分钟、
/// 5=日线、6=周线、7=月线。不同通达信版本的 PERIOD 编码可能不同，
/// 如你的版本不符，改这里即可（公式侧仍传原始 PERIOD，不需要改模板）。
std::string PeriodCodeToFreq(int code) {
    switch (code) {
        case 0:
            return "F1";
        case 1:
            return "F5";
        case 2:
            return "F15";
        case 3:
            return "F30";
        case 4:
            return "F60";
        case 5:
            return "D";
        case 6:
            return "W";
        case 7:
            return "M";
        default:
            return {};
    }
}

/// 缓存 951 通道传入的周期（按数据段指纹键控）。
void CachePeriod(uint64_t segment_key, std::string freq) {
    std::lock_guard<std::mutex> lock(Runtime().period_cache_mutex);
    auto& cache = Runtime().period_cache;
    auto& order = Runtime().period_cache_order;
    if (cache.find(segment_key) != cache.end()) {
        cache[segment_key] = std::move(freq);
        return;
    }
    cache.emplace(segment_key, std::move(freq));
    order.push_back(segment_key);
    while (cache.size() > kPeriodCacheMaxEntries) {
        const uint64_t oldest = order.front();
        order.pop_front();
        cache.erase(oldest);
    }
}

/// 查询某数据段的周期；未命中返回 nullopt。
std::optional<std::string> LookupPeriod(uint64_t segment_key) {
    std::lock_guard<std::mutex> lock(Runtime().period_cache_mutex);
    const auto it = Runtime().period_cache.find(segment_key);
    if (it == Runtime().period_cache.end()) {
        return std::nullopt;
    }
    return it->second;
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

    for (int i = 0; i < data_len; ++i) {
        mix(in_high[i]);
        mix(in_low[i]);
        mix(in_close[i]);
    }
    // 量能已融合时纳入哈希：带/不带量能的请求在服务端缓存中互不污染
    if (in_vol != nullptr && vol_len > 0) {
        for (int i = 0; i < vol_len; ++i) {
            mix(in_vol[i]);
        }
    }
    return hash;
}

AnalyzeRequestCpp BuildRequest(uint32_t func_id,
                               int data_len,
                               float* in_high,
                               float* in_low,
                               float* in_close) {
    AnalyzeRequestCpp request;
    request.request_id = Runtime().request_counter.fetch_add(1, std::memory_order_relaxed);
    request.requested_func_ids = {101, 102, 201, 202, 203, 204, 301, 302, 303, 304, 305, 306, 307, 308, 401, 402, 403, 404, 405, 406, 407, 408, 901, 902, 903};
    request.data_len = data_len > 0 ? static_cast<uint32_t>(data_len) : 0;
    request.input_hash = ComputeInputHash(data_len, in_high, in_low, in_close);

    // 量能融合：公式先以 950 通道传入 VOL（按数据段指纹缓存），
    // 分析请求按同一指纹取回并附带发送；哈希同时纳入量能，
    // 使带/不带量能的请求在 DLL 与服务端缓存中互不污染。
    if (data_len > 0) {
        const uint64_t segment_key = ComputeSegmentKey(data_len, in_high, in_low);
        if (const std::optional<std::string> cached_freq = LookupPeriod(segment_key);
            cached_freq.has_value()) {
            request.freq_hint = *cached_freq;
        }
        if (const std::optional<std::vector<float>> cached_vol = LookupVolume(segment_key);
            cached_vol.has_value() &&
            cached_vol->size() == static_cast<size_t>(data_len)) {
            request.vol = *cached_vol;
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

    // 调试: 记录通达信传入的数据长度与首尾价格, 用于排查对齐/偏移问题
    LogInfo("BuildRequest func_id=" + std::to_string(func_id) + " data_len=" +
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

    // 目前所有 func_id 都归属 single_level_structure。
    // 后续有真实买卖点时，可拆出 signal group。
    request.func_group = (func_id >= 400 && func_id < 500) ? "single_level_signals" : "single_level_structure";
    return request;
}

CacheKey BuildCacheKey(const AnalyzeRequestCpp& request) {
    CacheKey key;
    // func_group 不改变通道内容（请求恒为全通道），不参与缓存键，避免
    // 结构/信号公式对同一份数据重复做全量分析。
    key.input_hash = request.input_hash;
    key.data_len = request.data_len;
    key.freq_hint = request.freq_hint;
    key.max_bi_num = request.options.max_bi_num;
    key.min_bi_len = request.options.min_bi_len;
    key.analysis_window = request.options.analysis_window;
    return key;
}

void FillDebugFallback(uint32_t func_id, int data_len, float* out_values) {
    if (func_id == 902) {
        std::fill(out_values, out_values + data_len, 0.0f);
        return;
    }
    FillZeros(data_len, out_values);
}

#define DECLARE_FUNC_WRAPPER(ID)                                      \
    void Func##ID(int data_len, float* out_values, float* in_high,    \
                  float* in_low, float* in_close) {                   \
        HandleFunc(ID, data_len, out_values, in_high, in_low, in_close); \
    }

DECLARE_FUNC_WRAPPER(101)
DECLARE_FUNC_WRAPPER(102)
DECLARE_FUNC_WRAPPER(201)
DECLARE_FUNC_WRAPPER(202)
DECLARE_FUNC_WRAPPER(203)
DECLARE_FUNC_WRAPPER(204)
DECLARE_FUNC_WRAPPER(301)
DECLARE_FUNC_WRAPPER(302)
DECLARE_FUNC_WRAPPER(303)
DECLARE_FUNC_WRAPPER(304)
DECLARE_FUNC_WRAPPER(305)
DECLARE_FUNC_WRAPPER(306)
DECLARE_FUNC_WRAPPER(307)
DECLARE_FUNC_WRAPPER(308)
DECLARE_FUNC_WRAPPER(401)
DECLARE_FUNC_WRAPPER(402)
DECLARE_FUNC_WRAPPER(403)
DECLARE_FUNC_WRAPPER(404)
DECLARE_FUNC_WRAPPER(405)
DECLARE_FUNC_WRAPPER(406)
DECLARE_FUNC_WRAPPER(407)
DECLARE_FUNC_WRAPPER(408)
DECLARE_FUNC_WRAPPER(901)
DECLARE_FUNC_WRAPPER(902)
DECLARE_FUNC_WRAPPER(903)

/// 量能输入通道（func_id=950）：不发起分析请求，只把第三路输入
/// （公式传 VOL 序列）按"数据段指纹"缓存，供后续分析请求融合；
/// 输出原样回填第三路（公式层可作量能线显示或直接忽略）。
void Func950(int data_len, float* out_values, float* in_high, float* in_low, float* in_close) {
    try {
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
    } catch (...) {
        if (out_values != nullptr && data_len > 0) {
            FillZeros(data_len, out_values);
        }
        try {
            LogError("Func950 caught unexpected exception");
        } catch (...) {
        }
    }
}

/// 周期输入通道（func_id=951）：公式以
/// `TDXDLL1(951, HIGH, LOW, PERIOD+0*CLOSE)` 传入当前图表周期码。
/// 不发起分析请求，只按数据段缓存周期；后续分析请求据此设置 freq_hint。
/// 输出原样回填第三路，便于公式侧调试（可忽略）。
void Func951(int data_len, float* out_values, float* in_high, float* in_low, float* in_close) {
    try {
        if (out_values == nullptr || data_len <= 0) {
            LogWarn("Func951 received invalid output buffer or data_len <= 0");
            return;
        }
        if (in_high == nullptr || in_low == nullptr || in_close == nullptr) {
            LogWarn("Func951 received null input series pointer");
            FillZeros(data_len, out_values);
            return;
        }

        const int period_code = static_cast<int>(std::lround(in_close[0]));
        const std::string freq = PeriodCodeToFreq(period_code);
        const uint64_t segment_key = ComputeSegmentKey(data_len, in_high, in_low);
        if (freq.empty()) {
            LogWarn("Func951 unsupported PERIOD code=" + std::to_string(period_code));
        } else {
            CachePeriod(segment_key, freq);
        }
        std::copy(in_close, in_close + data_len, out_values);

        LogInfo("Func951 cached period data_len=" + std::to_string(data_len) +
                " seg_key=" + std::to_string(segment_key) +
                " period_code=" + std::to_string(period_code) +
                " freq=" + (freq.empty() ? "fallback" : freq));
    } catch (...) {
        if (out_values != nullptr && data_len > 0) {
            FillZeros(data_len, out_values);
        }
        try {
            LogError("Func951 caught unexpected exception");
        } catch (...) {
        }
    }
}

PluginTCalcFuncInfo g_calc_funcs[] = {
    {101, &Func101},
    {102, &Func102},
    {201, &Func201},
    {202, &Func202},
    {203, &Func203},
    {204, &Func204},
    {301, &Func301},
    {302, &Func302},
    {303, &Func303},
    {304, &Func304},
    {305, &Func305},
    {306, &Func306},
    {307, &Func307},
    {308, &Func308},
    {401, &Func401},
    {402, &Func402},
    {403, &Func403},
    {404, &Func404},
    {405, &Func405},
    {406, &Func406},
    {407, &Func407},
    {408, &Func408},
    {901, &Func901},
    {902, &Func902},
    {903, &Func903},
    {kVolumeInputFuncId, &Func950},
    {kPeriodInputFuncId, &Func951},
    {0, nullptr},
};

}  // namespace

void HandleFunc(uint32_t func_id,
                int data_len,
                float* out_values,
                float* in_high,
                float* in_low,
                float* in_close) {
    try {
        if (out_values == nullptr || data_len <= 0) {
            LogWarn("HandleFunc received invalid output buffer or data_len <= 0");
            return;
        }

        FillZeros(data_len, out_values);

        if (in_high == nullptr || in_low == nullptr || in_close == nullptr) {
            LogWarn("HandleFunc received null input series pointer");
            return;
        }

        const AnalyzeRequestCpp request = BuildRequest(func_id, data_len, in_high, in_low, in_close);
        const CacheKey cache_key = BuildCacheKey(request);

        AnalysisSnapshotCpp snapshot;
        if (Runtime().cache.TryGet(cache_key, &snapshot)) {
            snapshot.service_cache_hit = true;
            FillFromSnapshot(snapshot, func_id, data_len, out_values);
            return;
        }

        std::string error;
        if (Runtime().client.Analyze(request, &snapshot, &error)) {
            Runtime().cache.Put(cache_key, snapshot);
            FillFromSnapshot(snapshot, func_id, data_len, out_values);
            return;
        }

        LogError("BridgeIpcClient::Analyze failed for func_id=" + std::to_string(func_id) +
                 " data_len=" + std::to_string(data_len) + " error=" + error);
        FillDebugFallback(func_id, data_len, out_values);
    } catch (...) {
        if (out_values != nullptr && data_len > 0) {
            FillZeros(data_len, out_values);
        }
        try {
            LogError("HandleFunc caught unexpected exception for func_id=" +
                     std::to_string(func_id));
        } catch (...) {
        }
    }
}

PluginTCalcFuncInfo* GetFunctionRegistry() {
    return g_calc_funcs;
}

#ifdef TDX_CZSC_TESTING
uint64_t ComputeInputHashForTest(int data_len, float* in_high, float* in_low, float* in_close) {
    return ComputeInputHash(data_len, in_high, in_low, in_close);
}

uint64_t ComputeSegmentKeyForTest(int data_len, float* in_high, float* in_low) {
    return ComputeSegmentKey(data_len, in_high, in_low);
}

std::string PeriodCodeToFreqForTest(int code) {
    return PeriodCodeToFreq(code);
}

std::string CachedFreqHintForTest(int data_len, float* in_high, float* in_low) {
    const std::optional<std::string> freq =
        LookupPeriod(ComputeSegmentKey(data_len, in_high, in_low));
    return freq.value_or("");
}
#endif

}  // namespace tdx_czsc

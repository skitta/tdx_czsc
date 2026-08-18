#include "bridge_functions.h"

#include <array>
#include <atomic>
#include <string>
#include <vector>

#include "bridge_cache.h"
#include "bridge_config.h"
#include "bridge_ipc_client.h"
#include "bridge_logging.h"
#include "bridge_output_mapper.h"
#include "bridge_service_protocol.h"

namespace tdx_czsc {
namespace {

struct BridgeRuntime {
    explicit BridgeRuntime(BridgeConfig cfg)
        : config(std::move(cfg)), cache(config.dll_cache_capacity), client(config) {}

    BridgeConfig config;
    SnapshotCache cache{4};
    BridgeIpcClient client{};
    std::atomic_uint64_t request_counter{1};
};

BridgeRuntime& Runtime() {
    static BridgeRuntime runtime(LoadBridgeConfig());
    return runtime;
}

uint64_t ComputeInputHash(int data_len, float* in_high, float* in_low, float* in_close) {
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
    request.requested_func_ids = {101, 102, 201, 202, 203, 204, 301, 302, 303, 304, 401, 402, 403, 404, 405, 406, 901, 902, 903};
    request.data_len = data_len > 0 ? static_cast<uint32_t>(data_len) : 0;
    request.input_hash = ComputeInputHash(data_len, in_high, in_low, in_close);

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
            " last_low=" + (data_len > 0 ? std::to_string(in_low[data_len - 1]) : "?"));

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
DECLARE_FUNC_WRAPPER(401)
DECLARE_FUNC_WRAPPER(402)
DECLARE_FUNC_WRAPPER(403)
DECLARE_FUNC_WRAPPER(404)
DECLARE_FUNC_WRAPPER(405)
DECLARE_FUNC_WRAPPER(406)
DECLARE_FUNC_WRAPPER(901)
DECLARE_FUNC_WRAPPER(902)
DECLARE_FUNC_WRAPPER(903)

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
    {401, &Func401},
    {402, &Func402},
    {403, &Func403},
    {404, &Func404},
    {405, &Func405},
    {406, &Func406},
    {901, &Func901},
    {902, &Func902},
    {903, &Func903},
    {0, nullptr},
};

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
}

PluginTCalcFuncInfo* GetFunctionRegistry() {
    return g_calc_funcs;
}

}  // namespace tdx_czsc

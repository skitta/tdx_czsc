#pragma once

#include <cstdint>
#include <string>
#include <unordered_map>
#include <vector>

namespace tdx_czsc {

struct AnalyzeOptions {
    uint32_t max_bi_num = 50;
    uint32_t min_bi_len = 6;
    uint32_t analysis_window = 1000;
    bool include_channels = true;
    bool include_objects = false;
    bool enable_incremental = false;
};

struct AnalyzeRequestCpp {
    std::string protocol_version = "1.0.0";
    uint64_t request_id = 0;
    std::string func_group = "single_level_structure";
    std::vector<uint32_t> requested_func_ids;
    std::string symbol_hint = "TDX";
    std::string freq_hint = "F30";
    uint32_t data_len = 0;
    uint64_t input_hash = 0;
    bool prefer_cached = true;
    uint64_t caller_instance_id = 0;
    AnalyzeOptions options;
    std::vector<float> high;
    std::vector<float> low;
    std::vector<float> close;
    // 量能序列（func_id 950 量能输入通道缓存融合；为空表示公式未挂量能通道）
    std::vector<float> vol;
};

struct AnalysisSnapshotCpp {
    uint64_t input_hash = 0;
    uint32_t data_len = 0;
    bool service_cache_hit = false;
    uint32_t latency_ms = 0;
    std::unordered_map<uint32_t, std::vector<float>> channels;
};

}  // namespace tdx_czsc

#include <algorithm>
#include <cstdint>
#include <cstdlib>
#include <iomanip>
#include <iostream>
#include <string>
#include <vector>

#include "bridge_ipc_client.h"
#include "bridge_config.h"
#include "bridge_logging.h"
#include "bridge_service_protocol.h"

namespace {

std::string GetEnvOrDefault(const char* name, const std::string& fallback) {
    const char* value = std::getenv(name);
    if (value == nullptr || *value == '\0') {
        return fallback;
    }
    return value;
}

std::wstring Utf8ToWide(const std::string& input) {
    return std::wstring(input.begin(), input.end());
}

std::string WideToUtf8(const std::wstring& input) {
#ifdef _WIN32
    if (input.empty()) {
        return {};
    }
    const int size_needed = WideCharToMultiByte(CP_UTF8, 0, input.c_str(),
                                                static_cast<int>(input.size()), nullptr, 0,
                                                nullptr, nullptr);
    if (size_needed <= 0) {
        return {};
    }
    std::string result(static_cast<std::size_t>(size_needed), '\0');
    WideCharToMultiByte(CP_UTF8, 0, input.c_str(), static_cast<int>(input.size()),
                        result.data(), size_needed, nullptr, nullptr);
    return result;
#else
    return std::string(input.begin(), input.end());
#endif
}

std::vector<float> MakeLinearSeries(float start, float step, std::size_t len) {
    std::vector<float> values;
    values.reserve(len);
    for (std::size_t i = 0; i < len; ++i) {
        values.push_back(start + step * static_cast<float>(i));
    }
    return values;
}

void PrintChannelPreview(const std::unordered_map<std::uint32_t, std::vector<float>>& channels,
                         std::uint32_t func_id,
                         std::size_t max_items = 12) {
    const auto it = channels.find(func_id);
    if (it == channels.end()) {
        std::cout << "func_id=" << func_id << " channel missing\n";
        return;
    }

    std::cout << "func_id=" << func_id << " values=";
    const auto& values = it->second;
    const std::size_t count = std::min(max_items, values.size());
    for (std::size_t i = 0; i < count; ++i) {
        if (i > 0) {
            std::cout << ',';
        }
        std::cout << std::fixed << std::setprecision(2) << values[i];
    }
    if (values.size() > count) {
        std::cout << "...";
    }
    std::cout << '\n';
}

}  // namespace

int main() {
    constexpr std::size_t kLen = 64;

    const tdx_czsc::BridgeConfig config = tdx_czsc::LoadBridgeConfig();
    std::cout << "== 生效配置 ==" << '\n';
    std::cout << "  pipe_name            : " << WideToUtf8(config.pipe_name) << '\n';
    std::cout << "  startup_timeout_ms   : " << config.startup_timeout_ms << '\n';
    std::cout << "  request_timeout_ms   : " << config.request_timeout_ms << '\n';
    std::cout << "  connect_retry_count  : " << config.connect_retry_count << '\n';
    std::cout << "  connect_retry_int_ms : " << config.connect_retry_interval_ms << '\n';
    std::cout << "  max_bi_num           : " << config.max_bi_num << '\n';
    std::cout << "  min_bi_len           : " << config.min_bi_len << '\n';
    std::cout << "  analysis_window      : " << config.analysis_window << '\n';
    std::cout << "  dll_cache_capacity   : " << config.dll_cache_capacity << '\n';
    std::cout << '\n';

    const std::string pipe_name_utf8 = WideToUtf8(config.pipe_name);
    const std::string freq_hint = GetEnvOrDefault("TDX_CZSC_FREQ", "F30");
    const std::string symbol_hint = GetEnvOrDefault("TDX_CZSC_SYMBOL", "000001.SZ");

    tdx_czsc::AnalyzeRequestCpp request;
    request.request_id = 1;
    request.func_group = "single_level_structure";
    request.requested_func_ids = {101, 102, 201, 202, 203, 204, 301, 302, 303, 304, 401, 402, 403, 404, 405, 406, 901, 902, 903};
    request.symbol_hint = symbol_hint;
    request.freq_hint = freq_hint;
    request.data_len = static_cast<std::uint32_t>(kLen);
    request.input_hash = 0x1122334455667788ULL;
    request.prefer_cached = false;
    request.caller_instance_id = 42;
    request.options.max_bi_num = 50;
    request.options.min_bi_len = 3;
    request.options.analysis_window = static_cast<std::uint32_t>(kLen);
    request.options.include_channels = true;
    request.options.include_objects = false;
    request.options.enable_incremental = false;

    // 构造一组简单但非完全单调的测试数据，便于服务端产出分型/笔/中枢。
    request.close = {
        10.0f, 10.2f, 10.5f, 10.1f,  9.8f,  9.6f,  9.9f, 10.3f,
        10.7f, 10.4f, 10.1f,  9.7f,  9.5f,  9.9f, 10.4f, 10.9f,
        10.6f, 10.1f,  9.8f,  9.4f,  9.7f, 10.2f, 10.8f, 11.2f,
        10.9f, 10.5f, 10.0f,  9.6f,  9.9f, 10.3f, 10.7f, 11.0f,
        10.8f, 10.3f,  9.9f,  9.5f,  9.8f, 10.1f, 10.6f, 11.1f,
        10.7f, 10.2f,  9.7f,  9.3f,  9.6f, 10.0f, 10.5f, 10.9f,
        10.6f, 10.1f,  9.8f,  9.4f,  9.7f, 10.2f, 10.6f, 11.0f,
        10.8f, 10.4f, 10.0f,  9.7f, 10.0f, 10.4f, 10.9f, 11.3f,
    };

    request.high.reserve(kLen);
    request.low.reserve(kLen);
    for (std::size_t i = 0; i < kLen; ++i) {
        request.high.push_back(request.close[i] + 0.25f + static_cast<float>(i % 3) * 0.03f);
        request.low.push_back(request.close[i] - 0.22f - static_cast<float>(i % 2) * 0.02f);
    }

    tdx_czsc::BridgeIpcClient client(config);
    tdx_czsc::AnalysisSnapshotCpp snapshot;
    std::string error;

    std::cout << "log_file         : " << tdx_czsc::GetBridgeLogFilePath() << '\n';

    const bool ok = client.Analyze(request, &snapshot, &error);
    if (!ok) {
        tdx_czsc::LogError("ipc_smoke_test Analyze failed: " + error);
        std::cerr << "Analyze failed: " << error << '\n';
        return 1;
    }

    tdx_czsc::LogInfo("ipc_smoke_test Analyze succeeded");
    std::cout << "Analyze ok\n";
    std::cout << "  input_hash       : " << snapshot.input_hash << '\n';
    std::cout << "  data_len         : " << snapshot.data_len << '\n';
    std::cout << "  service_cache_hit: " << (snapshot.service_cache_hit ? "true" : "false") << '\n';
    std::cout << "  latency_ms       : " << snapshot.latency_ms << '\n';
    std::cout << "  channel_count    : " << snapshot.channels.size() << '\n';

    PrintChannelPreview(snapshot.channels, 101);
    PrintChannelPreview(snapshot.channels, 102);
    PrintChannelPreview(snapshot.channels, 201);
    PrintChannelPreview(snapshot.channels, 202);
    // 段端点/段方向（203/204）：段端点=段首笔起点与末笔终点分型价格
    PrintChannelPreview(snapshot.channels, 203);
    PrintChannelPreview(snapshot.channels, 204);
    PrintChannelPreview(snapshot.channels, 301);
    PrintChannelPreview(snapshot.channels, 302);
    PrintChannelPreview(snapshot.channels, 303);
    // 304 中枢边界: 左界=1 右界=2, 全量显示便于确认链路
    PrintChannelPreview(snapshot.channels, 304, 64);
    // 买卖点信号可能出现在窗口后半段，401~406 显示全量值便于确认。
    PrintChannelPreview(snapshot.channels, 401, 64);
    PrintChannelPreview(snapshot.channels, 402, 64);
    PrintChannelPreview(snapshot.channels, 403, 64);
    PrintChannelPreview(snapshot.channels, 404, 64);
    PrintChannelPreview(snapshot.channels, 405, 64);
    PrintChannelPreview(snapshot.channels, 406, 64);
    PrintChannelPreview(snapshot.channels, 901);
    PrintChannelPreview(snapshot.channels, 902);
    PrintChannelPreview(snapshot.channels, 903);

    return 0;
}

#include <algorithm>
#include <cstdint>
#include <cstdlib>
#include <iomanip>
#include <iostream>
#include <string>
#include <vector>

#include "bridge_functions.h"
#include "bridge_ipc_client.h"
#include "bridge_config.h"
#include "bridge_logging.h"
#include "bridge_service_protocol.h"

namespace {

std::string GetEnvOrDefault(const char* name, const std::string& fallback) {
#ifdef _WIN32
    // MSVC 将 getenv 标记为弃用（C4996），改用 CRT 安全 API。
    char* raw = nullptr;
    size_t len = 0;
    if (_dupenv_s(&raw, &len, name) == 0 && raw != nullptr) {
        std::string value(raw);
        std::free(raw);
        return value.empty() ? fallback : value;
    }
    return fallback;
#else
    const char* value = std::getenv(name);
    if (value == nullptr || *value == '\0') {
        return fallback;
    }
    return value;
#endif
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

    tdx_czsc::AnalyzeRequestCpp request;
    request.request_id = 1;
    request.func_group = "single_level_structure";
    request.requested_func_ids = {101, 102, 201, 202, 203, 204, 301, 302, 303, 304, 401, 402, 403, 404, 405, 406, 901, 902, 903};
    request.freq_hint = freq_hint;
    request.data_len = static_cast<std::uint32_t>(kLen);
    request.input_hash = 0x1122334455667788ULL;
    request.prefer_cached = false;
    request.options.max_bi_num = 50;
    request.options.min_bi_len = 3;
    request.options.analysis_window = static_cast<std::uint32_t>(kLen);
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

    // ==== 量能通道（950）验证：模拟公式先调 TDXDLL1(950, HIGH, LOW, VOL) ====
    std::cout << "\n== 950 量能输入通道 ==\n";
    PluginTCalcFuncInfo* func_950 = nullptr;
    for (PluginTCalcFuncInfo* p = tdx_czsc::GetFunctionRegistry();
         p != nullptr && p->pCallFunc != nullptr; ++p) {
        if (p->nFuncMark == 950) {
            func_950 = p;
            break;
        }
    }
    if (func_950 == nullptr) {
        std::cerr << "func_id=950 not registered\n";
        return 1;
    }

    // VOL 序列：与 high/low 等长、有区分度（模拟通达信 VOL）
    std::vector<float> vol;
    vol.reserve(kLen);
    for (std::size_t i = 0; i < kLen; ++i) {
        vol.push_back(1000.0f + static_cast<float>(i % 7) * 500.0f +
                      static_cast<float>((i * 13) % 11));
    }

    std::vector<float> vol_echo(kLen, 0.0f);
    func_950->pCallFunc(static_cast<int>(kLen), vol_echo.data(), request.high.data(),
                        request.low.data(), vol.data());
    const bool echo_ok =
        vol_echo.size() == vol.size() &&
        std::equal(vol_echo.begin(), vol_echo.end(), vol.begin());
    std::cout << "  Func950 echo==VOL : " << (echo_ok ? "true" : "false") << '\n';
    if (!echo_ok) {
        std::cerr << "Func950 output should echo the third input (VOL)\n";
        return 1;
    }

    // 950 之后走一次分析通道：请求应融合量能并经服务端正常处理。
    // 服务端 Windows 命名管道为 FIRST_PIPE_INSTANCE 单实例语义，
    // 处理请求期间无监听实例；间隔等待下一实例就绪再连接。
    Sleep(400);
    std::vector<float> out_401(kLen, 0.0f);
    tdx_czsc::HandleFunc(401, static_cast<int>(kLen), out_401.data(), request.high.data(),
                         request.low.data(), request.close.data());
    const bool has_signal = std::any_of(out_401.begin(), out_401.end(),
                                        [](float v) { return v != 0.0f; });
    std::cout << "  HandleFunc(401) after 950 : " << (has_signal ? "has signal" : "zero output") << '\n';

    Sleep(400);
    // 带量能请求（模拟 BuildRequest 融合后的请求）与分析请求对比：
    // 服务端应正常消费 vol 并返回与不带量能一致的结构通道集合。
    tdx_czsc::AnalyzeRequestCpp request_with_vol = request;
    request_with_vol.vol = vol;
    request_with_vol.input_hash = 0x99AABBCCDDEEFF00ULL;  // 融合后哈希（DLL 侧已纳入 vol）
    request_with_vol.prefer_cached = false;

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

    // 带量能请求：验证服务端 vol 消费链路正常（结构通道集合一致即可）
    tdx_czsc::AnalysisSnapshotCpp snapshot_with_vol;
    std::string vol_error;
    const bool vol_ok = client.Analyze(request_with_vol, &snapshot_with_vol, &vol_error);
    if (!vol_ok) {
        tdx_czsc::LogError("ipc_smoke_test Analyze(with vol) failed: " + vol_error);
        std::cerr << "Analyze(with vol) failed: " << vol_error << '\n';
        return 1;
    }
    std::cout << "Analyze(with vol) ok\n";
    std::cout << "  channel_count    : " << snapshot_with_vol.channels.size()
              << " (without vol: " << snapshot.channels.size() << ")\n";
    if (snapshot_with_vol.channels.size() != snapshot.channels.size()) {
        std::cerr << "channel set mismatch between with/without vol requests\n";
        return 1;
    }

    return 0;
}

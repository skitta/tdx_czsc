#include <cstdint>
#include <cstdio>
#include <string>

#include <Windows.h>

#include "bridge_config.h"

namespace {

int failures = 0;

void Check(bool cond, const char* what) {
    if (!cond) {
        std::printf("FAIL: %s\n", what);
        ++failures;
    }
}

void SetEnv(const char* name, const char* value) {
    SetEnvironmentVariableA(name, value);
}

void ClearEnv(const char* name) {
    SetEnvironmentVariableA(name, nullptr);
}

}  // namespace

int main() {
    // 全部环境变量清空：回到内置默认值。
    {
        const char* names[] = {
            "TDX_CZSC_PIPE",           "TDX_CZSC_STARTUP_TIMEOUT_MS",
            "TDX_CZSC_REQUEST_TIMEOUT_MS", "TDX_CZSC_RETRY_COUNT",
            "TDX_CZSC_RETRY_INTERVAL_MS",  "TDX_CZSC_MAX_BI_NUM",
            "TDX_CZSC_MIN_BI_LEN",         "TDX_CZSC_ANALYSIS_WINDOW",
            "TDX_CZSC_CACHE_CAPACITY",      "TDX_CZSC_SERVICE_EXE",
            "TDX_CZSC_SERVICE_STARTUP_WAIT_MS",
        };
        for (const char* name : names) {
            ClearEnv(name);
        }

        const tdx_czsc::BridgeConfig config = tdx_czsc::LoadBridgeConfig();
        Check(config.pipe_name == LR"(\\.\pipe\tdx_czsc_analyzer)", "default pipe_name");
        Check(config.request_timeout_ms == 3000, "default request_timeout_ms");
        Check(config.max_bi_num == 1000, "default max_bi_num");
        Check(config.min_bi_len == 6, "default min_bi_len");
        Check(config.analysis_window == 0, "default analysis_window");
        Check(config.dll_cache_capacity == 16, "default dll_cache_capacity");
        Check(!config.service_exe.empty(), "default service_exe resolved");
    }

    // 环境变量覆盖数字项与字符串项。
    {
        SetEnv("TDX_CZSC_REQUEST_TIMEOUT_MS", "1234");
        SetEnv("TDX_CZSC_MAX_BI_NUM", "77");
        SetEnv("TDX_CZSC_MIN_BI_LEN", "4");
        SetEnv("TDX_CZSC_ANALYSIS_WINDOW", "250");
        SetEnv("TDX_CZSC_CACHE_CAPACITY", "3");
        SetEnv("TDX_CZSC_PIPE", "\\\\.\\pipe\\tdx_czsc_test_config");

        const tdx_czsc::BridgeConfig config = tdx_czsc::LoadBridgeConfig();
        Check(config.request_timeout_ms == 1234, "env overrides request_timeout_ms");
        Check(config.max_bi_num == 77, "env overrides max_bi_num");
        Check(config.min_bi_len == 4, "env overrides min_bi_len");
        Check(config.analysis_window == 250, "env overrides analysis_window");
        Check(config.dll_cache_capacity == 3, "env overrides dll_cache_capacity");
        Check(config.pipe_name == L"\\\\.\\pipe\\tdx_czsc_test_config", "env overrides pipe_name");
    }

    // 非法值被忽略，保留默认值（不抛异常、不写 0）。
    {
        SetEnv("TDX_CZSC_REQUEST_TIMEOUT_MS", "abc");
        SetEnv("TDX_CZSC_CACHE_CAPACITY", "-1");
        SetEnv("TDX_CZSC_MAX_BI_NUM", "12x");

        const tdx_czsc::BridgeConfig config = tdx_czsc::LoadBridgeConfig();
        Check(config.request_timeout_ms == 3000, "invalid u32 keeps default");
        Check(config.max_bi_num == 1000, "trailing garbage keeps default");
        Check(config.dll_cache_capacity == 16, "negative size keeps default");
    }

    // 空字符串视为未设置。
    {
        SetEnv("TDX_CZSC_MIN_BI_LEN", "");
        const tdx_czsc::BridgeConfig config = tdx_czsc::LoadBridgeConfig();
        Check(config.min_bi_len == 6, "empty env value keeps default");
    }

    // 超长环境变量（旧实现 128 字节上限）应完整生效。
    {
        std::string long_pipe = "\\\\.\\pipe\\tdx_czsc_";
        long_pipe.append(300, 'x');
        SetEnv("TDX_CZSC_PIPE", long_pipe.c_str());

        const tdx_czsc::BridgeConfig config = tdx_czsc::LoadBridgeConfig();
        Check(config.pipe_name.size() == long_pipe.size(), "long env value kept whole");
        Check(config.pipe_name == std::wstring(long_pipe.begin(), long_pipe.end()),
              "long env value content");
    }

    if (failures != 0) {
        std::printf("config_test: %d failure(s)\n", failures);
        return 1;
    }
    std::printf("config_test: ok\n");
    return 0;
}

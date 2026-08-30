#include "bridge_config.h"

#include <algorithm>
#include <charconv>
#include <cstdlib>
#include <fstream>
#include <string>

#include <Windows.h>

namespace tdx_czsc {
namespace {

std::wstring ModuleDirectory() {
    wchar_t buffer[MAX_PATH]{};
    // 必须取 DLL 自身的模块句柄：GetModuleFileNameW(nullptr) 返回的是宿主
    // 进程（通达信 TdxW.exe）的路径，会错误地把配置/服务路径解析到通达信
    // 根目录而不是 T0002\dlls。用本模块内函数的地址反查模块句柄。
    HMODULE module = nullptr;
    GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                           GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                       reinterpret_cast<LPCWSTR>(&ModuleDirectory), &module);
    const DWORD copied = GetModuleFileNameW(module != nullptr ? module : nullptr, buffer, MAX_PATH);
    if (copied == 0 || copied >= MAX_PATH) {
        return {};
    }
    std::wstring path(buffer);
    const auto pos = path.find_last_of(L"\\/");
    if (pos != std::wstring::npos) {
        path.resize(pos);
    }
    return path;
}

std::string WtoUtf8(const std::wstring& input) {
    if (input.empty()) {
        return {};
    }
    const int size_needed =
        WideCharToMultiByte(CP_UTF8, 0, input.c_str(), static_cast<int>(input.size()), nullptr, 0,
                            nullptr, nullptr);
    if (size_needed <= 0) {
        return {};
    }
    std::string result(static_cast<std::size_t>(size_needed), '\0');
    WideCharToMultiByte(CP_UTF8, 0, input.c_str(), static_cast<int>(input.size()), result.data(),
                        size_needed, nullptr, nullptr);
    return result;
}

std::wstring Utf8ToW(const std::string& input) {
    if (input.empty()) {
        return {};
    }
    const int size_needed = MultiByteToWideChar(CP_UTF8, 0, input.c_str(),
                                                static_cast<int>(input.size()), nullptr, 0);
    if (size_needed <= 0) {
        return {};
    }
    std::wstring result(static_cast<std::size_t>(size_needed), L'\0');
    MultiByteToWideChar(CP_UTF8, 0, input.c_str(), static_cast<int>(input.size()), result.data(),
                        size_needed);
    return result;
}

std::string Trim(std::string s) {
    const auto is_space = [](unsigned char c) { return c == ' ' || c == '\t' || c == '\r'; };
    s.erase(s.begin(), std::find_if(s.begin(), s.end(), [&](unsigned char c) { return !is_space(c); }));
    s.erase(std::find_if(s.rbegin(), s.rend(), [&](unsigned char c) { return !is_space(c); }).base(),
            s.end());
    return s;
}

bool ParseUint(const std::string& text, uint32_t* out) {
    if (text.empty()) {
        return false;
    }
    uint32_t value = 0;
    const auto [ptr, ec] = std::from_chars(text.data(), text.data() + text.size(), value);
    if (ec != std::errc() || ptr != text.data() + text.size()) {
        return false;
    }
    *out = value;
    return true;
}

bool ParseSize(const std::string& text, std::size_t* out) {
    if (text.empty()) {
        return false;
    }
    std::size_t value = 0;
    const auto [ptr, ec] = std::from_chars(text.data(), text.data() + text.size(), value);
    if (ec != std::errc() || ptr != text.data() + text.size()) {
        return false;
    }
    *out = value;
    return true;
}

const char* EnvValue(const char* name) {
    static thread_local char buffer[128]{};
    const DWORD size = GetEnvironmentVariableA(name, buffer, sizeof(buffer));
    if (size == 0 || size >= sizeof(buffer)) {
        return nullptr;
    }
    return buffer;
}

/// 对单个配置项应用：INI 值 < 环境变量 < 调用方。
/// env/ini 均为空时保留默认值。
struct IniOverrides {
    std::string pipe_name;
    std::string startup_timeout_ms;
    std::string request_timeout_ms;
    std::string connect_retry_count;
    std::string connect_retry_interval_ms;
    std::string max_bi_num;
    std::string min_bi_len;
    std::string analysis_window;
    std::string dll_cache_capacity;
    std::string service_exe;
    std::string service_startup_wait_ms;
};

IniOverrides ParseIniFile(const std::wstring& ini_path) {
    IniOverrides overrides;
    std::ifstream file(ini_path);
    if (!file.is_open()) {
        return overrides;
    }

    std::string line;
    while (std::getline(file, line)) {
        line = Trim(line);
        if (line.empty() || line[0] == '#' || line[0] == ';' || line[0] == '[') {
            continue;
        }
        const auto eq = line.find('=');
        if (eq == std::string::npos) {
            continue;
        }
        const std::string key = Trim(line.substr(0, eq));
        const std::string value = Trim(line.substr(eq + 1));
        if (key == "pipe_name") {
            overrides.pipe_name = value;
        } else if (key == "startup_timeout_ms") {
            overrides.startup_timeout_ms = value;
        } else if (key == "request_timeout_ms") {
            overrides.request_timeout_ms = value;
        } else if (key == "connect_retry_count") {
            overrides.connect_retry_count = value;
        } else if (key == "connect_retry_interval_ms") {
            overrides.connect_retry_interval_ms = value;
        } else if (key == "max_bi_num") {
            overrides.max_bi_num = value;
        } else if (key == "min_bi_len") {
            overrides.min_bi_len = value;
        } else if (key == "analysis_window") {
            overrides.analysis_window = value;
        } else if (key == "dll_cache_capacity") {
            overrides.dll_cache_capacity = value;
        } else if (key == "service_exe") {
            overrides.service_exe = value;
        } else if (key == "service_startup_wait_ms") {
            overrides.service_startup_wait_ms = value;
        }
    }
    return overrides;
}

const char* PickEnv(const char* env_name, const std::string& ini_value) {
    const char* env = EnvValue(env_name);
    if (env != nullptr && *env != '\0') {
        return env;
    }
    return ini_value.empty() ? nullptr : ini_value.c_str();
}

}  // namespace

BridgeConfig LoadBridgeConfig() {
    BridgeConfig config;

    const IniOverrides ini = ParseIniFile(ModuleDirectory() + L"\\tdx_czsc_bridge.ini");

    uint32_t u32 = 0;
    std::size_t size = 0;

    if (const char* v = PickEnv("TDX_CZSC_PIPE", ini.pipe_name); v != nullptr) {
        config.pipe_name = Utf8ToW(v);
    }
    if (const char* v = PickEnv("TDX_CZSC_STARTUP_TIMEOUT_MS", ini.startup_timeout_ms);
        v != nullptr && ParseUint(v, &u32)) {
        config.startup_timeout_ms = u32;
    }
    if (const char* v = PickEnv("TDX_CZSC_REQUEST_TIMEOUT_MS", ini.request_timeout_ms);
        v != nullptr && ParseUint(v, &u32)) {
        config.request_timeout_ms = u32;
    }
    if (const char* v = PickEnv("TDX_CZSC_RETRY_COUNT", ini.connect_retry_count);
        v != nullptr && ParseUint(v, &u32)) {
        config.connect_retry_count = u32;
    }
    if (const char* v = PickEnv("TDX_CZSC_RETRY_INTERVAL_MS", ini.connect_retry_interval_ms);
        v != nullptr && ParseUint(v, &u32)) {
        config.connect_retry_interval_ms = u32;
    }
    if (const char* v = PickEnv("TDX_CZSC_MAX_BI_NUM", ini.max_bi_num);
        v != nullptr && ParseUint(v, &u32)) {
        config.max_bi_num = u32;
    }
    if (const char* v = PickEnv("TDX_CZSC_MIN_BI_LEN", ini.min_bi_len);
        v != nullptr && ParseUint(v, &u32)) {
        config.min_bi_len = u32;
    }
    if (const char* v = PickEnv("TDX_CZSC_ANALYSIS_WINDOW", ini.analysis_window);
        v != nullptr && ParseUint(v, &u32)) {
        config.analysis_window = u32;
    }
    if (const char* v = PickEnv("TDX_CZSC_CACHE_CAPACITY", ini.dll_cache_capacity);
        v != nullptr && ParseSize(v, &size)) {
        config.dll_cache_capacity = size;
    }

    // 服务可执行文件：默认 DLL 同目录 tdx_czsc_service.exe（与部署脚本拷贝位置一致）
    if (const char* v = PickEnv("TDX_CZSC_SERVICE_EXE", ini.service_exe); v != nullptr) {
        config.service_exe = Utf8ToW(v);
    } else {
        config.service_exe = ModuleDirectory() + L"\\tdx_czsc_service.exe";
    }
    if (const char* v = PickEnv("TDX_CZSC_SERVICE_STARTUP_WAIT_MS", ini.service_startup_wait_ms);
        v != nullptr && ParseUint(v, &u32)) {
        config.service_startup_wait_ms = u32;
    }

    return config;
}

}  // namespace tdx_czsc

#include "bridge_logging.h"

#include <chrono>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <mutex>
#include <sstream>
#include <string>

#ifdef _WIN32
#include <Windows.h>
#endif

namespace tdx_czsc {
namespace {

std::mutex& LogMutex() {
    static std::mutex mutex;
    return mutex;
}

std::string WideToUtf8(const std::wstring& input) {
#ifdef _WIN32
    if (input.empty()) {
        return {};
    }

    const int size_needed = WideCharToMultiByte(
        CP_UTF8, 0, input.c_str(), static_cast<int>(input.size()), nullptr, 0, nullptr, nullptr);
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

std::filesystem::path ResolveModuleDirectory() {
#ifdef _WIN32
    HMODULE module = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                                GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                            reinterpret_cast<LPCWSTR>(&ResolveModuleDirectory), &module)) {
        return std::filesystem::current_path();
    }

    std::wstring buffer(MAX_PATH, L'\0');
    DWORD copied = GetModuleFileNameW(module, buffer.data(), static_cast<DWORD>(buffer.size()));
    if (copied == 0) {
        return std::filesystem::current_path();
    }
    buffer.resize(copied);
    return std::filesystem::path(buffer).parent_path();
#else
    return std::filesystem::current_path();
#endif
}

std::string ResolveLogPath() {
    try {
        const auto dir = ResolveModuleDirectory() / "logs";
        std::filesystem::create_directories(dir);
        return (dir / "tdx_czsc_bridge.log").string();
    } catch (...) {
        return "tdx_czsc_bridge.log";
    }
}

std::string CurrentTimestamp() {
    using clock = std::chrono::system_clock;
    const auto now = clock::now();
    const auto time_t_now = clock::to_time_t(now);
    const auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(now.time_since_epoch()) % 1000;

    std::tm local_tm{};
#ifdef _WIN32
    localtime_s(&local_tm, &time_t_now);
#else
    localtime_r(&time_t_now, &local_tm);
#endif

    std::ostringstream oss;
    oss << std::put_time(&local_tm, "%Y-%m-%d %H:%M:%S")
        << '.' << std::setw(3) << std::setfill('0') << ms.count();
    return oss.str();
}

void WriteLine(const char* level, const std::string& message) {
    std::lock_guard<std::mutex> lock(LogMutex());
    const std::string path = ResolveLogPath();
    std::ofstream out(path, std::ios::out | std::ios::app);
    if (!out.is_open()) {
        return;
    }

    out << '[' << CurrentTimestamp() << "] [" << level << "] " << message << '\n';
}

}  // namespace

void LogDebug(const std::string& message) {
    WriteLine("DEBUG", message);
}

void LogInfo(const std::string& message) {
    WriteLine("INFO", message);
}

void LogWarn(const std::string& message) {
    WriteLine("WARN", message);
}

void LogError(const std::string& message) {
    WriteLine("ERROR", message);
}

std::string GetBridgeLogFilePath() {
    return ResolveLogPath();
}

}  // namespace tdx_czsc

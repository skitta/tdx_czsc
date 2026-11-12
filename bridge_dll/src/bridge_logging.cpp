#include "bridge_logging.h"

#include <chrono>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <mutex>
#include <sstream>
#include <string>

#include <Windows.h>

#include "bridge_text.h"

namespace tdx_czsc {
namespace {

/// 单个日志文件上限；超过后轮转为 .1（只保留一份历史），
/// 避免通达信长时间挂机把磁盘写满。
constexpr std::uintmax_t kMaxLogBytes = 8ull * 1024 * 1024;

std::mutex& LogMutex() {
    static std::mutex mutex;
    return mutex;
}

std::string ResolveLogPath() {
    try {
        const std::wstring module_dir = ModuleDirectory();
        const auto dir = module_dir.empty()
                             ? std::filesystem::current_path() / "logs"
                             : std::filesystem::path(module_dir) / "logs";
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
    localtime_s(&local_tm, &time_t_now);

    std::ostringstream oss;
    oss << std::put_time(&local_tm, "%Y-%m-%d %H:%M:%S")
        << '.' << std::setw(3) << std::setfill('0') << ms.count();
    return oss.str();
}

void WriteLine(const char* level, const std::string& message) {
    std::lock_guard<std::mutex> lock(LogMutex());
    const std::string path = ResolveLogPath();
    try {
        if (std::filesystem::file_size(path) >= kMaxLogBytes) {
            std::filesystem::path rotated = path;
            rotated += ".1";
            std::filesystem::remove(rotated);
            std::filesystem::rename(path, rotated);
        }
    } catch (...) {
        // 轮转失败不影响写日志本身
    }
    std::ofstream out(path, std::ios::out | std::ios::app);
    if (!out.is_open()) {
        return;
    }

    out << '[' << CurrentTimestamp() << "] [" << level << "] " << message << '\n';
}

}  // namespace

bool DebugLoggingEnabled() {
    static const bool enabled = [] {
        char value[8]{};
        const DWORD size = GetEnvironmentVariableA("TDX_CZSC_LOG_DEBUG", value, sizeof(value));
        return size == 1 && value[0] == '1';
    }();
    return enabled;
}

void LogDebug(const std::string& message) {
    // 热路径（每次公式调用都会记录）默认关闭，需要时设 TDX_CZSC_LOG_DEBUG=1。
    if (!DebugLoggingEnabled()) {
        return;
    }
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

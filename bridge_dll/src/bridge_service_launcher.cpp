#include "bridge_service_launcher.h"

#include <chrono>
#include <mutex>
#include <string>

#include "bridge_logging.h"
#include "bridge_text.h"

namespace tdx_czsc {
namespace {

/// 进程级服务拉起状态：节流与冷却，避免公式多通道并发调用时重复拉起/长时间阻塞。
std::mutex g_service_mutex;
std::chrono::steady_clock::time_point g_last_spawn{};
std::chrono::steady_clock::time_point g_spawn_fail_until{};
constexpr auto kSpawnThrottle = std::chrono::seconds(5);
constexpr auto kSpawnFailCooldown = std::chrono::seconds(30);

/// KILL_ON_JOB_CLOSE 任务对象：通达信进程退出（正常关闭或崩溃）时，
/// 最后一个 job 句柄随之关闭，Windows 自动终止挂在该 job 下的服务进程。
/// 由 g_service_mutex 保护。
HANDLE g_service_job = nullptr;

HANDLE EnsureServiceJob() {
    if (g_service_job != nullptr) {
        return g_service_job;
    }
    HANDLE job = CreateJobObjectW(nullptr, nullptr);
    if (job != nullptr) {
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION info{};
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if (!SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info, sizeof(info))) {
            LogWarn("EnsureServiceJob: SetInformationJobObject failed error=" +
                    std::to_string(GetLastError()));
            CloseHandle(job);
            job = nullptr;
        }
    }
    g_service_job = job;
    return job;
}

/// 拉起 Rust 服务进程。
///
/// - `bInheritHandles=FALSE` + `CREATE_NO_WINDOW`：不继承调用方（通达信/测试进程）的
///   输出管道等句柄，避免子进程句柄导致父进程管道永不关闭；
/// - 服务日志由服务自身通过 `--log <路径>` 写入（exe 同目录 tdx_czsc_service.log）；
/// - 服务挂到 KILL_ON_JOB_CLOSE 任务对象，通达信退出时服务随之退出。
/// 任何失败返回 false，调用方安全降级为零输出。
bool LaunchService(const std::wstring& exe_path) {
    if (exe_path.empty()) {
        LogWarn("LaunchService: service_exe is empty");
        return false;
    }
    if (GetFileAttributesW(exe_path.c_str()) == INVALID_FILE_ATTRIBUTES) {
        LogWarn("LaunchService: service exe not found: " + WideToUtf8(exe_path));
        return false;
    }

    // 日志文件：<exe目录>\tdx_czsc_service.log
    std::wstring log_path;
    const auto slash = exe_path.find_last_of(L"\\/");
    if (slash != std::wstring::npos) {
        log_path = exe_path.substr(0, slash + 1) + L"tdx_czsc_service.log";
    }

    // 命令行需可写；路径含空格时加引号
    std::wstring command_line = L"\"" + exe_path + L"\"";
    if (!log_path.empty()) {
        command_line += L" --log \"" + log_path + L"\"";
    }

    STARTUPINFOW si{};
    si.cb = sizeof(si);
    PROCESS_INFORMATION pi{};
    const BOOL ok = CreateProcessW(nullptr, command_line.data(), nullptr, nullptr,
                                   /*bInheritHandles=*/FALSE, CREATE_NO_WINDOW, nullptr,
                                   nullptr, &si, &pi);
    if (!ok) {
        LogError("LaunchService: CreateProcessW failed for " + WideToUtf8(exe_path) +
                 " error=" + std::to_string(GetLastError()));
        return false;
    }

    // 服务挂到 KILL_ON_JOB_CLOSE 任务对象：通达信退出时服务自动退出。
    // 若失败（如调用方进程本身在受限 job 中不允许嵌套）则降级为独立运行。
    if (HANDLE job = EnsureServiceJob(); job != nullptr &&
        !AssignProcessToJobObject(job, pi.hProcess)) {
        LogWarn("LaunchService: AssignProcessToJobObject failed error=" +
                std::to_string(GetLastError()) + " (service will run standalone)");
    }

    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);
    LogInfo("LaunchService: spawned " + WideToUtf8(exe_path));
    return true;
}

}  // namespace

bool PipeExists(const std::wstring& pipe_name) {
    if (WaitNamedPipeW(pipe_name.c_str(), 0)) {
        return true;
    }
    const DWORD error = GetLastError();
    return error != ERROR_FILE_NOT_FOUND && error != ERROR_BAD_PATHNAME;
}

HANDLE EnsureServiceAndConnect(const std::wstring& pipe_name,
                               const std::wstring& exe_path,
                               std::uint32_t startup_wait_ms,
                               const std::function<HANDLE()>& connect_fn,
                               std::string* out_error) {
    const auto now = std::chrono::steady_clock::now();
    bool spawn_failed = false;
    {
        std::lock_guard<std::mutex> lock(g_service_mutex);
        if (now < g_spawn_fail_until) {
            // 最近一次拉起失败/超时，冷却期内快速失败
            if (out_error != nullptr) {
                *out_error = "service spawn in cooldown; check service_exe / service log";
            }
            return INVALID_HANDLE_VALUE;
        }
        if (now - g_last_spawn >= kSpawnThrottle) {
            g_last_spawn = now;
            if (!LaunchService(exe_path)) {
                g_spawn_fail_until = now + kSpawnFailCooldown;
                spawn_failed = true;
            }
        }
        // 被节流（其它调用刚拉起过）时不重复拉起，继续等待其就绪
    }

    // 本次拉起失败（exe 缺失/进程创建失败）：立即返回，不阻塞公式计算
    if (spawn_failed) {
        if (out_error != nullptr) {
            *out_error = "failed to launch service: " + WideToUtf8(exe_path);
            LogWarn(*out_error);
        }
        return INVALID_HANDLE_VALUE;
    }

    // 等待服务就绪（管道出现），最多 startup_wait_ms
    const auto deadline = now + std::chrono::milliseconds(startup_wait_ms);
    while (std::chrono::steady_clock::now() < deadline) {
        if (WaitNamedPipeW(pipe_name.c_str(), 0)) {
            HANDLE connected = connect_fn();
            if (connected != INVALID_HANDLE_VALUE) {
                return connected;
            }
            // 管道存在但连接失败（如实例繁忙）→ 稍后重试
        }
        Sleep(200);
    }

    // 超时：进入冷却，避免后续每个通道调用都长时间阻塞
    {
        std::lock_guard<std::mutex> lock(g_service_mutex);
        g_spawn_fail_until = std::chrono::steady_clock::now() + kSpawnFailCooldown;
    }
    if (out_error != nullptr) {
        *out_error = "service did not become ready within " + std::to_string(startup_wait_ms) + "ms";
        LogWarn(*out_error);
    }
    return INVALID_HANDLE_VALUE;
}

}  // namespace tdx_czsc

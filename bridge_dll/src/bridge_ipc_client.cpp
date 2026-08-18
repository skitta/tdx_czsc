// analysis.pb.h 必须先于 bridge_ipc_client.h 包含：
// 后者会引入 <Windows.h>，其宏（min/max 等）会破坏 protobuf 生成头。
#include "analysis.pb.h"

#include "bridge_ipc_client.h"

#include <algorithm>
#include <chrono>
#include <cstring>
#include <mutex>
#include <sstream>
#include <string>
#include <vector>

#include "bridge_logging.h"

#ifdef _WIN32
#include <Windows.h>
#endif

namespace tdx_czsc {
namespace {

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

void FillRepeatedFloat(const std::vector<float>& src, google::protobuf::RepeatedField<float>* dst) {
    if (dst == nullptr) {
        return;
    }
    dst->Reserve(static_cast<int>(src.size()));
    for (float value : src) {
        dst->Add(value);
    }
}

void FillRepeatedUInt32(const std::vector<std::uint32_t>& src, google::protobuf::RepeatedField<std::uint32_t>* dst) {
    if (dst == nullptr) {
        return;
    }
    dst->Reserve(static_cast<int>(src.size()));
    for (std::uint32_t value : src) {
        dst->Add(value);
    }
}

void CopyChannelsFromProto(const tdx::czsc::v1::AnalysisSnapshot& proto_snapshot,
                           AnalysisSnapshotCpp* out_snapshot) {
    out_snapshot->channels.clear();
    for (const auto& channel : proto_snapshot.channels()) {
        std::vector<float> values;
        values.reserve(static_cast<std::size_t>(channel.values_size()));
        for (float value : channel.values()) {
            values.push_back(value);
        }
        out_snapshot->channels.emplace(channel.func_id(), std::move(values));
    }
}

void BuildAnalyzeEnvelope(const AnalyzeRequestCpp& request, tdx::czsc::v1::RequestEnvelope* envelope) {
    envelope->set_protocol_version(request.protocol_version);
    envelope->set_request_id(request.request_id);

    auto* analyze = envelope->mutable_analyze();
    analyze->set_func_group(request.func_group);
    FillRepeatedUInt32(request.requested_func_ids, analyze->mutable_requested_func_ids());
    analyze->set_symbol_hint(request.symbol_hint);
    analyze->set_freq_hint(request.freq_hint);
    analyze->set_data_len(request.data_len);
    analyze->set_input_hash(request.input_hash);
    analyze->set_bar_build_mode(tdx::czsc::v1::BAR_BUILD_FROM_HLC3);
    analyze->set_prefer_cached(request.prefer_cached);
    analyze->set_caller_instance_id(request.caller_instance_id);

    auto* series = analyze->mutable_series();
    FillRepeatedFloat(request.high, series->mutable_high());
    FillRepeatedFloat(request.low, series->mutable_low());
    FillRepeatedFloat(request.close, series->mutable_close());

    auto* options = analyze->mutable_options();
    options->set_max_bi_num(request.options.max_bi_num);
    options->set_min_bi_len(request.options.min_bi_len);
    options->set_analysis_window(request.options.analysis_window);
    options->set_include_channels(request.options.include_channels);
    options->set_include_objects(request.options.include_objects);
    options->set_enable_incremental(request.options.enable_incremental);
}

bool ParseAnalyzeResponse(const tdx::czsc::v1::ResponseEnvelope& envelope,
                          AnalysisSnapshotCpp* out_snapshot,
                          std::string* out_error) {
    if (envelope.status() != tdx::czsc::v1::STATUS_OK) {
        if (out_error != nullptr) {
            std::ostringstream oss;
            oss << "service returned status=" << envelope.status()
                << " error=" << envelope.error_message();
            *out_error = oss.str();
        }
        return false;
    }

    if (!envelope.has_analyze()) {
        if (out_error != nullptr) {
            *out_error = "response body does not contain analyze";
        }
        return false;
    }

    const auto& analyze = envelope.analyze();
    if (!analyze.has_snapshot()) {
        if (out_error != nullptr) {
            *out_error = "analyze response does not contain snapshot";
        }
        return false;
    }

    const auto& snapshot = analyze.snapshot();
    out_snapshot->service_cache_hit = analyze.service_cache_hit();
    out_snapshot->latency_ms = analyze.has_timing() ? analyze.timing().total_ms() : 0;

    if (snapshot.has_meta()) {
        out_snapshot->input_hash = snapshot.meta().input_hash();
        out_snapshot->data_len = snapshot.meta().data_len();
    } else {
        out_snapshot->input_hash = 0;
        out_snapshot->data_len = 0;
    }

    CopyChannelsFromProto(snapshot, out_snapshot);
    return true;
}

}  // namespace

BridgeIpcClient::BridgeIpcClient(BridgeConfig config) : config_(std::move(config)) {}

bool BridgeIpcClient::Analyze(const AnalyzeRequestCpp& request,
                              AnalysisSnapshotCpp* out_snapshot,
                              std::string* out_error) {
#ifdef _WIN32
    return AnalyzeImpl(request, out_snapshot, out_error);
#else
    (void)request;
    (void)out_snapshot;
    if (out_error != nullptr) {
        *out_error = "BridgeIpcClient is only available on Windows";
    }
    return false;
#endif
}

bool BridgeIpcClient::AnalyzeImpl(const AnalyzeRequestCpp& request,
                                  AnalysisSnapshotCpp* out_snapshot,
                                  std::string* out_error) {
#ifdef _WIN32
    if (out_snapshot == nullptr) {
        if (out_error != nullptr) {
            *out_error = "out_snapshot is null";
        }
        LogError("BridgeIpcClient::AnalyzeImpl received null out_snapshot");
        return false;
    }

    tdx::czsc::v1::RequestEnvelope request_envelope;
    BuildAnalyzeEnvelope(request, &request_envelope);

    std::vector<std::uint8_t> request_payload(static_cast<std::size_t>(request_envelope.ByteSizeLong()));
    if (!request_envelope.SerializeToArray(request_payload.data(), static_cast<int>(request_payload.size()))) {
        if (out_error != nullptr) {
            *out_error = "failed to serialize RequestEnvelope";
        }
        LogError("failed to serialize RequestEnvelope");
        return false;
    }

    HANDLE pipe = ConnectWithRetry(out_error);
    if (pipe == INVALID_HANDLE_VALUE) {
        return false;
    }

    const auto close_pipe = [&pipe]() {
        if (pipe != INVALID_HANDLE_VALUE) {
            CloseHandle(pipe);
            pipe = INVALID_HANDLE_VALUE;
        }
    };

    if (!WriteFrame(pipe, request_payload, out_error)) {
        close_pipe();
        return false;
    }

    std::vector<std::uint8_t> response_payload;
    if (!ReadFrame(pipe, &response_payload, out_error)) {
        close_pipe();
        return false;
    }
    close_pipe();

    tdx::czsc::v1::ResponseEnvelope response_envelope;
    if (!response_envelope.ParseFromArray(response_payload.data(), static_cast<int>(response_payload.size()))) {
        if (out_error != nullptr) {
            *out_error = "failed to parse ResponseEnvelope";
            LogError(*out_error);
        }
        return false;
    }

    const bool ok = ParseAnalyzeResponse(response_envelope, out_snapshot, out_error);
    if (!ok && out_error != nullptr) {
        LogError(*out_error);
    }
    return ok;
#else
    (void)request;
    (void)out_snapshot;
    if (out_error != nullptr) {
        *out_error = "BridgeIpcClient is only available on Windows";
    }
    return false;
#endif
}

#ifdef _WIN32

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

/// 判断命名管道是否存在（服务是否在运行）。
bool PipeExists(const std::wstring& pipe_name) {
    if (WaitNamedPipeW(pipe_name.c_str(), 0)) {
        return true;
    }
    const DWORD error = GetLastError();
    return error != ERROR_FILE_NOT_FOUND && error != ERROR_BAD_PATHNAME;
}

/// 拉起 Rust 服务进程。
///
/// - `bInheritHandles=FALSE` + `CREATE_NO_WINDOW`：不继承调用方（通达信/测试进程）
///   的输出管道等句柄，避免子进程句柄导致父进程管道永不关闭；
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

/// 一次带重试的连接尝试（快速路径，服务已运行时使用）。
HANDLE BridgeIpcClient::TryConnect(std::string* out_error) const {
    const std::string pipe_utf8 = WideToUtf8(config_.pipe_name);
    const uint32_t attempts = config_.connect_retry_count + 1;

    for (uint32_t attempt = 0; attempt < attempts; ++attempt) {
        // WaitNamedPipeW 只在服务端存在并等待连接时成功；服务刚启动时
        // 首次可能尚未就绪，超时用 startup_timeout_ms。
        if (!WaitNamedPipeW(config_.pipe_name.c_str(), config_.startup_timeout_ms)) {
            if (attempt + 1 < attempts) {
                Sleep(config_.connect_retry_interval_ms);
                continue;
            }
            if (out_error != nullptr) {
                std::ostringstream oss;
                oss << "WaitNamedPipeW failed for pipe " << pipe_utf8
                    << " error=" << GetLastError() << " (service not running?)";
                *out_error = oss.str();
                LogWarn(*out_error);
            }
            return INVALID_HANDLE_VALUE;
        }

        HANDLE pipe = CreateFileW(config_.pipe_name.c_str(), GENERIC_READ | GENERIC_WRITE, 0,
                                  nullptr, OPEN_EXISTING, FILE_FLAG_OVERLAPPED, nullptr);
        if (pipe != INVALID_HANDLE_VALUE) {
            return pipe;
        }

        const DWORD error = GetLastError();
        if (attempt + 1 < attempts) {
            Sleep(config_.connect_retry_interval_ms);
            continue;
        }
        if (out_error != nullptr) {
            std::ostringstream oss;
            oss << "CreateFileW failed for pipe " << pipe_utf8 << " error=" << error;
            *out_error = oss.str();
            LogError(*out_error);
        }
        return INVALID_HANDLE_VALUE;
    }
    return INVALID_HANDLE_VALUE;
}

/// 连接（带服务自动拉起）：
/// 1. 快速路径：服务已在运行，直接按原重试参数连接；
/// 2. 管道不存在（服务未运行）→ 节流拉起服务（5s 内最多一次），等待其就绪
///    后重连；拉起失败/超时进入 30s 冷却，冷却期内快速失败不阻塞公式计算。
/// 任何失败都返回 INVALID_HANDLE_VALUE，由调用方安全降级为零输出。
HANDLE BridgeIpcClient::ConnectWithRetry(std::string* out_error) const {
    // 1) 快速路径
    HANDLE pipe = TryConnect(out_error);
    if (pipe != INVALID_HANDLE_VALUE) {
        return pipe;
    }

    // 2) 仅当"服务未运行"（管道不存在）才尝试拉起；其它错误不处理
    if (PipeExists(config_.pipe_name)) {
        return INVALID_HANDLE_VALUE;
    }

    const auto now = std::chrono::steady_clock::now();
    bool spawned = false;
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
            spawned = LaunchService(config_.service_exe);
            if (!spawned) {
                g_spawn_fail_until = now + kSpawnFailCooldown;
                spawn_failed = true;
            }
        }
        // 被节流（其它调用刚拉起过）时不重复拉起，继续等待其就绪
    }

    // 本次拉起失败（exe 缺失/进程创建失败）：立即返回，不阻塞公式计算
    if (spawn_failed) {
        if (out_error != nullptr) {
            *out_error = "failed to launch service: " + WideToUtf8(config_.service_exe);
            LogWarn(*out_error);
        }
        return INVALID_HANDLE_VALUE;
    }

    // 3) 等待服务就绪（管道出现），最多 service_startup_wait_ms
    const auto deadline = now + std::chrono::milliseconds(config_.service_startup_wait_ms);
    while (std::chrono::steady_clock::now() < deadline) {
        if (WaitNamedPipeW(config_.pipe_name.c_str(), 0)) {
            HANDLE connected = TryConnect(nullptr);
            if (connected != INVALID_HANDLE_VALUE) {
                return connected;
            }
            // 管道存在但连接失败（如实例繁忙）→ 稍后重试
        }
        Sleep(200);
    }

    // 4) 超时：进入冷却，避免后续每个通道调用都长时间阻塞
    {
        std::lock_guard<std::mutex> lock(g_service_mutex);
        g_spawn_fail_until = std::chrono::steady_clock::now() + kSpawnFailCooldown;
    }
    if (out_error != nullptr) {
        *out_error = "service did not become ready within " +
                     std::to_string(config_.service_startup_wait_ms) + "ms";
        LogWarn(*out_error);
    }
    return INVALID_HANDLE_VALUE;
}

/// 对管道句柄执行一次 OVERLAPPED 读写，超时则取消并返回 false。
/// 返回本次实际传输字节数。
bool BridgeIpcClient::IoWithTimeout(HANDLE pipe,
                                    bool is_write,
                                    void* buffer,
                                    DWORD bytes_to_transfer,
                                    DWORD* out_transferred,
                                    std::string* out_error) const {
    OVERLAPPED overlapped{};
    overlapped.hEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    if (overlapped.hEvent == nullptr) {
        if (out_error != nullptr) {
            *out_error = "CreateEventW failed";
        }
        return false;
    }

    const auto cleanup = [&overlapped]() { CloseHandle(overlapped.hEvent); };

    BOOL ok = is_write ? WriteFile(pipe, buffer, bytes_to_transfer, nullptr, &overlapped)
                       : ReadFile(pipe, buffer, bytes_to_transfer, nullptr, &overlapped);
    if (!ok && GetLastError() == ERROR_IO_PENDING) {
        const DWORD wait = WaitForSingleObject(overlapped.hEvent, config_.request_timeout_ms);
        if (wait == WAIT_TIMEOUT) {
            CancelIoEx(pipe, &overlapped);
            cleanup();
            if (out_error != nullptr) {
                *out_error = std::string(is_write ? "WriteFile" : "ReadFile") +
                             " timed out after " + std::to_string(config_.request_timeout_ms) + "ms";
                LogWarn(*out_error);
            }
            return false;
        }
        if (wait != WAIT_OBJECT_0) {
            CancelIoEx(pipe, &overlapped);
            cleanup();
            if (out_error != nullptr) {
                *out_error = "WaitForSingleObject failed error=" + std::to_string(GetLastError());
            }
            return false;
        }
        ok = GetOverlappedResult(pipe, &overlapped, out_transferred, FALSE);
    } else if (!ok) {
        cleanup();
        if (out_error != nullptr) {
            std::ostringstream oss;
            oss << (is_write ? "WriteFile" : "ReadFile") << " failed error=" << GetLastError();
            *out_error = oss.str();
        }
        return false;
    } else {
        // 同步完成（极少见），WriteFile/ReadFile 未写入 transferred 参数。
        *out_transferred = bytes_to_transfer;
    }

    cleanup();
    return ok;
}

bool BridgeIpcClient::WriteFrame(void* pipe_handle,
                                 const std::vector<std::uint8_t>& payload,
                                 std::string* out_error) {
    HANDLE pipe = static_cast<HANDLE>(pipe_handle);
    const std::uint32_t payload_size = static_cast<std::uint32_t>(payload.size());

    DWORD written = 0;
    if (!IoWithTimeout(pipe, true, const_cast<uint32_t*>(&payload_size), sizeof(payload_size),
                       &written, out_error) ||
        written != sizeof(payload_size)) {
        return false;
    }

    std::size_t offset = 0;
    while (offset < payload.size()) {
        const DWORD chunk = static_cast<DWORD>(
            std::min<std::size_t>(payload.size() - offset, 64 * 1024));
        if (!IoWithTimeout(pipe, true, const_cast<uint8_t*>(payload.data() + offset), chunk,
                           &written, out_error) ||
            written == 0) {
            return false;
        }
        offset += written;
    }
    return true;
}

bool BridgeIpcClient::ReadFrame(void* pipe_handle,
                                std::vector<std::uint8_t>* out_payload,
                                std::string* out_error) {
    if (out_payload == nullptr) {
        if (out_error != nullptr) {
            *out_error = "out_payload is null";
        }
        return false;
    }

    HANDLE pipe = static_cast<HANDLE>(pipe_handle);
    std::uint32_t payload_size = 0;
    DWORD read = 0;

    if (!IoWithTimeout(pipe, false, &payload_size, sizeof(payload_size), &read, out_error) ||
        read != sizeof(payload_size)) {
        return false;
    }

    out_payload->assign(payload_size, 0);
    std::size_t offset = 0;
    while (offset < out_payload->size()) {
        const DWORD chunk = static_cast<DWORD>(
            std::min<std::size_t>(out_payload->size() - offset, 64 * 1024));
        if (!IoWithTimeout(pipe, false, out_payload->data() + offset, chunk, &read, out_error) ||
            read == 0) {
            return false;
        }
        offset += read;
    }
    return true;
}
#endif

}  // namespace tdx_czsc

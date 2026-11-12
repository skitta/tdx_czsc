// bridge_proto_mapping.h 会先引入 analysis.pb.h：Windows.h 的宏会破坏
// protobuf 生成头，因此必须在 bridge_ipc_client.h 之前包含。
#include "bridge_proto_mapping.h"

#include "bridge_ipc_client.h"

#include <algorithm>
#include <sstream>
#include <string>
#include <vector>

#include "bridge_logging.h"
#include "bridge_service_launcher.h"
#include "bridge_text.h"

#include <Windows.h>

namespace tdx_czsc {
namespace {

/// 单帧响应体上限：服务端正常响应远小于此值，超过即视为协议错位/异常对端，
/// 避免一个损坏的 4 字节长度让 DLL 按 4GB 分配内存。
constexpr std::uint32_t kMaxResponseFrameBytes = 64u * 1024u * 1024u;

}  // namespace

BridgeIpcClient::BridgeIpcClient(BridgeConfig config) : config_(std::move(config)) {}

bool BridgeIpcClient::Analyze(const AnalyzeRequestCpp& request,
                              AnalysisSnapshotCpp* out_snapshot,
                              std::string* out_error) {
    return AnalyzeImpl(request, out_snapshot, out_error);
}

bool BridgeIpcClient::AnalyzeImpl(const AnalyzeRequestCpp& request,
                                  AnalysisSnapshotCpp* out_snapshot,
                                  std::string* out_error) {
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
}

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

    // 3) 节流拉起 + 等待就绪，就绪后由 connect_fn 重连
    return EnsureServiceAndConnect(
        config_.pipe_name, config_.service_exe, config_.service_startup_wait_ms,
        [this]() { return TryConnect(nullptr); }, out_error);
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
    // 取消是异步的：必须等操作真正结束后才能释放缓冲区与事件，
    // 否则内核可能继续写入调用方已释放的栈/堆内存。
    const auto cancel_and_reap = [&]() {
        CancelIoEx(pipe, &overlapped);
        DWORD reaped = 0;
        GetOverlappedResult(pipe, &overlapped, &reaped, TRUE);
    };

    BOOL ok = is_write ? WriteFile(pipe, buffer, bytes_to_transfer, nullptr, &overlapped)
                       : ReadFile(pipe, buffer, bytes_to_transfer, nullptr, &overlapped);
    if (!ok && GetLastError() == ERROR_IO_PENDING) {
        const DWORD wait = WaitForSingleObject(overlapped.hEvent, config_.request_timeout_ms);
        if (wait == WAIT_TIMEOUT) {
            cancel_and_reap();
            cleanup();
            if (out_error != nullptr) {
                *out_error = std::string(is_write ? "WriteFile" : "ReadFile") +
                             " timed out after " + std::to_string(config_.request_timeout_ms) + "ms";
                LogWarn(*out_error);
            }
            return false;
        }
        if (wait != WAIT_OBJECT_0) {
            cancel_and_reap();
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

bool BridgeIpcClient::WriteFrame(HANDLE pipe,
                                 const std::vector<std::uint8_t>& payload,
                                 std::string* out_error) {
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

bool BridgeIpcClient::ReadFrame(HANDLE pipe,
                                std::vector<std::uint8_t>* out_payload,
                                std::string* out_error) {
    if (out_payload == nullptr) {
        if (out_error != nullptr) {
            *out_error = "out_payload is null";
        }
        return false;
    }

    std::uint32_t payload_size = 0;
    DWORD read = 0;

    if (!IoWithTimeout(pipe, false, &payload_size, sizeof(payload_size), &read, out_error) ||
        read != sizeof(payload_size)) {
        return false;
    }

    if (payload_size > kMaxResponseFrameBytes) {
        if (out_error != nullptr) {
            *out_error = "response frame too large: " + std::to_string(payload_size) + " bytes";
        }
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

}  // namespace tdx_czsc

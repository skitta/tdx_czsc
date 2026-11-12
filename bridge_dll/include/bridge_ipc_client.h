#pragma once

#include <cstdint>
#include <string>
#include <vector>

// Windows.h 的 min/max 宏会破坏 protobuf 生成头（numeric_limits::max()），
// 这里强制 NOMINMAX 保护，不依赖编译选项。
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <Windows.h>

#include "bridge_config.h"
#include "bridge_service_protocol.h"

namespace tdx_czsc {

class BridgeIpcClient {
public:
    explicit BridgeIpcClient(BridgeConfig config = BridgeConfig{});

    bool Analyze(const AnalyzeRequestCpp& request, AnalysisSnapshotCpp* out_snapshot, std::string* out_error);

private:
    bool AnalyzeImpl(const AnalyzeRequestCpp& request, AnalysisSnapshotCpp* out_snapshot, std::string* out_error);

    HANDLE ConnectWithRetry(std::string* out_error) const;
    /// 一次带重试的连接尝试（快速路径）。
    HANDLE TryConnect(std::string* out_error) const;
    bool IoWithTimeout(HANDLE pipe, bool is_write, void* buffer, DWORD bytes_to_transfer,
                       DWORD* out_transferred, std::string* out_error) const;
    bool WriteFrame(HANDLE pipe, const std::vector<std::uint8_t>& payload, std::string* out_error);
    bool ReadFrame(HANDLE pipe, std::vector<std::uint8_t>* out_payload, std::string* out_error);

    BridgeConfig config_;
};

}  // namespace tdx_czsc

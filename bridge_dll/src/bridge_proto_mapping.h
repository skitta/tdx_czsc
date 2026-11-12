#pragma once

// analysis.pb.h 必须早于任何引入 <Windows.h> 的头：Windows.h 的宏会破坏
// protobuf 生成头（numeric_limits::max()）。本头文件只依赖协议结构与 proto，
// 不包含 Windows.h，供 bridge_ipc_client.cpp 复用。
#include "analysis.pb.h"

#include <string>

#include "bridge_service_protocol.h"

namespace tdx_czsc {

void BuildAnalyzeEnvelope(const AnalyzeRequestCpp& request, tdx::czsc::v1::RequestEnvelope* envelope);

bool ParseAnalyzeResponse(const tdx::czsc::v1::ResponseEnvelope& envelope,
                          AnalysisSnapshotCpp* out_snapshot,
                          std::string* out_error);

}  // namespace tdx_czsc

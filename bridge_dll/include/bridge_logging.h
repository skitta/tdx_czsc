#pragma once

#include <string>

namespace tdx_czsc {

/// 调试日志是否开启（TDX_CZSC_LOG_DEBUG=1）。热路径可先用它跳过消息拼接。
bool DebugLoggingEnabled();

void LogDebug(const std::string& message);
void LogInfo(const std::string& message);
void LogWarn(const std::string& message);
void LogError(const std::string& message);

std::string GetBridgeLogFilePath();

}  // namespace tdx_czsc

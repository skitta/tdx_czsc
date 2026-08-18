#pragma once

#include <string>

namespace tdx_czsc {

void LogDebug(const std::string& message);
void LogInfo(const std::string& message);
void LogWarn(const std::string& message);
void LogError(const std::string& message);

std::string GetBridgeLogFilePath();

}  // namespace tdx_czsc

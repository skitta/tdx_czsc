#pragma once

#include <cstdint>
#include <functional>
#include <string>

#include <Windows.h>

namespace tdx_czsc {

/// 判断命名管道是否存在（服务是否在运行）。
bool PipeExists(const std::wstring& pipe_name);

/// 服务未运行时按节流（5s）/冷却（30s）策略拉起，并等待管道就绪（最长
/// startup_wait_ms）；每次就绪后调用 connect_fn 尝试取连接，失败则 200ms 后重试。
/// 返回 INVALID_HANDLE_VALUE 表示失败，原因写入 out_error。
HANDLE EnsureServiceAndConnect(const std::wstring& pipe_name,
                               const std::wstring& exe_path,
                               std::uint32_t startup_wait_ms,
                               const std::function<HANDLE()>& connect_fn,
                               std::string* out_error);

}  // namespace tdx_czsc

#pragma once

#include <string>

namespace tdx_czsc {

/// 本 DLL 所在目录（不带结尾反斜杠）。取不到模块句柄或路径超长时返回空串。
///
/// 必须取 DLL 自身的模块句柄：GetModuleFileNameW(nullptr) 返回宿主进程
/// （通达信 TdxW.exe）的路径，会错误地把配置/服务路径解析到通达信根目录。
std::wstring ModuleDirectory();

std::string WideToUtf8(const std::wstring& input);
std::wstring Utf8ToWide(const std::string& input);

}  // namespace tdx_czsc

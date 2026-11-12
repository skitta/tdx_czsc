#include "bridge_text.h"

#include <Windows.h>

namespace tdx_czsc {

std::wstring ModuleDirectory() {
    HMODULE module = nullptr;
    GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                           GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                       reinterpret_cast<LPCWSTR>(&ModuleDirectory), &module);
    if (module == nullptr) {
        return {};
    }

    wchar_t buffer[MAX_PATH]{};
    const DWORD copied = GetModuleFileNameW(module, buffer, MAX_PATH);
    if (copied == 0 || copied >= MAX_PATH) {
        return {};
    }

    std::wstring path(buffer, copied);
    const auto pos = path.find_last_of(L"\\/");
    if (pos != std::wstring::npos) {
        path.resize(pos);
    }
    return path;
}

std::string WideToUtf8(const std::wstring& input) {
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
}

std::wstring Utf8ToWide(const std::string& input) {
    if (input.empty()) {
        return {};
    }

    const int size_needed =
        MultiByteToWideChar(CP_UTF8, 0, input.c_str(), static_cast<int>(input.size()), nullptr, 0);
    if (size_needed <= 0) {
        return {};
    }

    std::wstring result(static_cast<std::size_t>(size_needed), L'\0');
    MultiByteToWideChar(CP_UTF8, 0, input.c_str(), static_cast<int>(input.size()),
                        result.data(), size_needed);
    return result;
}

}  // namespace tdx_czsc

// deploy_host_smoke —— 模拟通达信宿主：从"与自身不同目录"加载 tdx_czsc_bridge.dll
// 并调用 func 902（服务存活通道），验证：
//   1. 服务自动拉起的路径解析基于 DLL 所在目录（ModuleDirectory 取 DLL 自身）；
//   2. 服务进程启动、902 返回 1（服务在线）；
//   3. 宿主退出后服务被 KILL_ON_JOB_CLOSE 终止。
//
// 用法（手动，真实布局模拟）：
//   host\deploy_host_smoke.exe            （宿主 exe 放在 host\）
//   dlls\tdx_czsc_bridge.dll 等四件套      （DLL + 依赖 + 服务 exe 放 dlls\）
//   运行前把 dlls\ 加入 PATH（依赖解析）或把依赖也放 host\ 旁。
#include <windows.h>

#include <cstdio>

#include "PluginTCalcFunc.h"

int main() {
    HMODULE dll = LoadLibraryA("tdx_czsc_bridge.dll");
    if (dll == nullptr) {
        std::printf("LoadLibrary(tdx_czsc_bridge.dll) failed error=%lu\n", GetLastError());
        return 1;
    }

    auto reg = reinterpret_cast<pRegisterPluginFUNC>(GetProcAddress(dll, "RegisterTdxFunc"));
    if (reg == nullptr) {
        std::printf("GetProcAddress(RegisterTdxFunc) failed\n");
        return 1;
    }

    PluginTCalcFuncInfo* infos = nullptr;
    // 成功返回 1（*pFun 为 null 时设置并返回 1；非 null 忽略并返回 0）
    if (reg(&infos) != 1 || infos == nullptr) {
        std::printf("RegisterTdxFunc failed\n");
        return 1;
    }

    for (PluginTCalcFuncInfo* p = infos; p->nFuncMark != 0; ++p) {
        if (p->nFuncMark != 902) {
            continue;
        }
        constexpr int kLen = 64;
        float high[kLen]{}, low[kLen]{}, close[kLen]{}, out[kLen]{};
        for (int i = 0; i < kLen; ++i) {
            const float v = 10.0f + static_cast<float>(i % 9) * 0.3f;
            high[i] = v + 0.2f;
            low[i] = v - 0.2f;
            close[i] = v;
        }
        p->pCallFunc(kLen, out, high, low, close);

        float sum = 0.0f;
        for (int i = 0; i < kLen; ++i) {
            sum += out[i];
        }
        std::printf("func902 sum=%.2f -> %s\n", sum, sum > 0.0f ? "SERVICE UP" : "SERVICE DOWN");
        return sum > 0.0f ? 0 : 2;
    }

    std::printf("func 902 not found in registry\n");
    return 1;
}

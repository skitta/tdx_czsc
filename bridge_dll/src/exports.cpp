#include "bridge_functions.h"
#include "bridge_logging.h"

extern "C" __declspec(dllexport) int RegisterTdxFunc(PluginTCalcFuncInfo** pFun) {
    // 异常不得穿透 DLL 边界：注册失败只返回 0，让通达信自行降级。
    try {
        if (pFun == nullptr) {
            tdx_czsc::LogError("RegisterTdxFunc received null PluginTCalcFuncInfo**");
            return 0;
        }

        if (*pFun == nullptr) {
            *pFun = tdx_czsc::GetFunctionRegistry();
            tdx_czsc::LogInfo("RegisterTdxFunc succeeded");
            return 1;
        }

        tdx_czsc::LogWarn("RegisterTdxFunc called with non-null registry pointer; ignored");
        return 0;
    } catch (...) {
        return 0;
    }
}

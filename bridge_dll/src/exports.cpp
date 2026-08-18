#include "bridge_functions.h"
#include "bridge_logging.h"

extern "C" __declspec(dllexport) int RegisterTdxFunc(PluginTCalcFuncInfo** pFun) {
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
}

#pragma once

#include <cstdint>

#include "PluginTCalcFunc.h"

namespace tdx_czsc {

void HandleFunc(uint32_t func_id, int data_len, float* out_values, float* in_high, float* in_low, float* in_close);
PluginTCalcFuncInfo* GetFunctionRegistry();

}  // namespace tdx_czsc

#pragma once

#include <cstdint>
#include <string>

#include "PluginTCalcFunc.h"

namespace tdx_czsc {

void HandleFunc(uint32_t func_id, int data_len, float* out_values, float* in_high, float* in_low, float* in_close);
PluginTCalcFuncInfo* GetFunctionRegistry();

#ifdef TDX_CZSC_TESTING
uint64_t ComputeInputHashForTest(int data_len, float* in_high, float* in_low, float* in_close);
uint64_t ComputeSegmentKeyForTest(int data_len, float* in_high, float* in_low);
std::string PeriodCodeToFreqForTest(int code);
std::string CachedFreqHintForTest(int data_len, float* in_high, float* in_low);
#endif

}  // namespace tdx_czsc

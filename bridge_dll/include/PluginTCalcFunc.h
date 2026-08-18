#pragma once

#pragma pack(push, 1)

// 通达信 DLL 公式回调签名：
// (数据个数, 输出, 输入a, 输入b, 输入c)
typedef void (*pPluginFUNC)(int, float*, float*, float*, float*);

typedef struct tagPluginTCalcFuncInfo {
    unsigned short nFuncMark;
    pPluginFUNC pCallFunc;
} PluginTCalcFuncInfo;

typedef int (*pRegisterPluginFUNC)(PluginTCalcFuncInfo**);

#pragma pack(pop)

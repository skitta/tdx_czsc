#pragma once

#include <cstdint>
#include <string>

namespace tdx_czsc {

/// DLL 运行配置。
///
/// 配置优先级（高到低）：
/// 1. 环境变量 `TDX_CZSC_*`
/// 2. 配置文件 `tdx_czsc_bridge.ini`（与 DLL 同目录，`key = value` 格式，`#`/`;` 注释）
/// 3. 内置默认值（对齐 docs/技术方案 11.4 与 14.1 的建议值）
struct BridgeConfig {
    std::wstring pipe_name = LR"(\\.\pipe\tdx_czsc_analyzer)";

    /// 连接等待超时（WaitNamedPipeW），首次/启动期允许更长。
    uint32_t startup_timeout_ms = 500;
    /// 单次请求读写超时。首刷（无缓存）在 Release 服务下实测：
    /// 1000 根窗口约 70~100ms、3933 根约 240ms（signal 演化占大头）；
    /// 通达信多窗口/多公式同时首刷会并发排队（服务端已做同数据段合并，
    /// 并发上限 2），因此保留 3s 余量；异常场景最多卡顿 3s（优于图上空白）。
    /// 缓存命中/去重等待后远低于此值。
    uint32_t request_timeout_ms = 3000;
    /// 连接失败后的额外重试次数（不含首次尝试）。
    uint32_t connect_retry_count = 2;
    /// 连接重试间隔。
    uint32_t connect_retry_interval_ms = 20;

    /// Rust 服务可执行文件路径。管道不存在（服务未运行）时 DLL 自动拉起它；
    /// 默认取 DLL 同目录的 tdx_czsc_service.exe，可用 INI/环境变量覆盖。
    std::wstring service_exe;
    /// 拉起服务后等待其就绪（管道出现）的最长时间。
    uint32_t service_startup_wait_ms = 8000;

    /// czsc 笔参数，随请求发送给服务端。
    uint32_t max_bi_num = 50;
    uint32_t min_bi_len = 6;
    uint32_t analysis_window = 1000;

    /// DLL 本地快照缓存容量。通达信组合图（日线主图 + 周线/分时附图等）
    /// 每个数据段各占一条，默认 16 覆盖常见多窗口组合；容量不足时旧段被
    /// 挤出会触发服务端重新分析。
    std::size_t dll_cache_capacity = 16;
};

/// 读取并合并三层配置，返回最终生效配置。
BridgeConfig LoadBridgeConfig();

}  // namespace tdx_czsc

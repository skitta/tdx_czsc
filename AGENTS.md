# AGENTS.md

## Project

tdx_czsc — 原型/联调阶段：Windows 通达信 x64 + Rust `czsc-core` 分析服务 + x64 C++ Bridge DLL（Named Pipe IPC）的缠论自动标注系统。先读 `README.md` 与 `CONTRIBUTING.md`。

## Structure / boundaries

- `proto/analysis.proto` — 协议唯一事实来源；Rust 侧由 `rust_service/build.rs` 的 prost-build 生成（需要 `protoc` 在 PATH 或 `PROTOC` 环境变量）。协议修改与实现修改必须配套提交。
- `rust_service/` — Rust 服务：构造 RawBar、调用 czsc-core、输出 AnalysisSnapshot 通道；模块：`analyzer`、`handlers`、`ipc`、`cache`、`proto.rs`、`state.rs`、`config.rs`。
- `bridge_dll/` — 通达信加载的 x64 C++ DLL：导出 `RegisterTdxFunc`，按 func_id 路由到输出通道，维护 DLL 本地缓存与日志。禁止异常穿透 DLL 边界；所有失败路径必须安全降级为 float 输出。
- `tdx_formula_examples/` — `.tn6` 公式模板；公式层只做显示，不承担缠论算法逻辑。
- `docs/` — 修改 IPC/DLL 相关代码前，先读 `技术方案`。

## Commands（Windows x64）

- Rust 检查/运行：`cargo check --manifest-path rust_service/Cargo.toml`；`cd rust_service && cargo run`
- DLL 构建 + 冒烟测试：`cd bridge_dll && cmake -S . -B build -A x64 && cmake --build build --config Release`；先跑 `build/Release/tdx_czsc_ipc_smoke_test.exe`，通过后再把 `tdx_czsc_bridge.dll` 放入通达信 `T0002/dlls/`
- DLL 日志位置：`bridge_dll/logs/tdx_czsc_bridge.log`

## Conventions / gotchas

- 通达信 DLL ABI 硬限制：3 路输入 `float[]`（HIGH、LOW、CLOSE）+ 1 路输出 `float[]`；复杂缠论对象必须拆成多通道输出（func_id 101–303 为结构通道，901–903 为调试通道；401–406 买卖点尚未实现）。
- DLL 编译与全链路验证只能在真实 Windows x64 + 通达信环境进行；协议与 Rust 侧工作可跨平台。
- 提交保持主题化（`rust_service: ...`、`bridge_dll: ...`、`docs: ...`）；当前阶段避免大范围重命名和在 DLL 侧引入大量新依赖。
- 本仓库注释与文档使用中文，保持一致。

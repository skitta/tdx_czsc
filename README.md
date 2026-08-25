# tdx_czsc

> 通达信 x64 + Rust `czsc-core` + x64 C++ Bridge DLL 的本地缠论自动标注系统原型

`tdx_czsc` 的目标不是重写缠论算法，而是把成熟的 Rust `czsc` 能力接入 **通达信 x64**，形成一套：

- 本地运行
- 可调试
- 可扩展
- 面向通达信公式系统的缠论自动标注方案

当前技术栈：

- **算法核心**：[`waditu/czsc`](https://github.com/waditu/czsc) / `czsc-core`
- **分析服务**：Rust
- **桥接层**：x64 C++ DLL
- **IPC**：Windows Named Pipe
- **展示层**：通达信公式 + `TDXDLLn(...)`

---

## 特性概览

当前仓库已实现/具备：

- `analysis.proto` 协议定义
- Rust 本地分析服务骨架
- `czsc-core` 真实接入
- `czsc-signals` 买卖点信号接入（401~406 一买一卖 / 二类 / 三类）
- 基础结构通道输出
- x64 Bridge DLL 工程骨架
- C++ protobuf + Named Pipe 客户端骨架（OVERLAPPED 超时 + 连接重试）
- DLL 本地缓存、日志与可配置策略（环境变量 / `tdx_czsc_bridge.ini`）
- Windows 控制台 `ipc_smoke_test`
- 通达信公式示例模板
- Windows 构建、联调、排障、checklist 文档

当前已完成的部分包括：

- Windows x64 真机全链路联调（Rust 服务 / bridge_dll / ipc_smoke_test / 通达信公式显示）—— **2026-08-17 首轮联调通过**
- 401~406 买卖点信号通道（接入 `czsc-signals`，一买/一卖为标准信号，二/三类为均线辅助判定；2026-08-17 实现）

当前还未完成的部分主要包括：

- 安装与分发方案（当前 DLL 依赖 `libprotobuf.dll` / `abseil_dll.dll` 两个运行时）

> **项目状态：原型 / 首轮联调已通过**

---

## 整体架构

```mermaid
flowchart TD
    A[通达信 x64] --> B[tdx_czsc_bridge.dll]
    B --> C[Rust Local Service]
    C --> D[czsc-core]
    C --> E[Service Snapshot Cache]
    B --> F[DLL Local Cache]
```

### 各层职责

#### 通达信公式层
- 调用 `TDXDLLn(func_id, HIGH, LOW, CLOSE)`
- 负责图形显示
- 不承担缠论算法计算

#### x64 Bridge DLL
- 导出 `RegisterTdxFunc`
- 接收 3 路 `float[]` 输入
- 将不同 `func_id` 路由为不同输出通道
- 维护 DLL 本地缓存
- 通过 Named Pipe 请求 Rust 服务
- 将结果回填为通达信可识别的 `float[]`

#### Rust Service
- 接收结构化请求
- 构造 `RawBar`
- 调用 `czsc-core`
- 输出 `AnalysisSnapshot`
- 管理服务级缓存与调试信息

---

## 当前已实现的通道

| func_id | 通道名 | 说明 |
|---:|---|---|
| 101 | `fx_top` | 顶分型 |
| 102 | `fx_bottom` | 底分型 |
| 201 | `bi_points` | 笔端点 |
| 202 | `bi_direction` | 笔方向 |
| 203 | `seg_points` | 段端点（段首笔起点/末笔终点分型价格） |
| 204 | `seg_direction` | 段方向（段覆盖区间 ±1） |
| 301 | `zs_upper` | 中枢上沿 |
| 302 | `zs_lower` | 中枢下沿 |
| 303 | `zs_middle` | 中枢中轴 |
| 401 | `buy1` | 一买（czsc-signals `cxt_first_buy`） |
| 402 | `sell1` | 一卖（czsc-signals `cxt_first_sell`） |
| 403 | `buy2` | 二买（czsc-signals `cxt_second_bs` 均线辅助） |
| 404 | `sell2` | 二卖（czsc-signals `cxt_second_bs` 均线辅助） |
| 405 | `buy3` | 三买（czsc-signals `cxt_third_bs` 均线辅助） |
| 406 | `sell3` | 三卖（czsc-signals `cxt_third_bs` 均线辅助） |
| 901 | `debug_cache_hit` | 缓存命中调试 |
| 902 | `debug_service_ready` | 服务存活调试 |
| 903 | `debug_latency_ms` | 最近一次耗时调试 |
| 950 | `vol_input` | **量能输入通道**（不发起分析） |

### 当前输入约定

通达信公式当前按以下方式传参：

```text
TDXDLL1(func_id, HIGH, LOW, CLOSE)
```

即：

- 第 1 路输入：`HIGH`
- 第 2 路输入：`LOW`
- 第 3 路输入：`CLOSE`

### 量能输入（func_id=950）与一买/一卖量能背驰

通达信 DLL ABI 只有 3 路 `float[]` 输入，无法同时传 CLOSE 与 VOL；为此公式需**额外调用一次量能通道**：

```text
TDXDLL1(950, HIGH, LOW, VOL)
```

- DLL 以「数据段指纹」（`data_len` + 前两路 HIGH/LOW 哈希，与第三路无关）缓存该 VOL 序列，
  并在后续 101/401 等分析请求构造时自动融合进 `MarketSeries.vol`；
- 融合后请求的 `input_hash` 会纳入量能，DLL 与服务端缓存自动区分「带量/不带量」结果；
- 公式模板中 950 调用行应放在 401~406 之前（通达信按公式文本顺序求值；顺序颠倒时
  DLL 进程级缓存会在下一轮刷新后命中，同样生效）。

**量能来源链（服务端 `build_raw_bars`）**：

1. 请求携带的真实 `vol`（950 通道融合 / CSV 工具链）——原样使用；
2. vol 缺失且 `derive_volume_proxy=true`（默认）——以单根相对振幅 `(high-low)/close×100`
   派生代理量，维持「价格背驰 AND（量能 OR 长度）背驰」的完整判定；
3. 其余（旧公式且关闭代理）——vol=0，量能背驰恒不成立（与早期版本一致）。

---

## 仓库结构

```text
tdx_czsc/
├── README.md
├── CONTRIBUTING.md
├── .gitignore
├── proto/
│   └── analysis.proto
├── rust_service/
│   ├── Cargo.toml
│   ├── build.rs
│   └── src/
├── bridge_dll/
│   ├── CMakeLists.txt
│   ├── include/
│   ├── src/
│   ├── tests/
│   └── logs/
├── deploy_to_tdx.bat        ← 一键部署：构建服务与 DLL 并拷贝到通达信
├── tdx_formula_examples/
│   ├── debug_ready.tn6
│   ├── debug_latency.tn6
│   ├── cache_hit_debug.tn6
│   ├── fx_marks.tn6
│   ├── bi_points.tn6
│   ├── zs_band.tn6
│   └── main_chart_combo.tn6
└── docs/
    ├── index.md
    ├── 技术方案_通达信缠论自动标注系统.md
    ├── 开发环境说明_Mac与Windows.md
    ├── Windows构建前置条件.md
    ├── Windows首轮联调步骤.md
    ├── Windows实际联调排错清单.md
    ├── Windows联调执行Checklist.md
    ├── 通达信公式联调说明.md
    └── 开源许可证选择建议.md
```

---

## 快速开始

### 1. 先看文档
如果你是第一次进入仓库，建议阅读顺序：

1. `docs/index.md`
2. `docs/技术方案_通达信缠论自动标注系统.md`
3. `docs/开发环境说明_Mac与Windows.md`
4. `docs/Windows构建前置条件.md`

### 2. Rust 服务
目录：

- `rust_service/`

当前 `build.rs` 使用 `prost-build` 编译 `proto/analysis.proto`，因此需要本机可用的 `protoc`。

#### macOS
```bash
brew install protobuf
cargo check --manifest-path rust_service/Cargo.toml
```

如果 `protoc` 不在 PATH，可显式指定：

```bash
PROTOC=/path/to/protoc cargo check --manifest-path rust_service/Cargo.toml
```

### 3. Windows x64 首轮联调
建议顺序：

#### 启动 Rust 服务
```powershell
cd rust_service
cargo run
```

#### 构建 Bridge DLL 与测试程序
```powershell
cd bridge_dll
cmake -S . -B build -A x64
cmake --build build --config Release
```

#### 先跑控制台 IPC 测试
```powershell
cd build\Release
./tdx_czsc_ipc_smoke_test.exe
```

> 联调期手动启动服务后跑 smoke test；**发布部署后不需要手动启动**——smoke test 与通达信都会在检测到服务未运行时自动拉起 `tdx_czsc_service.exe`（见下）。

#### 再进入通达信
确认 `ipc_smoke_test` 跑通后，运行**根目录一键部署脚本**（自动构建服务与 DLL 并拷贝到通达信）：

```powershell
.\deploy_to_tdx.bat          # 默认 D:\new_tdx64；也可指定: .\deploy_to_tdx.bat D:\new_tdx64
```

脚本做三件事：① `cargo build --release` 构建 Rust 服务；② `cmake` 构建 bridge DLL；③ 拷贝 `tdx_czsc_bridge.dll`（含 `libprotobuf.dll`、`abseil_dll.dll` 两个运行时依赖）和 `tdx_czsc_service.exe` 到 `<通达信安装目录>/T0002/dlls/`。

**服务生命周期（自动）**：DLL 检测到服务未运行（管道不存在）时自动拉起同目录的 `tdx_czsc_service.exe`（日志写 `tdx_czsc_service.log`）；通达信退出（含崩溃）时服务由 Job Object（`KILL_ON_JOB_CLOSE`）自动终止——无需手动启停。服务 exe 路径可用 `tdx_czsc_bridge.ini`（样例见 `bridge_dll/tdx_czsc_bridge.ini.example`）或环境变量 `TDX_CZSC_SERVICE_EXE` 覆盖。

最后在通达信中：

---

## 通达信公式模板

目录：

- `tdx_formula_examples/`

建议联调顺序：

1. `debug_ready.tn6`
2. `debug_latency.tn6`
3. `cache_hit_debug.tn6`
4. `fx_marks.tn6`
5. `bi_points.tn6`
6. `zs_band.tn6`
7. `buy_sell_points.tn6`
8. `main_chart_combo.tn6`（精简综合版）
9. `main_chart_full.tn6`（完整综合版）

详细说明见：

- `docs/通达信公式联调说明.md`

---

## 开发环境说明

### Mac 上适合做什么
当前可以在 macOS 上优先推进：

- 协议设计
- Rust 服务开发
- `czsc-core` 接入
- 通道映射
- 文档整理
- C++ Bridge DLL 骨架编写

### Windows 上必须做什么
以下内容必须在 Windows x64 环境真实验证：

- Bridge DLL 编译
- Protobuf C++ 生成与链接
- Win32 Named Pipe 联调
- 通达信 x64 加载 DLL
- 通达信公式显示验证

详细见：

- `docs/开发环境说明_Mac与Windows.md`

---

## 文档导航

### 总体方案
- `docs/技术方案_通达信缠论自动标注系统.md`

### 环境与构建
- `docs/开发环境说明_Mac与Windows.md`
- `docs/Windows构建前置条件.md`

### 联调步骤
- `docs/Windows首轮联调步骤.md`
- `docs/Windows联调执行Checklist.md`

### 排障
- `docs/Windows实际联调排错清单.md`

### 公式联调
- `docs/通达信公式联调说明.md`

### 贡献与仓库维护
- `CONTRIBUTING.md`
- `docs/开源许可证选择建议.md`

---

## 性能参数与调优

**Release 实测**（`analyze_csv`，单请求）：

| 窗口 | 全量分析耗时 |
|---|---|
| 1000 根日线 | 约 70~100ms |
| 3933 根日线 | 约 240ms |
| 420 根 30 分钟 | 约 23ms |

主要耗时在买卖点信号演化（约 40%）与通道填充/编码（约 50%）。

**链路吞吐设计**：通达信每轮刷新对同一数据段有约 19 次 `TDXDLLn` 调用，仅首个未命中缓存的请求触发一次 IPC 全量分析，其余全部命中 DLL 本地快照缓存（默认 16 条，覆盖常见多窗口组合图）；服务端另有一级快照缓存（64 条）与**同数据段并发合并**。

**关键参数**（DLL 侧，优先级：环境变量 `TDX_CZSC_*` > `tdx_czsc_bridge.ini` > 内置默认）：

- `TDX_CZSC_REQUEST_TIMEOUT_MS`（默认 3000）：单次请求读写超时。首刷全量分析 100~240ms，多窗口并发有 2 路分析合并/排队，保留 3s 余量；异常场景最多卡顿 3s（优于图上空白）。
- `TDX_CZSC_CACHE_CAPACITY`（默认 16）：DLL 本地快照缓存条数，多周期组合图偏多时调大。
- `TDX_CZSC_MAX_BI_NUM` / `TDX_CZSC_MIN_BI_LEN` / `TDX_CZSC_ANALYSIS_WINDOW`：分析参数。

**服务端**：全量分析在独立阻塞线程池执行（`spawn_blocking`），不占用 IPC 协程；同数据段并发首刷合并为一次分析（singleflight）；全量分析并发上限 2（防多窗口同时首刷打满 CPU 导致全部超时）。

---

## 当前已知限制

1. 通达信 DLL ABI 天然限制为：
   - 3 路输入 `float[]`
   - 1 路输出 `float[]`
2. 复杂缠论对象必须降维为多个通道输出
3. 最终图形表达能力仍受通达信公式系统限制
4. 量能：公式需按上文「量能输入（func_id=950）」挂载 VOL 通道，一买/一卖才使用真实量能；
   未挂载时服务端自动以振幅代理量兜底（`AppConfig::derive_volume_proxy` 可关闭）
5. 当前仓库处于原型验证阶段；2026-08-17 已通过 Windows x64 真机首轮全链路联调（见 `docs/Windows实际联调排错清单.md` 的联调记录）

---

## 开源许可证说明

本项目当前采用：

- **MIT License**

许可证文件见：

- `LICENSE`

如果你想了解为什么当前选择 MIT，以及其他许可证（如 Apache-2.0 / GPL-3.0）的取舍，可参考：

- `docs/开源许可证选择建议.md`

---

## 贡献

欢迎围绕以下方向贡献：

- Rust 服务完善
- Windows x64 Bridge DLL 完善
- 通达信联调验证
- 公式模板优化
- 文档和排障资料补充

请先阅读：

- `CONTRIBUTING.md`

---

## 免责声明

本项目当前用于技术研究、工程验证与本地集成实验，不构成任何投资建议。

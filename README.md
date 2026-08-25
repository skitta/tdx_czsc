<div align="center">

# tdx_czsc

**通达信 x64 缠论自动标注系统** · *Chan-theory auto-labeling for TongDaXin (TDX)*

让通达信原生支持缠论：分型、笔、中枢、段、买卖点自动计算并在主图标注。

基于 **Rust `czsc-core` 分析服务** · **x64 C++ Bridge DLL** · **Windows Named Pipe IPC** · **通达信公式层显示**

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows%20x64-0078D6)](#快速开始-quick-start)
[![Lang](https://img.shields.io/badge/Lang-Rust%20%2F%20C%2B%2B-orange)](#仓库结构-repository-layout)
[![Status](https://img.shields.io/badge/Status-Prototype%20%E2%80%94%20%E8%81%94%E8%B0%83%E9%80%9A%E8%BF%87-green)](#项目状态--roadmap)

</div>

---

## 目录

- [项目简介](#项目简介)
- [特性 Features](#特性-features)
- [架构 Architecture](#架构-architecture)
- [快速开始 Quick Start](#快速开始-quick-start)
- [通道说明 Channels](#通道说明-channels)
- [仓库结构 Repository Layout](#仓库结构-repository-layout)
- [测试 Testing](#测试-testing)
- [性能 Performance](#性能-performance)
- [项目状态 & Roadmap](#项目状态--roadmap)
- [文档 Documentation](#文档-documentation)
- [参与贡献 Contributing](#参与贡献-contributing)
- [许可证 License](#许可证-license)
- [免责声明 Disclaimer](#免责声明-disclaimer)

---

## 项目简介

`tdx_czsc` 的目标不是重写缠论算法，而是把成熟的 Rust `czsc` 生态接入 **通达信 x64**，形成一套**本地运行、可调试、可扩展**的缠论自动标注方案：

```mermaid
flowchart LR
    A[通达信 x64 公式层] -->|TDXDLLn func_id| B[tdx_czsc_bridge.dll]
    B -->|Named Pipe| C[Rust Local Service]
    C --> D[czsc-core / czsc-signals]
    C --> E[服务端快照缓存]
    B --> F[DLL 本地缓存]
    A -.->|显示| G[主图标注：分型/笔/中枢/段/买卖点]
```

**设计原则**

- **算法在服务端**：缠论计算全部由 Rust 服务承担，通达信公式层只做显示；
- **协议先行**：`proto/analysis.proto` 是通道/消息的唯一事实来源，DLL 与服务的接口以它为准；
- **结果稳定可回归**：真实行情 CSV 驱动的回归测试覆盖通道值与绘制规则。

---

## 特性 Features

- ✅ **分型 / 笔**：顶底分型、笔端点与笔方向通道（101/102/201/202）
- ✅ **段（线段）**：特征序列分型法识别段（203/204）
- ✅ **中枢**：原生 `czsc-core` 贪心归属 + 前端/后端优化（301~304）
  - 前端：中枢第一笔与行情趋势相反（上升行情首笔下、下跌行情首笔上），行情方向按中枢高低位置判定；
  - 后端：中枢最后一笔与趋势相反（奇数笔结构），末笔与趋势同向的离开笔剔除；
  - 相邻间隔：前一个中枢后端与后一中枢前端至少间隔一笔，**前端优化优先于后端**；
- ✅ **买卖点**：接入 `czsc-signals`（401~406，一买/一卖标准信号 + 二/三类均线辅助），
  含锚点对齐、信号去重与三买/三卖按中枢严格化
- ✅ **量能背驰**：950 量能输入通道融合真实 VOL；未挂载时自动以振幅代理量兜底
- ✅ **Windows 全链路**：Named Pipe 服务 + 自动拉起/退出回收（Job Object）+ 冒烟测试
- ✅ **公式模板**：综合主图模板 `tdx_formula_examples/main_chart_full.tn6`

---

## 架构 Architecture

### 各层职责

| 层 | 职责 |
|---|---|
| **通达信公式层** | 调用 `TDXDLLn(func_id, HIGH, LOW, CLOSE)`，只负责图形显示，不承担算法计算 |
| **Bridge DLL（x64 C++）** | 导出 `RegisterTdxFunc`；按 `func_id` 路由到输出通道；维护 DLL 本地缓存（默认 16 条）；经 Named Pipe 请求 Rust 服务；服务未运行时自动拉起 `tdx_czsc_service.exe` |
| **Rust Service** | 接收结构化请求 → 构造 `RawBar` → 调用 `czsc-core`/`czsc-signals` → 输出 `AnalysisSnapshot`；管理服务级快照缓存（64 条）与同数据段并发合并（singleflight） |

### 输入约定

通达信公式当前按以下方式传参：

```text
TDXDLL1(func_id, HIGH, LOW, CLOSE)
```

- 第 1 路输入：`HIGH`
- 第 2 路输入：`LOW`
- 第 3 路输入：`CLOSE`

> 通达信 DLL ABI 只有 3 路 `float[]` 输入 + 1 路 `float[]` 输出，复杂缠论对象因此降维为**多通道输出**（见下方通道表）。

---

## 快速开始 Quick Start

### 前置要求

| 组件 | 版本要求 |
|---|---|
| 操作系统 | **Windows x64**（DLL 集成）；Rust 服务部分可跨平台开发 |
| Rust | stable（`rust_service/Cargo.toml`，edition 2024） |
| `protoc` | 需在 PATH（或设 `PROTOC` 环境变量，`build.rs` 生成协议） |
| CMake + MSVC | x64 工具链（构建 Bridge DLL） |
| 通达信 | x64 版本（验证显示） |

### 1. 构建与运行 Rust 服务

```powershell
cd rust_service
cargo run            # 启动 Named Pipe 服务（默认管道 \\.\pipe\tdx_czsc_analyzer）
```

macOS/Linux 下同样可 `cargo check` / `cargo test`（服务端逻辑），仅 DLL 联调必须 Windows。

### 2. 构建 Bridge DLL 并跑冒烟测试

```powershell
cd bridge_dll
cmake -S . -B build -A x64
cmake --build build --config Release
build\Release\tdx_czsc_ipc_smoke_test.exe   # 先跑控制台 IPC 测试
```

### 3. 部署到通达信

```powershell
.\deploy_to_tdx.bat            # 默认部署到 D:\new_tdx64；可传参：.\deploy_to_tdx.bat D:\new_tdx64
```

脚本自动完成：① `cargo build --release`；② cmake 构建 DLL；③ 拷贝
`tdx_czsc_bridge.dll`（含 `libprotobuf.dll`、`abseil_dll.dll` 运行时）与
`tdx_czsc_service.exe` 到 `<通达信安装目录>/T0002/dlls/`。

**服务生命周期（自动）**：DLL 检测到管道不存在时自动拉起同目录的
`tdx_czsc_service.exe`（日志 `tdx_czsc_service.log`）；通达信退出（含崩溃）时
由 Job Object（`KILL_ON_JOB_CLOSE`）自动终止服务——无需手动启停。服务 exe 路径
可用 `bridge_dll/tdx_czsc_bridge.ini.example` 或环境变量 `TDX_CZSC_SERVICE_EXE` 覆盖。

### 4. 通达信公式模板

在通达信中加载 `tdx_formula_examples/main_chart_full.tn6`（综合主图模板，含
分型/笔/中枢/买卖点标注），切换到带量能的周期数据即可看到标注结果。

> 量能提示：如需一买/一卖使用**真实量能**，公式需额外调用量能通道
> `TDXDLL1(950, HIGH, LOW, VOL)`（放在 401~406 之前）；未挂载时服务端自动
> 以振幅代理量兜底（可经配置关闭）。

---

## 通道说明 Channels

`func_id` 与通道的一一映射（单一来源：`rust_service/src/analyzer/channels.rs`，
协议定义见 `proto/analysis.proto`）：

| func_id | 通道名 | 说明 |
|---:|---|---|
| 101 | `fx_top` | 顶分型（分型极值K线处输出价格） |
| 102 | `fx_bottom` | 底分型 |
| 201 | `bi_points` | 笔端点（起止分型价格） |
| 202 | `bi_direction` | 笔方向（覆盖区间 ±1） |
| 203 | `seg_points` | 段端点（仅已确认段） |
| 204 | `seg_direction` | 段方向（段覆盖区间 ±1） |
| 301 | `zs_upper` | 中枢上沿 |
| 302 | `zs_lower` | 中枢下沿 |
| 303 | `zs_middle` | 中枢中轴 |
| 304 | `zs_boundary` | 中枢左右界标记（1=左界、2=右界、3=左右界共用） |
| 401~406 | `buy1`…`sell3` | 一买/一卖/二买/二卖/三买/三卖 |
| 901 | `debug_cache_hit` | 调试：缓存命中（1/0） |
| 902 | `debug_service_ready` | 调试：服务就绪（恒 1） |
| 903 | `debug_latency_ms` | 调试：最近一次请求耗时 |
| 950 | `vol_input` | **量能输入通道**（只上传 VOL，不发起分析） |

---

## 仓库结构 Repository Layout

```text
tdx_czsc/
├── proto/
│   └── analysis.proto          # 协议唯一事实来源（prost-build 生成 Rust 绑定）
├── rust_service/               # Rust 分析服务
│   ├── src/
│   │   ├── analyzer/           # 分型/笔/中枢/段/买卖点 + 通道映射（纯逻辑，无运行时依赖）
│   │   ├── handlers/           # 信封分发、分析协调（缓存/单飞）、调试通道
│   │   ├── cache/  state/  config/  ipc/   # 运行期支撑
│   │   └── main.rs
│   ├── examples/analyze_csv.rs # 离线 CSV 分析工具（脱离通达信验证算法）
│   └── tests/                  # 真实数据回归测试（通路/段/中枢/信号/背驰）
├── bridge_dll/                 # x64 C++ DLL（通达信加载）+ 冒烟测试 + ini 样例
├── tdx_formula_examples/       # 通达信公式模板
├── tests/data/                 # 回归测试真实行情 CSV
├── docs/                       # 技术方案 + 通达信 DLL 编程规范
├── deploy_to_tdx.bat           # 一键部署：构建服务与 DLL 并拷贝到通达信
├── CONTRIBUTING.md
└── LICENSE                     # MIT
```

---

## 测试 Testing

```powershell
cd rust_service
cargo test                       # 单元 + 集成回归（真实行情数据）
cargo test -- --nocapture        # 需要观察通道明细/性能剖面时
cargo clippy --all-targets -- -D warnings   # 质量门
cargo fmt -- --check
```

回归测试使用仓库内真实行情（`tests/data/*.csv`）分别覆盖：

- 通道值与绘制规则（中枢边界、首末笔方向、相邻间隔）
- 段识别规则（特征序列分型、连续性、方向校验）
- 买卖点锚点与去重、三买/三卖按中枢严格化
- 一买/一卖量能背驰（合成行情 + 代理量路径）

离线算法验证（不依赖通达信）：

```powershell
cd rust_service
cargo run --example analyze_csv -- ../tests/data/002340_day.csv 6
```

---

## 性能 Performance

**Release 实测**（`analyze_csv`，单请求全量分析）：

| 窗口 | 耗时 |
|---|---|
| 1000 根日线 | 约 70~100ms |
| 3933 根日线 | 约 240ms |
| 420 根 30 分钟 | 约 23ms |

**链路吞吐设计**：通达信每轮刷新对同一数据段约 19 次 `TDXDLLn` 调用，仅首个未命中
缓存的请求触发一次 IPC 全量分析，其余命中 DLL 本地快照缓存（默认 16 条）；服务端
另有快照缓存（64 条）与**同数据段并发合并**（singleflight）、全量分析并发上限 2。

**关键参数**（DLL 侧，优先级：环境变量 `TDX_CZSC_*` > `tdx_czsc_bridge.ini` > 内置默认）：

- `TDX_CZSC_REQUEST_TIMEOUT_MS`（默认 3000）：单次请求读写超时；
- `TDX_CZSC_CACHE_CAPACITY`（默认 16）：DLL 本地快照缓存条数；
- `TDX_CZSC_MAX_BI_NUM` / `TDX_CZSC_MIN_BI_LEN` / `TDX_CZSC_ANALYSIS_WINDOW`：分析参数。

---

## 项目状态 & Roadmap

**状态：原型 / 首轮联调阶段**

- 2026-08-17：Windows x64 真机全链路联调通过（Rust 服务 / bridge_dll / 冒烟测试 / 通达信公式显示）

**近期规划**

- [ ] 安装与分发方案（当前 DLL 依赖 `libprotobuf.dll` / `abseil_dll.dll` 两个运行时，目标静态链接或自带运行时）
- [ ] 多周期分析（跨周期中枢/背驰）
- [ ] 增量更新（避免整窗口重算）
- [ ] 图形表达增强（通达信公式层）

---

## 文档 Documentation

| 文档 | 内容 |
|---|---|
| [`docs/技术方案_通达信缠论自动标注系统.md`](docs/技术方案_通达信缠论自动标注系统.md) | 总体方案：协议、通道设计、中枢/段绘制规则、配置与验收 |
| [`docs/通达信DLL函数编程规范/`](docs/通达信DLL函数编程规范) | 通达信官方 DLL 编程规范与示例插件 |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | 贡献指南与 Rust 代码结构约定 |

---

## 参与贡献 Contributing

欢迎围绕以下方向贡献：

- Rust 服务完善（通道映射、算法规则、性能）
- Windows x64 Bridge DLL 完善
- 通达信联调验证与公式模板优化
- 文档与排障资料补充

请先阅读 [`CONTRIBUTING.md`](CONTRIBUTING.md)（含模块职责表、错误分层与抽象门槛约定）。

---

## 许可证 License

本项目采用 [MIT License](LICENSE)（Copyright © 2026 chainta）。

---

## 免责声明 Disclaimer

本项目用于技术研究、工程验证与本地集成实验，**不构成任何投资建议**。缠论标注结果
仅供参考，据此交易风险自负。
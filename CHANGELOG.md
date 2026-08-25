# Changelog

本项目采用"节点式手写"轻量维护方式；服务版本由 `rust_service/Cargo.toml`
（`CARGO_PKG_VERSION`）发布，通道与协议以 `proto/analysis.proto` 为准。

## [0.1.0] - 2026-08

**首个可用版本**（Windows x64 真机全链路联调通过：Rust 服务 / bridge DLL /
冒烟测试 / 通达信公式显示）。

### 新增

- 缠论结构：分型、笔、段（特征序列分型法）、中枢（原生 `czsc-core`
  `get_zs_seq` + 前端/后端优化）、买卖点（401~406，`czsc-signals`）；
- 中枢绘制规则：前端（首笔与趋势相反，方向按中枢高低位置判定）、后端
  （末笔与趋势相反，奇数笔结构）、相邻间隔（至少一笔，前端优先于后端）；
- 通道体系：101~204 结构通道、301~304 中枢通道（含 304 左右界标记）、
  901~903 调试通道、950 量能输入通道（真实 VOL 融合 + 振幅代理量兜底）；
- 运行时：Named Pipe IPC + protobuf、两级缓存（DLL 16 条 / 服务端 64 条）、
  singleflight 并发合并、服务自动拉起与 Job Object 退出回收；
- 工程化：错误类型分层（`AnalysisError`）与协议状态码映射、真实数据回归
  测试（4 个回归套件 + 背驰合成长行情）、离线 `analyze_csv` 工具、一键部署脚本
  `deploy_to_tdx.bat`、公式模板 `main_chart_full.tn6`。

### 代码结构演进

- `analyzer/` 按职责拆分：`rawbars`（请求→K线）、`pivots`（中枢）、
  `segs`（段）、`signals`（买卖点）、`channels`（通道）、`objects`
  （对齐与映射）、`error`（类型化错误）；
- `build_snapshot` 依赖倒置（`AnalysisParams`），分析管线为纯函数；
- 质量门：`clippy -D warnings` + `rustfmt` + `cargo test` 全绿。

### 已知限制

- DLL 依赖 `libprotobuf.dll` / `abseil_dll.dll` 运行时（分发方案见 Roadmap）；
- 静态快照分析，多周期/增量分析待实现。
# Changelog

本项目采用"节点式手写"轻量维护方式；服务版本由 `rust_service/Cargo.toml`
（`CARGO_PKG_VERSION`）发布，通道与协议以 `proto/analysis.proto` 为准。

## [0.2.0] - 2026-08

**公式层增强**（不涉及协议与 DLL 变更）。

### 新增

- 公式模板 `tdx_formula_examples/main_chart_full.tn6` 改为**结构版**（分型/
  笔/段/中枢，调试已注释），买卖点标注剥离为独立脚本
  `tdx_formula_examples/buy_sell_expert.tn6`——通达信**交易系统（专家
  系统）**公式：401~406 合成买入/卖出条件，输出 `ENTERLONG`（一买/二买/
  三买，K 线图标注 B）与 `EXITLONG`（一卖/二卖/三卖，标注 S），可回测；
  内置 950 量能通道调用。注意：专家系统公式不允许绘图函数（TDXSTEPxx
  输出名报错），类型文字/图标标注由配套技术指标公式
  `tdx_formula_examples/buy_sell_mark.tn6`（主图叠加）承担；
- 公式模板 `tdx_formula_examples/atr_tp_sl.tn6`：买点 ATR 动态止盈止损线
  （一买/二买/三买 401/403/405 **各自独立成段、互不合并**，每类各画一对
  动态线、可同时并存：止损线=吊灯 `HHV(高-K1×ATR, 买入以来)` 位于价格
  **下方**、下穿离场；止盈线=动态目标价 `HHV(前高+K2×ATR, 买入以来)`
  位于价格**上方**、上穿离场，随每次创新高跳升上移、只升不降；影线/收盘
  两种触发方式，买入 bar 自身不参与穿线判定；K 线首次穿线即终止该类线的
  绘画；只画线、无文字/图标标记；参数仅 ATR 周期/止损倍数/止盈倍数/
  触发方式 4 项；ATR 手写 TR 求算，跨通达信版本兼容）；
- 回归测试 `rust_service/tests/atr_tp_sl_regression.rs`：公式规则的影子
  实现，真实行情（002340 日线三类买点通道）验证"止损/止盈线段内只上移 /
  活段内止盈高于止损 / 首次穿线即终止且终止后段内已触发恒真"等语义
  不变量；合成平坦行情验证"三类独立成段、一买段跨越二买不被截断"。

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
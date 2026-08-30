# Changelog

本项目采用"节点式手写"轻量维护方式；服务版本由 `rust_service/Cargo.toml`
（`CARGO_PKG_VERSION`）发布，通道与协议以 `proto/analysis.proto` 为准。

## [0.5.0] - 2026-08

**全量演进（P0c）**——`analysis_window=0` 即全量（默认），`max_bi_num` 默认
1000；协议语义不变（0 = 全量），服务端与 DLL 默认值同步。修复：通达信
缩放到历史区域时前方 K 线无笔/段/中枢（此前尾部 1000 根窗口+笔数上限 50
双重截断）。

### 变更

- `analysis_window == 0`（默认）表示**全量**：分析通达信传入的全部 K 线，
  新增 `analyzer::window_start_of`（全量/限窗统一表达），`build_snapshot`
  与增量路径共用；
- `max_bi_num` 默认 50 → **1000**（对齐 czsc Python 默认）：全量历史
  笔数不再被截断（002340 全量 332 笔 vs 旧上限 50；买卖点自历史早期
  index=59 起完整保留）；`max_bi_num` 仍为 czsc 硬上限，超短线周期可
  调大 `TDX_CZSC_MAX_BI_NUM`；
- 全量窗口首部稳定 → P0b 增量承接（序列增长/盘中原地更新）在默认配置
  下直接生效，滚动/缩放零重算；
- DLL 默认值同步：`bridge_config.h` / `bridge_service_protocol.h` /
  `tdx_czsc_bridge.ini.example`（`analysis_window=0`、`max_bi_num=1000`）。

### 测试

- `window_start_zero_means_full_series` / `analysis_window_zero_snapshot_covers_all_bars`
  （单测：0 = 全量语义）；
- `full_history_window_zero_keeps_all_bis_and_signals`（集成：3933 根全量
  通道全覆盖、笔数 >50、401 信号存在）；
- `incremental_regression` 全量窗口用例改用 `analysis_window=0`。

### 实测

| 场景 | 耗时 |
|---|---|
| 3933 根日线全量（min_bi_len=6，332 笔） | 单遍 253ms / 总 523ms |
| 3933 根日线全量（min_bi_len=3，>1000 笔） | 单遍 401ms / 总 813ms |
| 增量承接（新增数百根） | 约 7~12ms |

### 真机验证

- 2026-08-30：Windows x64 + 通达信（D:\new_tdx64）真机验证通过——
  全历史区域笔/段/中枢连续标注（滚动/缩放无空白）、历史买卖点完整、
  滚动/刷新流畅（增量+快照缓存命中）。

### 文档

- `docs/技术方案` §6.2/§10/§11 更新默认值与实测口径；README 特性/参数/
  性能表/规划同步。

## [0.4.0] - 2026-08

**增量分析（enable_incremental）**——纯 `rust_service` 侧改动，不动协议与 DLL
（协议 `CzscOptions.enable_incremental` 字段预留，服务端默认配置开启）。

### 新增

- `analyzer/incremental.rs`：`IncrementalState`（已演化的 CZSC + 信号评估器
  `SignalEvaluator`）与 LRU `IncrementalCache`（默认 8 条）：
  - **序列增长**（全量窗口 `data_len <= analysis_window`）：只对新增 K 线
    `update_bar + observe`，稳态刷新 O(新增)。实测（002340）：新增 200~300
    根约 7~12ms、同序列缓存命中约 10µs；
  - **等长末笔原地更新**（盘中刷新）：前缀不变、末笔同 dt 值变化，走
    czsc "同 dt 延伸"分支；
  - 不适用自动回退全量（无状态/前缀不一致/窗口缩短/末笔 dt 变化/需要
    信号但评估器缺失/固定尾窗口平移），输出与全量路径**位级一致**；
- `analyzer/signals.rs`：演化与评估状态提取为可复用 `SignalEvaluator`
  （observe/finalize_points），`evolve_bars` 成为全量/增量共用的演化核心；
- `analyzer/mod.rs`：拆分 `assemble_snapshot`（全量/增量共用装配），
  `build_snapshot` 内部改用单遍演化核心；
- `handlers/analyze.rs`：增量优先编排（取出→锁外演化→放回，并发安全），
  全量回退同时建立增量状态；
- 配置：`enable_incremental`（默认 true）、`incremental_cache_capacity`
  （默认 8）。

### 测试

- `tests/incremental_regression.rs`：真实行情（002340 日线 3933 根）经
  `handle` 端到端验证：序列增长/末笔原地更新与独立全量**逐通道逐值一致**
  且不触发第二次全量；固定尾窗口平移回退全量但输出正确；
- `incremental.rs` 单元测试：逐块承接等价性、评估器跨结构-only 步骤保鲜、
  原地更新/拒绝路径、前缀不匹配状态不变、LRU 语义。

### 文档

- `docs/技术方案` §9.4 新增增量状态缓存设计；§11.2 更新稳态刷新吞吐；
  §14 已知限制与 §15 路线图同步（P0b 完成项）。

## [0.3.0] - 2026-08

**性能优化（P0 单遍演化）**——纯 `rust_service` 侧改动，不动协议与 DLL。

### 变更

- `analyzer/signals.rs`：`compute_buy_sell_points` 返回演化完成的 `CZSC`
  （终态与 `CZSC::new(bars)` 等价），`build_snapshot` 直接复用它作结构
  通道输入（分型/笔/中枢/段）——同一窗口只跑一遍逐 bar 演化，消除
  "信号演化 + 结构 `CZSC::new`" 的双重全量扫描；未请求 401~406 时仍走
  `CZSC::new` 快速路径（语义不变，回归测试锁定）；
- 明确不做"信号最终态一次性评估"：czsc-signals 信号函数只检查
  `bi_list` 尾部窗口（最近 5/21/19/... 笔），最终态评估会丢失全部历史
  买卖点（原因记录于 `signals.rs` 模块文档与 `docs/技术方案` §8.4）；
- `examples/analyze_csv.rs` 剖面输出同步为单遍模型（`czsc+signals(单遍)`）；
- 性能剖面单测 `profile_analysis_stages` 更新为新模型计时。

### 文档

- `docs/技术方案` §8.4 记录单遍演化与"不做最终态评估"的原因；
  §11.1 更新单遍化后的耗时分布说明。

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
# Changelog

本项目采用"节点式手写"轻量维护方式；服务版本由 `rust_service/Cargo.toml`
（`CARGO_PKG_VERSION`）发布，通道与协议以 `proto/analysis.proto` 为准。

## [0.10.0] - 2026-11

**段中枢**（以已确认段为构件，复用笔中枢规则）。

### 变更

- **中枢算法抽出构件视图**：新增 `PivotUnit` 与 `build_selection`，笔中枢
  `PivotAnalyzer` 与段中枢 `SegmentPivotAnalyzer` 共用原生贪心归属、
  前端/后端修剪与相邻间隔规则；`PivotAnalyzer::build` 签名与输出（`Vec<ZS>`）
  不变，三买三卖严格化与既有回归测试不受影响；
- **新增段中枢通道**：305 `seg_zs_upper`、306 `seg_zs_lower`、
  307 `seg_zs_boundary`（`ChannelKind`、`CHANNEL_SPECS`、DLL func_id 清单同步）；
- **协议新增对象**：`AnalysisSnapshot.segment_pivots`（`PivotZone`），
  `include_objects=true` 时返回，未确认段不参与；
- **主图公式**：段中枢洋红实线矩形 + `段中枢开关`（默认开），笔中枢改为
  蓝色虚线以便区分；
- **测试**：新增 `tests/segment_pivot_regression.rs`（30 分钟 3 个段中枢锚点、
  不重叠、仅用确认段、日线结构合法性）；
- 部署顺序：旧服务端 + 新 DLL 会因未知 func_id 导致整个快照失败，
  必须**先升级服务端、再更新 DLL 与公式**。

## [0.9.0] - 2026-08

**预留清理 + 配置加载**（按用户原则：非用户提出/同意的功能不再预留）。

### 变更

- **协议死字段移除**：`STATUS_UNSUPPORTED_VERSION/TIMEOUT/SERVICE_BUSY`
  三状态码（服务端无产生路径）、`HealthRequest.verbose`（无消费方）、
  `TimingStats.build_bars_ms/map_channels_ms`（恒 0）、proto 的
  java/go/csharp 生成 option（prost 不需要）、`requested_func_ids`
  空集默认分支（DLL 恒传全量）；
- **移除 `symbol_hint` 预留通道**：`AnalyzeRequest.symbol_hint` 与
  `SnapshotMeta.symbol` 从协议删除（DLL 恒发占位符、无多标的通路），
  Rust RawBar 符号恒定 `"TDX"`；测试辅助 `analyze_request(_ex)` 同步
  去掉 symbol 参数与全部调用点；
- **服务端配置加载**：`AppConfig` 支持 exe 同目录 `tdx_czsc_service.ini`
  （UTF-8，风格对齐 DLL 侧）与 `TDX_CZSC_*` 环境变量，INI 不存在静默
  回退默认值（此前服务端独有参数只能改代码重编译）；附
  `rust_service/tdx_czsc_service.ini.example`；
- 协议字段移除涉及 DLL 与服务端两端同步（构建时由
  `protobuf_generate_cpp` 重新生成），请随下次部署真机复验；
- 修正技术方案 §10.1 残留的已删键 `request_timeout_ms`。

## [0.8.0] - 2026-08

**P2：内存/编译微优化**（实测驱动决策）。

### 变更

- `[profile.release] strip="debuginfo"`（零编译代价，减小部署体积）；
- **thin LTO + codegen-units=1 实测后弃用**：全量快照 170→160ms（约 5%），
  release 构建 31s → 6m49s（10 倍）——本项目 release 构建伴随每次部署/
  验证迭代，负收益；决策与依据记录于 `Cargo.toml` 与 `docs/技术方案` §11.3；
- 内存侧评估（无改动）：RawBar 逐根克隆属 czsc-core 依赖内行为
  （外层省 1/3 改动面大、收益小）；增量状态双份 K 线为前缀校验设计必需。

## [0.7.0] - 2026-08

**代码清理（chore/cleanup）**——三档：死代码/冗余清理 + 协议字段移除 + 微优化。
冒烟测试管道环节受本会话沙箱限制，协议字段移除（第二档）请随下次部署真机复验。

### 第一档：死配置/死依赖/测试辅助收敛

- 删 `bytes` 死依赖（全仓无引用）与 `AppConfig.request_timeout_ms` 死配置
  （服务端无读取点，超时全在 DLL 侧与 IPC 层）；
- `SnapshotCache::is_empty` 保留（clippy `len_without_is_empty` 强制配对）；
- tests：`common` 新增 `load_raw_bars`（CSV→RawBar，全局 id/真实日期）与
  `analyze_request_ex`（max_bi_num 定制），`signal_regression` /
  `signals_fast_parity` / `incremental_regression` 收敛统一辅助，消除
  三份近同 CSV 解析与本地请求构造。

### 第二档：协议未使用字段移除（proto + Rust + DLL 两端同步）

- `AnalyzeRequest.bar_build_mode` / `caller_instance_id`（+ `BarBuildMode`
  enum）与 `CzscOptions.include_channels`：扫描确认无任何服务端读取，
  全部构造点/导入/设置点清理；prost 重新生成、DLL cmake Release 编译通过；
- `CzscOptions.enable_incremental` 保留（服务端配置默认开启，字段预留）。

### 第三档：微优化

- `signals_fast` need_init 判定的 `existing.ids.contains`（O(N)）→
  二分 `ids_contains`（O(log N)，id 序列单调递增，位级等价；对照测试
  全绿）；
- `analyze_csv` 双遍分析补充注释（区分"直连演化剖面"与"快照装配剖面"）。

## [0.6.0] - 2026-08

**P1：并行 + 异步优化**——纯 `rust_service` 侧改动，不动协议与 DLL。

### 性能剖面工具化（P1-1）

- `SnapshotProfile` + `build_snapshot_profiled` / `evolve_bars_profiled`
  （可选收集，热路径零开销）；`analyze_csv` 输出各阶段耗时与
  protobuf 编码耗时；
- **剖面结论（3933 根全量）**：信号评估 observe 227.5ms（87%）、
  czsc update_bar 22.9ms、pivots+segs 5ms、**channels 0.27ms / objects
  0.08ms / 编码 0.19ms**——旧"通道/编码约 50%"认知证伪，通道并行
  收益约 0.2ms 无意义，P1 转向信号评估热路径。

### 信号评估热路径（P1-2，主要收益）

- 新增 `analyzer/signals_fast.rs`：`cxt_second_bs_V230320` /
  `cxt_third_bs_V230318` 的 O(log N) 重实现——O(1) 增量追加判定 +
  尾部窗口整段重算（复刻 `update_ma_cache` 位级语义）、二分 id 定位
  （替代每次评估 O(N) 的 `bar_index_map`）、快照重算（复刻
  `ma_snapshot_value`）；信号字符串经官方构造器生成；
- `SIGNAL_SPECS` 分派：一买/一卖保持原实现（纯数学），403~406 走
  快速路径；`SignalEvaluator` 持有 `MaSeriesCache` 跨评估共享；
- 对照测试 `tests/signals_fast_parity.rs`：4 个长度递增状态连续喂入
  （覆盖 need_init/增量/修剪分支）+ 末笔原地修正（快照重算路径），
  新旧实现信号字符串逐字符一致；
- 实测：observe 227.5ms → **51.3ms**（4.4×）、evolve 250.7ms →
  74.6ms、快照总 523ms → **170ms**（3.1×）；信号输出与优化前逐条一致。

### 并发编排参数化（P1-4）

- 全量分析并发上限 `MAX_CONCURRENT_ANALYSES=2` 硬编码 →
  `AppConfig::analysis_parallelism`（默认 4），多窗口/多周期/多标的
  场景可调大；增量承接与 singleflight 等待不占槽位。

### 文档

- `docs/技术方案` §10.1/§11.1/§11.2 更新 P1 实测与剖面结论；
  README 性能表/吞吐设计同步；本条目。

### 真机验证

- 2026-08-30：Windows x64 + 通达信（D:\new_tdx64）真机验证通过——
  P1 性能回归（滚动/缩放零重算、多窗口并发首刷、尾盘增量刷新流畅）
  与既有功能回归全项通过。

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
- 公式模板 `tdx_formula_examples/atr_sl_day.tn6`：日线 ATR 动态止损线
  （一买/二买/三买独立成段、吊灯式下穿终止）；输出带 `PERIOD` 周期门控，
  **仅在日线周期显示，其他周期自动不显示**（日线编码按版本可调）；
- 公式模板 `tdx_formula_examples/chan_select.tn6`：缠论**条件选股公式**
  （选股器 30 分钟周期扫描，多周期联立口径：近 N 根内出现一买/二买/三买 +
  窗口无卖点 + 信号放量 + 日线向上确认 `CLOSE#DAY`；参数 `回看`/`放量系数`；
  真机验证项：选股批量执行时 DLL 通道与 #DAY 跨周期引用的可用性）；
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

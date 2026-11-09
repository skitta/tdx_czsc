# 严格派中枢（L2）实现计划

## 摘要

- 目标：在保留现有笔中枢 301–304 的前提下，新增一套“段中枢 → 9 段升级/扩张 → 更高级别”的严格派中枢管线。
- 分级：L1 = 段中枢 + 虚中枢 + `GG/DD` + 段级三买三卖；L2 = L1 + 9 段升级（扩展）+ 扩张 + 级别合并（本计划）；L3 = 真·多级别走势中枢、区间套、跨周期联立。
- 非目标：L3；本计划不做跨周期联立。
- 默认落地方式：新增并行通道 305–308、407–408 和独立公式模板；**现有 301–304 的通道语义与 `main_chart_full.tn6` 产物保持不变**（只新增，不改旧通道行为）。
- 交付顺序：先 `PLAN.md`，再 Rust 算法与测试（含 `analyze_csv --compare` 离线验收工具），再通道/DLL/公式，最后真机验证。
- 口径原则：本文件 §规则定义 是设计草案，**唯一规范文本是 `docs/严格中枢规则.md`**；代码注释只写“见 docs/严格中枢规则.md#小节”，不复制公式，避免三处漂移。

## 调研结论：市面画法 vs 本项目

| 方案 | 中枢构成单元 | 级别与升级 | 延伸/结束 | 三买三卖 | 主要特点 |
| --- | --- | --- | --- | --- | --- |
| 缠论原文/严格派 | 次级别走势，工程上常取段或走势段 | 级别由构成单元递归；9 段延伸可升级；两个同级别中枢波动区间重叠为扩张 | 后续次级别仍与 `[ZD,ZG]` 重叠则延伸；完全离开即结束 | 离开后次级别回抽不破 `ZG/ZD` | 规则完整，但原文表述有歧义、段确认滞后 |
| `czsc`（本项目依赖） | 笔 | 无级别、无段中枢、无扩张 | `get_zs_seq` 贪心合并；离开/连接笔常并入下一中枢 | 独立信号函数，后按中枢去重 | 轻量稳定，但笔中枢噪声大、窄区间多 |
| `chan.py`（Vespa314） | 笔/段/走势中枢可配置 | `CZS/CZSList`、`sub_zs`、3 个子中枢合并升级 | 有 `is_sure`、虚中枢、延伸/扩张 | 有段级三买三卖 | 规则较全，参数多、移植成本高 |
| `chanlun-pro` | 多级别笔/段/走势 | 递归级别、区间套 | 较完整 | 有 | 偏商业化，依赖重，不适合直接嵌入 |
| TradingView/通达信常见指标 | ZigZag 或笔 | 多数只做单级别，少量 9 段升级 | 简单延伸到反向突破 | 常见简化版 | 视觉直观，但规则不严、易有未来函数 |
| 本项目现状 | 笔（`czsc`）+ 首尾裁剪 | 无级别、无段中枢、无 9 段、无扩张 | 原生贪心 + 进入/离开笔裁剪 | 401–406，按中枢严格化 | 与严格派的主要差距是“构成单元”和“级别”；`GG/DD` 仅在 `czsc::ZS` 内部存在，未输出 |

**窄中枢的意义**：上下沿挨得近，本质是该级别重叠区窄、震荡幅度小，不是强弱信号本身。是否有意义看四点：级别（段中枢比笔中枢可靠）、位置（趋势中继还是末端）、持续时间与量能、突破后的回抽确认。窄而长的中枢常是筹码密集/蓄势，突破力度可能大，但也更容易被单根大 K 穿透、假突破多；且窄中枢的 `GG/DD` 可能远宽于 `ZG/ZD`，只看上下沿会低估真实波动。零宽度（`ZG==ZD`）是合法极端，不应过滤，但需要级别和确认状态区分“精确共识”与笔划分噪声。结论：严格派中枢值得做，价值在于用段/级别过滤笔中枢噪声并用虚中枢表达不确定性，而不是把图“画得更像”。

**可行性**：高。`BI`/`Segment` 输入已具备，严格中枢可作为纯函数新增；主要成本不在算法本身，而在规则口径、段确认滞后，以及 6 个新通道在 proto/服务/channels/DLL/公式五处的注册一致性。**注意**：现有实测（3933 根日线）`pivots+segs≈4.2ms`、`evolve≈70ms`、总计约 165ms，301–304 不是耗时大头，严格中枢只是一次 O(段数) 扫描，性能门槛按绝对值给（见 §测试与验收）。

## 规则定义

### 级别与对象

- 级别 1：**段中枢**，构成单元 = 段；未确认段（尾部未完成段，或窗口起点切出的首段）参与时标虚。
- 级别 2：**9 段升级中枢**（3 个 3 段组）或**扩张中枢**（2 个同级别中枢）。
- 级别 3：级别 2 中枢扩张后的更高级别；L2 到此封顶，不再递归。
- 新增内部类型 `StrictPivot`：
  - `level`、`kind`、`is_sure`、`start_index`、`end_index`、`upper`、`lower`、`middle`、`gg`、`dd`、`move_count`（`kind` 阶段 B 才引入；A 阶段常量视为 `Segment`）
  - `move_indices: Vec<usize>`：本中枢的构成单元索引；`kind==Segment` 时是段下标、`NineSegmentUpgrade` 时是段下标（分组由 level 算法解释）、`Expansion` 时是 **`pivots` 下标**（子中枢）。由 `kind` 决定解释域。
  - `parent: Option<usize>` / `children: Vec<usize>`：扩张/升级的父子关系（引用 `pivots` 下标），供调试与对象回放。
- `kind` 取值：`Segment`、`NineSegmentUpgrade`、`Expansion`（阶段 B 才引入，见 §实施阶段）。
- `move_count`：构成单元数（段中枢=段数、9 段升级=3 段组数、扩张=子中枢数）；派生自 `move_indices.len()`，不单独维护以免不一致。

### 段中枢

- 取连续 3 个段，且**每个段必须先满足“完成次级别走势”门槛**（见下），再判定 `min(段包络高) >= max(段包络低)`，形成级别 1 中枢。
  - **段包络**：段内全部构成笔的 `get_high()` 的最大值 / `get_low()` 的最小值，统一由 `seg_envelope(bis, seg)` 计算（`objects.rs` 与严格中枢共用同一函数，不得各写一套）。
  - **门槛（关键）**：只有“自身构成次级别走势”的段才参与三元组。判定：①该段内笔序列按现有笔中枢规则能形成至少 1 个笔中枢；或 ②该段笔数 ≥ `MIN_SEG_BI_LEN`（模块常量，默认 5）。不满足门槛的段视为“未完成的次级别走势”，不参与重叠判定，本轮跳过并后移扫描。
  - 说明：相邻段共享端点会让“3 段重叠”近乎恒真（只有深跌破位/弱反弹才不成立），加门槛后才真正具备“过滤笔中枢噪声”的意义，否则中枢数量≈段数/常数、与验收预期相反。该门槛是模块内常量，不进协议/配置，避免“无消费方配置项”。
- `ZG = min(段包络高)`，`ZD = max(段包络低)`；前三段确认后固定，后续延伸不改变。
- `GG = max(段包络高)`，`DD = min(段包络低)`，随延伸更新；`ZZ = (ZG + ZD) / 2`。
- 允许 `ZG == ZD`（零宽度中枢），不额外过滤；**所有重叠判定统一“`>=` 记相交”**，等号边界需有测试。
- **延伸与结束**：
  - 定义“有交集” = 段包络 `[low,high]` 与 `[ZD,ZG]` 有公共点（`high >= ZD && low <= ZG`）。
  - 下一段**有交集**（含跨过整个区间的突破段 `low < ZD && high > ZG`）→ 并入并更新 `GG/DD`。跨段视为延伸，不单独特殊处理；其终点若落在区间外，只是抬升/压低 `GG/DD`。
  - 下一段**完全在区间外**（`high < ZD || low > ZG`）→ 中枢在该段之前结束；该段是**离开段**、不属于中枢。下一轮从离开段**之后**开始扫描（离开段本身不参与新中枢，也不被两个中枢共享）。
  - 段归属唯一：每个段要么属于某一个中枢，要么作为某一中枢的离开段；不得被两个中枢重复占用。
  - 中枢结束前一直延伸到数据末尾：`end_index` 取最后一段终点；`is_sure` 只由构成段是否确认决定。
- 前 3 段（已过门槛）无重叠：跳过首段，继续向后找；不生成中枢。

### 9 段升级（中枢扩展）

- 级别 1 中枢的 `move_count >= 9` 时，取前 9 段按 3 段一组分 `G1/G2/G3`。
- **统一口径（初始组与延伸组一致）**：每个 3 段组必须“自身合法”（组内 `GZ = min(三段包络高) >= GD = max(三段包络低)`）且“与级别 2 当前区间相交（`>=` 记相交）”。级别 2 区间由 `G1` 的 `[GD,GZ]` 初始化，随 `G2/G3` 收缩为交叠 `ZG2 = min(GZ_i)`、`ZD2 = max(GD_i)`；收缩后 `ZG2 < ZD2` 则升级失败。后续每加一组同规则，不再另立“相交”口径。
- `GG/DD` 取 9 段的总 `max/min`；`move_count = 3 段组数`；`kind = NineSegmentUpgrade`。
- 超过 9 段时，每多 3 段成一组：该组自身合法且与级别 2 区间相交则并入、更新 `GG/DD`、`move_count + 1`，否则停止；剩余 1–2 段留在级别 1。
- **首个 9 段升级失败时**（`move_count >= 9` 但 G1/G2/G3 收缩后 `ZG2 < ZD2`）：该中枢不再尝试升级，全部段留在级别 1 并继续按段中枢规则延伸；不得丢弃已成立的段中枢。
- 9 段升级只是常见严格派工程规则，不宣称是原文逐字定义，文档中明确标注。

### 扩张（两个同级中枢合并）

- **口径（保留 GG/DD，但加双条件与上限）**：相邻同级别中枢需同时满足
  1. 波动区间重叠：`prev.GG >= curr.DD && curr.GG >= prev.DD`；
  2. **核心区间也重叠**：`prev.upper >= curr.lower && curr.upper >= prev.lower`（`upper/lower` 即 `ZG/ZD`）。
  只用条件 1 会把“核心区完全不同、仅被毛刺撑宽”的两个中枢合并（如 A 核心 `[18,20]`、`GG=100`/`DD=5`，B 核心 `[88,90]`），再叠加“高级别隐藏低级别”会让 A 凭空消失，故加条件 2。
- 合并区间：`upper = min(prev.GG, curr.GG)`、`lower = max(prev.DD, curr.DD)`；`gg = max(prev.GG, curr.GG)`、`dd = min(prev.DD, curr.DD)`。
  - **注意**：`upper/lower` 用 `GG/DD` 求交是“扩张（波动区间）”的定义；与上文『9 段升级』用子中枢 `ZG/ZD` 并不矛盾——二者是不同概念，文档需并列说明。
- **终止约束（防止整段历史并成一个巨中枢）**，三者同时生效：
  - 链长上限：`move_count <= MAX_EXPANSION_MEMBERS`（模块常量，默认 4）；
  - 时间跨度上限：合并后 `end_index - start_index <= MAX_EXPANSION_SPAN_BARS`（模块常量，默认 250）；
  - 只允许“同级中枢列表中相邻、中间无其它同级别中枢”的链式合并；超过任一上限即停止吸收，保留为多个同级别中枢。
- `is_sure = prev.is_sure && curr.is_sure`；`kind = Expansion`；`move_count = 合并的子中枢数`。
- 左到右逐对合并，合并结果继续与右侧比较；级别 3 只在级别 2 中枢之间做**一次**扩张（不递归到级别 4），同样受上述上限约束。
- **升级流水线（互斥，避免同一段重复产出 level2）**：
  1. 先产出全部 level1 段中枢；
  2. 对 `move_count >= 9` 的 level1 做 9 段升级，升级后的 level1 标记“已升级”，**不再参与扩张**；
  3. 只在“未被升级的 level1”之间做扩张（产出 level2）；
  4. level3 只在 level2 之间扩张，且同一子中枢只能进一个上级中枢（父子引用唯一）。
  - 对象层与显示层都按此流水线去重，不能只靠显示层排序。

### 虚中枢与确认

- `is_sure = true` 当且仅当所有构成段 `is_sure = true`，**且不含窗口起点切出的首段**（该段是窗口伪影，参与构建的中枢一律标虚，默认不绘制，仅对象/调试保留）。
- 中枢是否已结束不影响确认状态（已结束但含未确认段仍为虚；未结束但构成段全确认仍为实）。
- **`is_sure` 非稳定**：`czsc` 尾部完成笔可能被新 K 线破坏/合并而重构（见 `signals.rs` 注释），重算后可能改判；文档需写明“`is_sure=true` 是当前快照认定，不保证不重绘”。
- 任一未确认段参与构建的中枢：`is_sure = false`，图上用灰色/负编码表示。
- 级别 2/3 中枢：所有构成段/子中枢都确认才算确认。
- 已知后果（必须写入风险与 UI 说明）：尾部未完成段恒 `is_sure=false`，故**最新中枢通常为虚、407/408 在近端不会出现**，只能事后回看。

### 三买三卖

- 只对 `is_sure = true` 的级别 1 段中枢计算，每个中枢最多一个三买和一个三卖；虚中枢不产出 407/408。
- **离开段与回抽段的准确定义**（修正原草案的方向错误）：
  - 中枢结束时，“离开段”= 第一个与 `[ZD,ZG]` 完全无交集的段；因“有交集即延伸”会把突破段吸收，**离开段的实际方向总是与离开方向相反**：
    - 向上离开：离开段是**下跌段**（整体在 `ZG` 上方，`low > ZG`），其终点即回抽低点；
    - 向下离开：离开段是**上涨段**（整体在 `ZD` 下方，`high < ZD`），其终点即反抽高点。
- **三买**：向上离开段的终点（回抽低点）`> ZG` → 三买锚在该离开段终点。其后一段（向上）若低点仍 `> ZG` 作为确认，**锚点不回移**。
- **三卖**：向下离开段的终点（反抽高点）`< ZD` → 三卖锚在该离开段终点。其后一段（向下）若高点仍 `< ZD` 作为确认，锚点不回移。
- 离开段与确认段都必须 `is_sure = true`，否则不产出 407/408。
- 任一后续段重新进入 `[ZD,ZG]` 则本次失败，不补第二次；级别 2/3 的三买三卖留给 L3。
- 复用 `BuySellPoint`，`func_id = 407/408`；阶段 B 为 `SignalKind` 增加 `SIGNAL_SEG_BUY3=7`、`SIGNAL_SEG_SELL3=8`（与 405/406 区分），`label` 加“段级”前缀供显示。
- **锁定测试**：中枢 `[100,110]` / 上 `105→130`（延伸段）/ 下 `130→115`（离开段、回抽低点 115）→ 三买必须锚在 115，而非随后上涨段的顶部。

## 实施阶段

- **阶段 A（L1）**：`strict_pivots` 段中枢（仅 level=1）+ 虚中枢 + `GG/DD` + 305–308（level 恒 1）+ 单元测试；交付 `analyze_csv --compare` 离线验收。**A 阶段不交付公式模板**，也不引入 `PivotKind`/`kind`/407/408。
- **阶段 B（L2）**：在 A 的移动列表上加 9 段升级、扩张、级别 2/3 和顶层可见选择；同一提交内新增 `PivotKind`（三值）与 `kind` 字段、407/408 通道、`SignalKind` 7/8；通道与对象字段复用。
- **阶段 C**：DLL 包装、`main_chart_strict.tn6`、文档与真机验证。
- **决策点（可量化）**：A 验收后若“窄中枢噪声”已明显缓解则 B 可暂缓。判定用 `analyze_csv --compare`（3 份 CSV：`002340_day`、`002340_30m`、`999999_day`，同窗口同 `min_bi_len`）：
  - 严格中枢数量 ≤ 笔中枢数量 × 60%；
  - 严格中枢 `[ZG,ZD]` 中位宽度 ≥ 笔中枢中位宽度 × 1.5；
  - 窄中枢（宽度 < 0.5×ATR(14)）占比严格 < 笔中枢；
  - 至少命中 1 个 407/408（阶段 B 后）。
  不达标则先修规则/门槛，不上 B。
- **提交拆分（AGENTS 功能边界）**：A 只加实际产出的协议项（`StrictPivotZone` 的**全部字段但剔除 `kind`**、305–308）；B 才加 `PivotKind`（含 2/3）、`kind` 字段、407/408、`SignalKind` 7/8。任何阶段都不得出现“已定义但无产生路径”的枚举值/字段。

## 公开接口与实现

### Proto

**阶段 A 新增**：

- `StrictPivotZone`（A 阶段不含 `kind`，`level` 恒 1）：
  - `uint32 start_index = 1`、`uint32 end_index = 2`、`double upper = 3`、`double lower = 4`、`double middle = 5`
  - `double gg = 6`、`double dd = 7`、`uint32 level = 8`、`bool is_sure = 9`、`uint32 move_count = 10`
  - 说明：`gg/dd` 不被任何通道消费，仅供对象输出（`include_objects=true`）与离线诊断/扩张逻辑使用，属调试字段而非生产契约。
- `AnalysisSnapshot` 新增 `repeated StrictPivotZone strict_pivots = 8;`（现有字段 1–7，字段号 8 空闲，已核对 `analysis.proto`）。
- `ChannelKind` 新增 `CHANNEL_ZS_STRICT_UPPER=305`、`CHANNEL_ZS_STRICT_LOWER=306`、`CHANNEL_ZS_STRICT_MIDDLE=307`、`CHANNEL_ZS_STRICT_BOUNDARY=308`。

**阶段 B 追加**（同一提交）：

- `enum PivotKind { PIVOT_KIND_UNSPECIFIED=0; PIVOT_KIND_SEGMENT=1; PIVOT_KIND_NINE_SEGMENT_UPGRADE=2; PIVOT_KIND_EXPANSION=3; }`（proto3 枚举值包级共享，带前缀避免冲突）。
- `StrictPivotZone` 追加 `PivotKind kind = 11;`（proto3 加字段线兼容）。
- `ChannelKind` 新增 `CHANNEL_BUY3_STRICT=407`、`CHANNEL_SELL3_STRICT=408`。
- `SignalKind` 新增 `SIGNAL_SEG_BUY3=7`、`SIGNAL_SEG_SELL3=8`（有产生路径：407/408）。

### 通道编码

- 305–307：只填“每根 bar 唯一可见中枢”的 `upper/lower/middle`，区间外为 0；**虚中枢取负**（公式用 `ABS` 取价、用符号判 `is_sure`，避免整段着色缺少确认信息）。A 股价格恒 > 0，0 可作哨兵，与现有 301–303 填法一致；“中枢结束”由 308 的右界码表达，不靠 0。
- 308 边界码（`level*10+edge`，避免与 304 的 1/2/3 撞码、解码更直观）：
  - `edge`：0=区间内部（只带级别）、1=左界、2=右界；
  - `level`：1/2/3；
  - 实中枢：区间内 `10/20/30`，边界 `11/12`、`21/22`、`31/32`；虚中枢取负；
  - `0` = 无中枢。公式解码：`实 := v > 0; 虚 := v < 0; level := INTPART(ABS(v)/10); edge := ABS(v) - level*10;`
  - **区间内必须写 `level*10`**：若只在边界写值，公式在区间内的级别恒 0，会退化成“只有左右竖线、没有上下沿”。
  - 负值只用于条件比较，**绝不可直接作为 `DRAWICON/STICKLINE` 的价格参数**。
  - 多级同 bar 处理：被顶层可见选择隐藏/淘汰的中枢**不写入 305–308**；可见集内两中枢不共享 bar，故边界不会互相覆盖。
- **顶层可见选择（每根 bar 至多一个可见中枢）**：可见优先级 = `is_sure`（实优先）> `level`（高优先）> `move_count`（大优先）> `start_index`（靠后优先）。即“实高级别隐藏与其时间重叠的低级别；**实低级优先于虚高级别**（虚高级别不覆盖已确认低级别）”。被淘汰者仍保留在对象 `strict_pivots` 中供调试。
- 407/408：在段级三买/三卖锚点输出分型价格。

### Rust 模块

- 新增 `rust_service/src/analyzer/strict_pivots.rs`：
  - `Move`（**只表示段**）：高/低（`seg_envelope` 聚合）、起止 K 线下标、方向、`is_sure`、`seg_index`。
  - `StrictPivot { level, kind, is_sure, start_index, end_index, upper, lower, middle, gg, dd, move_count, move_indices, parent, children }`。
  - `StrictPivotSet { pivots: Vec<StrictPivot>, segments: Vec<Move> }`（删除原草案“`levels` 保存各级移动列表供三买三卖”的错误设计：三买三卖只需段列表 + 中枢结束段下标）。
  - `build_strict_pivots(&[BI], &[Segment]) -> StrictPivotSet`
  - `build_strict_signals(&StrictPivotSet) -> Vec<BuySellPoint>`
  - `seg_envelope(bis, seg)`：段包络统一函数（严格中枢与 `objects.rs::map_segment` 共用）。
  - 现有内部 `Segment` 只有 `start_bi_idx/end_bi_idx`；不新增字段到 `Segment`。
- 修改 `analyzer/mod.rs`：
  - 仅在请求 305–308/407–408 或 `include_objects=true` 时构建严格中枢；**注意 DLL `BuildRequest` 固定请求全量通道，因此 DLL 路径默认每次都会构建**，性能按此计算。
  - **未知 func_id 不得使整个请求失败**：`assemble_snapshot` 的通道构建对未知/未支持 id **跳过并补全 0 通道**（`resolve_channel_meta` 仍对测试报错），避免“新 DLL + 旧 service”时整图全零。
  - `assemble_snapshot` 同时输出笔中枢、严格中枢、严格三买三卖；严格信号仅在 `include_objects=true` 时并入 `signals`，label 加“段级”前缀。
  - `strict_pivots` 对象**仅 `include_objects=true` 时填充并序列化**（与 `pivots` 同规则）。
- 修改 `analyzer/objects.rs`：新增 `TryFrom<&StrictPivot> for StrictPivotZone`；`map_segment` 的 high/low 改用 `seg_envelope`（统一段高低口径）。
- 修改 `analyzer/channels.rs`：
  - 保留现有 `build_channel` 签名与行为；`resolve_channel_meta` **只保留 `build_channel` 实际支持的 id**，严格通道使用独立的 `resolve_strict_channel_meta`（避免“元数据声称支持、dispatch 却 Err”的矛盾）。
  - 新增 `build_strict_channel(func_id, data_len, strict_pivots, strict_signals)`；`assemble_snapshot` 按 func_id 分派。
  - `SUPPORTED_FUNC_IDS` 加入 305–308（A）/407–408（B），**并顺带补入现有遗漏的 203/204**（消除既有漂移；见 §文档与诊断）。
  - 更新 `channels.rs` 模块注释：严格通道分派在 `assemble_snapshot`，`build_channel` 不再是“所有通道唯一入口”。
  - 不顺手修其它缺口，不新增依赖。
- 修改 `analyzer/incremental.rs` / `handlers/analyze.rs`：严格中枢每次从当前 `czsc` 全量重算（非增量维护），但必须保证“全量路径与增量承接后输出逐值一致”，并补测试（见 §测试与验收）。

### DLL 与公式

- `bridge_dll/src/bridge_functions.cpp`：
  - 新增 `Func305..Func308`、`Func407`、`Func408` 包装与 `g_calc_funcs` 表项；**无需改 `PluginTCalcFunc.h` 签名或 `exports.cpp` 导出表**。
  - `BuildRequest.requested_func_ids` 加入新通道；**保持“恒请求全量通道列表”为硬不变量**（缓存 `CacheKey` 不含 `requested_func_ids`，一旦按需请求会错命中→全零；加运行时断言/单测守护）。
  - 407/408 因 `func_group = (func_id in 400..500)` 自动归 `single_level_signals`，**无需改 DLL 分组逻辑**；注意因此严格模板同时调 305–308(structure)+407–408(signals) 会让同一数据段做 **2 次全量分析**，计入性能预算。
  - 版本错配（新 DLL + 旧 service）不在现有冒烟覆盖范围，列为真机部署的“分阶段升级”专项检查项。
- `bridge_dll` 的 `FillFromSnapshot` 无需改动，仍按 func_id 取通道；新增 C++ 单测：传入不含 305 的 snapshot，断言输出全 0 且返回 false。
- 新增 `tdx_formula_examples/main_chart_strict.tn6`（阶段 C）：
  - 读取 305–308、407–408；用 `ABS`/`INTPART` 解码 `level*10+edge` 三态（0=无、负=虚、正=实）。
  - 级别 1/2/3 用不同颜色；虚中枢灰色；左右竖线用 `STICKLINE`；上下沿用折线/`DRAWNULL` 断开；**禁止复制 `main_chart_full.tn6` 的 `中枢界=1/=2/=3`、`<>3` 片段**（那是 304 的旧编码）。
  - 407/408 用图标或文字标注段级三买/三卖；负值只参与条件，不作价格参数。
  - 现有 `main_chart_full.tn6` 不改。

### 文档与诊断

- 新增 `docs/严格中枢规则.md`：级别、公式、9 段说明、扩张、虚中枢、通道码、与原文差异；明确记录“完全离开即结束”“9 段用子中枢 ZG/ZD 收缩”“扩张用 GG/DD + 核心区间双条件 + 上限”“每 bar 唯一可见中枢”四条口径，并补“窄中枢判读”。**该文档为唯一规范文本**。
- 更新 `docs/技术方案_通达信缠论自动标注系统.md`：§4.2 模块表、§5.1 消息定义（`StrictPivotZone`/`PivotKind`）、§6.4 输出快照、§7.2 通道表、§7.4 映射约定（补“308 是编码通道”例外）、新增 §8.x“严格派中枢”、§11 性能、§14 已知限制、§15 路线图、§16 风险、§17 验收。
- 更新 `README.md`（特性/通道表/仓库结构/测试/文档表）、`docs/使用说明.md`（模板表与清单）、`docs/真机验证清单.md`、`CHANGELOG.md`、`CONTRIBUTING.md`（模块职责表加 `strict_pivots.rs`、公式清单加 strict 模板）；**更新 `AGENTS.md` 的通道范围说明**（原“101–304 结构 / 401–406 买卖点”需加入 305–308/407–408）。
- 纠正 README / 技术方案中“单一来源、不会漂移”的表述，并说明本次已把 203/204 补入 `SUPPORTED_FUNC_IDS`。
- 扩展 `rust_service/examples/analyze_csv.rs`：新增 `--compare` 模式，输出笔中枢 vs 严格中枢的数量、中位宽度、窄中枢占比、虚中枢占比、每千根数量、407/408 命中、严格构建耗时，并内置阈值断言（不达标 `exit 1`）；`CHANNEL_NAMES` 补齐 203/204/305–308/407/408。

## 测试与验收

- 单元测试（`strict_pivots.rs`）：
  - 门槛：段内无子中枢且笔数 < `MIN_SEG_BI_LEN` → 不参与重叠；满足门槛才成中枢。
  - 3 段重叠成中枢；区间固定；延伸不改变 `upper/lower`，更新 `gg/dd`。
  - 下一段仍重叠 → 延伸；完全在区间外（`high<ZD || low>ZG`）→ 中枢结束且离开段不属于中枢；跨区间段 `low<ZD && high>ZG` → 按延伸处理；`ZG==ZD` 等号边界。
  - 离开后返回 → 旧中枢不延伸，返回段重新扫描（可形成新中枢）；段归属唯一。
  - 前 3 段无重叠 → 后移扫描。
  - 9 段升级、12 段延伸、剩余 1–2 段留级别 1；**首个 9 段升级失败后 10–12 段留在级别 1**。
  - 扩张：GG/DD 重叠但核心区间不重叠 → **不合并**；双条件都满足才合并；链长/时间跨度上限触发时停止吸收；级别 3 只扩一次。
  - 升级流水线互斥：已升级的 level1 不再参与扩张；同一子中枢只进一个上级。
  - 虚中枢、窗口起点首段伪影、零宽度、顶层可见选择（“每 bar 唯一可见 + 实低级优先于虚高级别”）。
  - 段级三买/三卖：锚在离开段终点（`[100,110]/130/115` 用例）；确认段低点仍 >ZG；二次回抽不补；回抽进入区间则失败。
  - `is_sure`：尾部未完成段、首段伪影。
- 集成测试：
  - `002340` 日线/`002340_30m` 真实数据：**性质断言为主**（`upper>=lower`、`gg>=upper>=lower>=dd`、level∈{1,2,3}、顶层中枢时间不重叠、严格中枢数 ≤ 笔中枢数、407 满足低点>ZG、408 满足高点<ZD、虚中枢处 308 为负），保留 1 条“中枢个数 + 首尾日期”的粗快照，外部化为 `tests/data/golden/strict_002340_*.csv` 并提供 `--bless` 与“算法微调后需人工确认 diff”说明。
  - 增量/窗口一致性：`analysis_window=0` 全量与 `enable_incremental` 承接后严格中枢逐值一致；同一序列尾部追加 N 根后，右侧 200 根内中枢 `(start,end,upper,lower,is_sure)` 稳定；平移窗口 50 根只在窗口首部变化。
  - 回归（低成本的“逐值一致”）：同一请求分别 `requested_func_ids=[301,302,303,304]` 与“全量含新通道”运行，断言 301–304 通道逐值完全相等（验证“新增不影响旧通道”，替代昂贵的全量 golden）。
  - 通道：305–308 的 `level*10+edge` 编码、负值虚中枢、407/408 命中；序列化/反序列化。
  - 一致性测试（防漏注册）：①遍历 `SUPPORTED_FUNC_IDS` 断言 `resolve_channel_meta` 均 `Ok`；②`SUPPORTED_FUNC_IDS` 快照断言；③跨语言：C++ 侧遍历 `ChannelKind` descriptor 断言 `g_calc_funcs` 有对应 `nFuncMark`，Rust 侧遍历同一 descriptor 断言均可解析。
- 工具验证：
  - `cargo test --manifest-path rust_service/Cargo.toml`
  - `cd rust_service && cargo run --example analyze_csv -- ../tests/data/002340_day.csv 6 0`（注意相对路径 `../tests/data/`）
  - `cargo run --example analyze_csv -- --compare ../tests/data/002340_day.csv ../tests/data/002340_30m.csv ../tests/data/999999_day.csv`
- 真机验证：
  - Windows x64 构建 DLL + 冒烟测试；更新 `ipc_client_smoke.cpp` 的请求列表与断言，覆盖 305–308 边界码、负值虚中枢、407/408 命中（需服务在线，Windows 本机手工）；新增 `FillFromSnapshot` 缺通道→全 0 单测；版本错配列为分阶段升级专项检查（不可自动覆盖）。
  - 通达信加载 `main_chart_strict.tn6`；检查级别 1/2/3 矩形、虚中枢灰色、左右边界闭合、407/408 标记、周期切换、公式行数/渲染性能。
- 验收标准：
  - 现有测试全绿，301–304 通道逐值不变。
  - 严格中枢规则测试全绿，真实数据无 panic。
  - 量化门槛（见 §实施阶段 决策点）达标。
  - 性能：严格管线额外耗时 **≤ 2ms@1000 根、≤ 5ms@3933 根**（在 `SnapshotProfile` 增加 `strict_pivots` 计时桶测量）；旧模板（仅 301–304）路径退化 **< 5%**。
  - 无新增依赖、无未消费的协议字段/通道；跨语言一致性测试通过。

## 假设与风险

- 假设：L2 = 段中枢 + 9 段升级 + 扩张 + 虚中枢 + `GG/DD` + 段级三买三卖；级别 3 封顶。
- 假设：图上只显示“每 bar 唯一可见中枢”；对象输出保留全部级别。
- 假设：9 段升级是常见严格派工程规则，不宣称原文逐字定义。
- 假设：延伸采用严格解释——完全离开即结束，不做“离开后返回再并入”；如需宽松延伸，另立开关/主题并补测试。
- 假设：段中枢要求“完成次级别走势”门槛（段内含子中枢或笔数 ≥ `MIN_SEG_BI_LEN`）。
- 假设：扩张保留 `GG/DD` 口径，但加“核心区间重叠 + 链长/时间跨度上限”三重约束。
- 风险：严格派规则本身有歧义；以 `docs/严格中枢规则.md` 为唯一口径，后续变更需配套测试。
- 风险：**段确认滞后**——尾部未完成段恒 `is_sure=false`，最新中枢通常为虚、**407/408 在近端不可用**，只能事后标注；须在 UI 与文档明确。
- 风险：**级别合并“一个大矩形套住整段历史”**——已用核心区间双条件 + 链长/跨度上限缓解，仍需真机观察视觉。
- 风险：**两套中枢系统同图冲突**——同时加载 `main_chart_full`(301–304) 与 `main_chart_strict`(305–308) 会矩形重叠、304 与 308 都是边界码但编码不同；需给“二选一/叠加”约定并在文档写明。
- 风险：**公式行数与渲染上限**——数百中枢的 `STICKLINE/DRAWICON` 可能触及通达信绘图上限；真机性能用例覆盖，必要时“只画最近 N 个/只画顶层”。
- 风险：**tn6 二次导出兼容**——模板需在通达信导出→手改→再导入，易损坏；须写明备份/重新导出流程。
- 风险：`strict_pivots` 对象在生产链路恒 `include_objects=false`，只是离线/调试字段，不是生产契约。
- 风险：级别 3 只做“级别 2 扩张”，不做 27 段升级；如实盘需要再扩展并明确上限。
- 风险：通道注册点较多（proto、channels、analyze_csv、DLL 表、冒烟请求、公式）；新增跨语言一致性测试避免漏注册。
- 风险：真多级别/区间套受 TDX 单周期输入限制；L3 需先改协议与跨周期上下文。
- 风险：`freq_hint` 现由 DLL 恒为 F30（既有缺陷），周期切换靠 `input_hash` 区分；严格中枢对周期更敏感，至少在已知限制中记录。

## 工期估算（含评审/文档）

| 阶段 | 主要工作项 | 估算 |
| --- | --- | --- |
| A | `strict_pivots` 段中枢 + 虚中枢 + GG/DD + 305–308 + 单测 + `analyze_csv --compare` | 4–6 人日 |
| B | 9 段升级 + 扩张 + 级别 2/3 + 顶层选择 + proto/DLL 扩展 + 单测 | 5–8 人日 |
| C | DLL 包装 + `main_chart_strict.tn6` + 文档 + 真机验证 | 5–8 人日 |
| 缓冲 | 真机联调 + 公式反复导出调试（CI 无法覆盖） | 3–5 人日 |
| 合计 | | 约 17–27 人日 |

- DLL 每次提交必须本机 x64 Release 构建 + 冒烟；公式调试按“改 → 导出 tn6 → 真机加载”循环计，无法自动化；A 的验收依赖 `analyze_csv --compare`，该扩展必须在 A 完成。

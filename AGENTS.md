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

- **功能边界（用户长期约束）**：只实现用户提出或明确同意的功能。禁止添加"预留但无功能实现"的代码——协议字段、枚举值、配置项、函数分支、依赖等，若无真实消费方（无产生路径/无数据源/无调用点）不得加入；已存在且被确认无功能的预留，按清理主题提交移除，不为其保留"以后可能用得上"的占位。
- 通达信 DLL ABI 硬限制：3 路输入 `float[]`（HIGH、LOW、CLOSE）+ 1 路输出 `float[]`；复杂缠论对象必须拆成多通道输出（func_id 101–304 为结构通道，401–406 为买卖点通道，901–903 为调试通道，950 为量能输入通道）。
- DLL 编译与全链路验证只能在真实 Windows x64 + 通达信环境进行；协议与 Rust 侧工作可跨平台。
- 提交保持主题化（`rust_service: ...`、`bridge_dll: ...`、`docs: ...`）；当前阶段避免大范围重命名和在 DLL 侧引入大量新依赖。
- 本仓库注释与文档使用中文，保持一致。

## Rust 设计与实现约定（rust_service）

### 设计取向

- 行为优先挂在类型上：用 `impl`、newtype、builder、RAII guard 和 trait 单态化承载逻辑，避免新增模块级自由函数；仅程序入口和测试夹具可保留自由函数。
- 优先零成本抽象：借用视图、`#[repr(transparent)]` 包装和泛型单态化优先；不要为了形式引入 `Box<dyn Trait>`、额外间接层或无真实消费方的 trait。
- 用 builder/链式 setter 表达可选输入，用 `TryFrom` newtype 表达已校验值，用 RAII 管理缓存租约、锁和请求生命周期。
- 不滥用 `Copy`/`Clone`/`unwrap`/`expect`：大对象不实现 `Copy`；克隆必须有语义理由；生产路径错误通过 `Result` 和 `?` 传播，锁中毒按既有策略恢复并记录日志，不新增裸 `unwrap`。
- 给类型补 derive 前先确认字段的 trait：`czsc_core` 的 `Mark` 没有 `Copy`，承载它的 `Fill` 就只能 `Debug`；含 `&'static` 引用或非 `Eq` 字段的类型无法派生 `Copy`/`PartialEq`。这时改用借用匹配（`match &spec.fill`）或去掉该 derive，不要顺手给底层类型加 trait。
- 抽象以真实调用方和不变量为边界，避免过度抽象；`std` 已有能力优先复用，不重复造轮子，也不为“可能以后有用”预留代码。
- 固定映射（func_id → 名称/行为）只维护一张 `const` 表：字段私有、经访问器读取，行为用枚举变体（如 `Fill`）表达；`handlers/version.rs`、`examples/` 等派生方一律从该表生成，禁止并行维护第二份清单（历史教训：`meta` / `SUPPORTED_FUNC_IDS` / 示例常量的条数与内容已经漂移）。
- 唯一使用者是测试的常量或分支视为无消费方：删除它并用既有 API 表达，改测试而不是保留占位（如 `AnalysisWindow::FULL` 用 `AnalysisWindow::new(0)` 替代）。

### 职责边界

- `handler` 是 `AnalysisEngine::analyze` 的上游：只负责请求生命周期、缓存、singleflight、并发槽、`spawn_blocking`、统计和响应组装，不直接决定增量/全量算法细节。
- `AnalysisEngine` 是分析用例边界并持有增量存储：负责增量承接、全量回退和状态提交；`SnapshotAnalyzer` 保持纯全量入口；analyzer 内部不感知 IPC、缓存和请求生命周期。
- `AppState` 只暴露必要访问器，缓存、并发槽和增量状态的不变量不得被外部模块绕过。
- IPC 层只负责帧读写、连接限制、超时和请求分派；协议错误映射和业务错误分类保持在 handlers/analyzer 边界。

### 可见性与对外暴露

- 默认私有，按 `pub(crate)` → `pub` 递进；只有 crate 外存在真实消费方（`src/main.rs`、`examples/`、`tests/`）才标 `pub`。`AppState`、`ChannelBuilder` 这类类型只经必要访问器对外，字段与常量保持私有。
- 同一公开项只给一个路径：优先逐个 `pub use`，不做 glob 或重复 re-export；没有外部使用者的 re-export 直接删除。
- `Cargo.toml` 的 `unreachable_pub = "warn"` 长期保留；命中时收敛可见性（包括 `tests/common/mod.rs` 的夹具），不要用 `#[allow]` 压制。
- 例外：唯一调用方整体在 `#[cfg(windows)]` 下的入口（如 `Handlers::dispatch`）保持 `pub`，否则非 Windows 构建会连锁触发 `dead_code`；这类项必须在文档注释里写明原因。
- 作为 `TryFrom`/`From` 的 `Error` 关联类型的错误值对外可达，即使本 crate 内无人构造也不要降级（如 `AlignError`）。

### 错误、边界与数据约束

- 公开分析 API 对用户输入不得 panic：索引前校验序列长度，通道写入做边界检查，空数据、缺失评估器等返回 `AnalysisError`，不使用 `assert!`/`expect` 兜底。
- 缓存键必须包含所有影响输出内容的输入（包括请求通道集合、对象开关等）；不能只按输入哈希或时间窗口复用不同语义的缓存。
- 配置解析必须严格：非法值返回 `ConfigError`，不静默回退默认值；布尔值直接用标准库 `FromStr` 语义；启动时校验正整数约束，并明确 `0` 的语义（如 `cache_capacity == 0` 表示禁用缓存）。
- 增量路径使用显式提交：未 `commit` 的 lease 在 `Drop` 时丢弃，提交应消费 lease；全量回退失败时恢复旧状态，避免破坏增量不变量。
- 数值转换优先 `u32::try_from`/`u64::try_from` 等可失败转换；不要用 `as` 做可能截断的生产路径转换。

### 标准库与性能

- 优先使用 `Entry`、`Option::get_mut`/`map_or`、`HashSet`、`sort_unstable` + `dedup`、`split_once`、`saturating_sub`、`Arc::unwrap_or_clone` 等标准库表达；避免手写 O(n²) 查找和重复容器实现。
- 共享容器（如 LRU）只保留一份泛型实现；`std` 无对应且确实需要的自写逻辑（INI 解析、IPC 长度前缀帧、算法特定峰值查找）应集中、可测试并说明理由。
- 热路径避免不必要的深拷贝、重复哈希和无界内存增长；缓存读取尽量在释放锁后再克隆。
- IPC 必须限制帧大小、连接数和读超时；长度字段用 `u32::try_from`，禁止截断写入。

### 验证

- Rust 改动至少通过 `cargo fmt --check`、`cargo clippy --all-targets --all-features -- -D warnings` 和 `cargo test`；不要用 crate 级 `#![deny(warnings)]` 代替按类别配置 lint。
- 文档门禁：`cargo doc --no-deps` 必须 0 warning；rustdoc 链接只能指向可达项，私有模块/类型用普通 code span（如 `rawbars`、`AnalysisSettings`）；改动公开符号时同步更新 `docs/` 中引用的旧名字。
- 行为变更或重构必须补充边界、等价性和回归测试；增量结果应与全量结果对齐，快速路径应与官方实现对齐。
- Windows 专用 IPC/DLL 代码无法在非 Windows 主机上完成编译和全链路验证；提交前仍需在真实 Windows x64 + 通达信环境验证。

<!-- gortex:communities:start -->
## Community Skills

| Area | Description | Explore |
|------|-------------|---------|
| Analyzer Cal Seg Sure | 106 symbols | `analyze(operation:"communities", id:"community-30")` |
| Analyzer 4 Dirs | 91 symbols | `analyze(operation:"communities", id:"community-38")` |
| Analyzer 1 Dirs Analysiserror | 72 symbols | `analyze(operation:"communities", id:"community-25")` |
| Analyzer 1 Dirs Combine | 58 symbols | `analyze(operation:"communities", id:"community-29")` |
| Handlers 2 Dirs | 50 symbols | `analyze(operation:"communities", id:"community-42")` |
| Analyzer 1 Dirs Accept | 50 symbols | `analyze(operation:"communities", id:"community-23")` |
| Analyzer Signalevaluator | 40 symbols | `analyze(operation:"communities", id:"community-35")` |
| Analyzer 1 Dirs Run Shadow | 36 symbols | `analyze(operation:"communities", id:"community-46")` |
| Bridge Dll Src 1 Dirs | 36 symbols | `analyze(operation:"communities", id:"community-2")` |
| Analyzer Build Channel | 30 symbols | `analyze(operation:"communities", id:"community-22")` |
| Include 2 Dirs | 26 symbols | `analyze(operation:"communities", id:"community-3")` |
| Analyzer Build Snapshot Populates Core C | 23 symbols | `analyze(operation:"communities", id:"community-24")` |
| Analyzer Fx Mid Index | 23 symbols | `analyze(operation:"communities", id:"community-26")` |
| Rust Service Src 2 Dirs Appconfig | 20 symbols | `analyze(operation:"communities", id:"community-39")` |
| Analyzer Pivot Edges Follow Trend | 20 symbols | `analyze(operation:"communities", id:"community-27")` |
| Tests Common Analyze Request Ex | 19 symbols | `analyze(operation:"communities", id:"community-47")` |
| Analyzer 1 Dirs Buysellpoint | 16 symbols | `analyze(operation:"communities", id:"community-33")` |
| Analyzer Buysellstate | 15 symbols | `analyze(operation:"communities", id:"community-32")` |
| Analyzer 1 Dirs Assert Parity | 15 symbols | `analyze(operation:"communities", id:"community-31")` |
| Analyzer Bs Sig | 14 symbols | `analyze(operation:"communities", id:"community-37")` |

<!-- gortex:communities:end -->

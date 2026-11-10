# Rust 代码审查：`rust_service`（`tdx_czsc_service` v0.9.0，edition 2024）

这是通达信 x64 通道分析服务的应用型 crate（lib + bin + examples + tests）：Named Pipe 收请求 → 校验/切窗 → czsc-core 逐根演化 → 分型/笔/中枢/段/严格中枢/买卖点 → float 通道。整体结构清晰、测试厚实，主要问题是**一条 panic 路径会把 singleflight 登记永久留在表里**，以及**若干可被外部数据触发的下标 panic**。

实测：`cargo fmt --check` 通过；`cargo test` 105 通过 0 失败；`cargo clippy --all-targets -- -W clippy::pedantic` lib 侧 215 条告警（124 条 doc backticks、39 条 `usize → u32` 截断、21 条 `#[must_use]`）。`protoc` 走的是 `build.rs` 的 vcpkg 兜底路径。

## Must fix

### 1. 分析线程 panic 后 singleflight 登记永久泄漏（`rust_service/src/handlers/analyze.rs:183`）
**Rule:** Panics on reachable paths / 资源必须随失败路径释放（M-PANIC-ON-BUG；Effective Rust 项 18 的"守卫要在所有出口生效"）
**Problem:** `spawn_blocking(...).await.map_err(...)?` 在 `JoinError`（分析线程 panic）时用 `?` 直接返回，跳过了 `release_in_flight`。该 `CacheKey` 从此永远留在 `in_flight` 里：后续同数据段请求会先等 15s、再等 15s、第三次才兜底自己算（`handle:130-141`），且多个客户端各自重复全量分析。结合下面第 3 条，这条路是可触发的。
**Fix:**
```rust
    let analyze = tokio::task::spawn_blocking(move || -> Result<_> {
        let params = analyzer::AnalysisParams::from((&analyze_request, &analyze_state.config));
        analyze_snapshot(&analyze_request, &params, &analyze_state)
    })
    .await;
    let analyze = match analyze {
        Ok(result) => result,
        Err(err) => {
            // 分析线程 panic 也必须撤销在途登记，否则同 key 后续请求每次白等 30s。
            if registered {
                release_in_flight(state, &cache_key);
            }
            return Err(anyhow!("analysis task join failed: {err}"));
        }
    };
```

### 2. `analysis_parallelism = 0` 让所有分析请求永久挂起（`rust_service/src/config.rs:95` / `rust_service/src/state.rs:48`）
**Rule:** 配置输入必须在信任边界上校验（Google Rust Style Guide, "validate at boundaries"）
**Problem:** INI/env 的 `set_usize` 接受 `0`，`Semaphore::new(0)` 之后 `acquire_owned().await` 永不返回：服务不报错、不退出，所有分析请求无限挂起，只有 health 还能回。运维写错一个数字就会静默全站不可用。
**Fix（`state.rs:47`，一处覆盖所有构造路径）:**
```rust
        // 0 会让 Semaphore::new(0) 永久阻塞全部分析请求。
        let analysis_parallelism = config.analysis_parallelism.max(1);
```

### 3. 特征序列状态机的 `panic!`（`rust_service/src/analyzer/segs.rs:268`）
**Rule:** 数据驱动路径不得 panic（M-PANIC-ON-BUG）；`debug_assert` 用于不变量，`panic!` 只用于真正的程序错误
**Problem:** `EigenFX::add` 在"三个元素已齐"时 `panic!`。当前上层调用者（`cal_seg_sure` / `find_revert_fx` / `reset`）都会在返回 `true` 后立刻 break，所以眼下大概率不可达；但这条不可达性靠的是三个函数之间的隐式约定，没有本地断言证明。行情数据一旦踩出来，就是整条分析线程 panic → 命中第 1 条的泄漏。
**Fix:**
```rust
        } else {
            // 三个元素齐备仍继续入列属状态机异常：重置后从当前笔重新开始，
            // 不让异常数据把分析线程打崩。
            debug_assert!(false, "特征序列已有 3 个元素: bi_idx={bi_idx}");
            self.clear();
            self.lst.push(bi_idx);
            self.treat_first_ele(bi_idx, bis)
        }
```

## Should fix

### 4. 7 处 `lock().unwrap()`：一次 panic 此后全服务 500（`handlers/analyze.rs:29,37,70,128,280,287,291`）
**Rule:** Lock hazards（poisoned lock 处理）；错误处理不得把局部故障升级为全局故障
**Problem:** 锁内的临界区（`HashMap::get/insert/remove`、`take/put`）本身不 panic，所以现在基本安全；但这些锁是 `std::sync::Mutex`，一旦未来有人在临界区里加一句会 panic 的代码，此后**每个**请求都在 `lock().unwrap()` 处 panic。被保护的都只是 map 操作，没有跨字段不变量，可以安全恢复。
**Fix:**
```rust
/// 取锁：临界区只有 map 操作、无跨字段不变量，中毒后数据仍可用；
/// 恢复而不是 unwrap，避免一次 panic 把整个服务钉死。
fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
```

### 5. 公共函数 `build_raw_bars` 直接索引、窗口可下溢（`analyzer/rawbars.rs:57` 与 `:60`）
**Rule:** 库的公开函数不得对可预期输入 panic（C-GOOD-ERR / M-PANIC-ON-BUG）
**Problem:** 它是 `pub`（`lib.rs` 暴露 `analyzer`），内部假设 `series.close.len() >= data_len` 且 `window_start <= data_len`，这两条只由另一个模块的 `validate_series_lengths` 保证。直接调用（或将来新增调用点）传短序列 → `series.close[i]` 越界 panic；传大 `window_start` → `data_len - window_start` 在 debug 下溢出 panic、release 下巨量 `with_capacity`。
**Fix（复用已有校验，两行）:**
```rust
    super::validate_series_lengths(request.data_len, series)?;
    let window_start = window_start.min(request.data_len as usize);
    let mut bars = Vec::with_capacity(request.data_len as usize - window_start);
```

### 6. 通道填充用裸下标写入（`analyzer/channels.rs:199,208,211,247,252`，及 `fill_zs_channel`）
**Rule:** 索引前必须证明边界（Google Rust Style Guide）；`i32 as usize` 不做符号检查（clippy `cast_sign_loss`）
**Problem:** `fx_mid_index` 返回 `bar.id as usize`（`objects.rs:38-39`，clippy 已告警"may lose the sign"），负 id 会变成巨大 `usize`；`fill_zs_channel` 连"是否越界"都没检查，而同一文件的 `fill_zs_boundary_channel` / `fill_strict_price` 都检查了——同类代码行为不一致说明这里靠的是运气。
**Fix:**
```rust
        if let Some(index) = fx_mid_index(fx)
            && let Some(slot) = values.get_mut(index)
        {
            *slot = fx.fx as f32;
        }
```
`fill_zs_channel` 同样加 `if end >= values.len() { continue; }`；信号通道改 `if let Some(slot) = values.get_mut(point.index)`。

### 7. `unwrap_or(0)` 伪造对齐下标，与 `qualified` 的文档相反（`analyzer/strict_pivots.rs:108-109`）
**Rule:** 文档契约与实现一致；不要用魔法值代替"不可用"
**Problem:** `Move.qualified` 的注释是"笔数足够且**有可对齐的 K 线下标**"，但实现里 `qualified` 完全不看下标，而 `start_index/end_index` 对不齐时静默变成 `0`：一个无法对齐的段会被画成从第 0 根 K 线开始的中枢；`build_strict_signals` 里离开/确认段不看 `qualified`，407/408 会被锚到下标 0。
**Fix（最小版）:**
```rust
    let start_index = bi_start_index(bis.get(seg.start_bi_idx));
    let end_index = bi_end_index(bis.get(seg.end_bi_idx));
    Move {
        ...
        qualified: start_index.is_some() && end_index.is_some() && envelope.is_some() && (has_pen_pivot || bi_count >= MIN_SEG_BI_LEN),
```
更彻底的做法是 `to_move` 返回 `Option<Move>`，让对不齐的段根本不进 `StrictPivotSet.segments`。

### 8. `AlignError` 缺 `Display` / `std::error::Error`（`analyzer/objects.rs:30`）
**Rule:** Errors implement `std::error::Error` + `Display`（C-GOOD-ERR, Rust API Guidelines）
**Problem:** 它是公开的 `TryFrom<&FX>` 错误类型，但调用方目前只能 `.ok()` 丢掉它；将来想用 `?` 或 `anyhow` 就写不了，只能改成 `String`。
**Fix:**
```rust
impl std::fmt::Display for AlignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("对象无法对齐到原始 K 线下标")
    }
}
impl std::error::Error for AlignError {}
```

## Consider

### 9. "请求值 > 0 否则配置" 的规则被抄了 4 份（`handlers/analyze.rs:87`；`analyzer/mod.rs:80,147,166,270`）
`max_bi_num`/`min_bi_len`/`analysis_window` 的解析在 `CacheKey`、`AnalysisParams::from`、`build_snapshot_profiled`、`assemble_snapshot_profiled` 各写一遍。今天是对的，但缓存键与分析参数一旦有一处不同步，就会出现"缓存串味"这种极难查的 bug。抽一个 `fn pick(option: u32, config: u32) -> u32 { if option > 0 { option } else { config } }` 全部改调它。

### 10. `request.clone()` 复制整份行情序列（`handlers/analyze.rs:176`）
后面不再用 `request`，可直接 `let analyze_request = request;` 移进闭包，省掉每请求一次 `high/low/close/vol/amount/dt` 深拷贝。

### 11. `same_bar` 不比较 `id`（`analyzer/incremental.rs:28`）
目前靠 `dt` 兜住了滑动窗口的错配（合成时间轴用绝对下标，真实 dt 也各不相同）。加一行 `a.id == b.id` 是零成本的更强保证，避免将来出现"值全等但下标已平移"被误判为可承接。

### 12. `SignalEvaluator.params` 永远是空表（`analyzer/signals.rs:570`）
全仓只有 `HashMap::new()` 赋值，没有一处 insert，`ParamView` 永远走默认参数。按本仓库"不得保留无消费方的配置路径"的约定，要么删掉字段改用一个明确的默认 `ParamView`，要么写一行注释说明"暂不支持自定义信号参数"。

### 13. 缓存每请求克隆 2 次整份快照（`cache/mod.rs:44`）
`get` 返回 `AnalysisSnapshot`（25 通道 × data_len × 4B，4000 根约 400KB）的克隆，`insert` 时又克隆一次。若将来大窗口下成为热点，把缓存值换成 `Arc<AnalysisSnapshot>` 即可；现在不必动。另外 `capacity == 0` 仍会存 1 条（`len() >= capacity` + 空队列 `pop_front()` 为 `None`），语义上"0 = 不缓存"不成立。

### 14. 测试里的通道清单是过期副本（`analyzer/channels.rs:363`）
`const ALL: [u32; 17]` 手抄了 17 个 id，漏了 203/204/305–308/407/408（`SUPPORTED_FUNC_IDS` 是 25 个）。测试名叫 `channel_meta_covers_all_supported_ids` 但并没有覆盖全部；改成 `for id in SUPPORTED_FUNC_IDS` 就能自动跟上新增通道。

## What's already good
- 错误分层干净：`AnalysisError`（thiserror）+ `anyhow` 上下文 + 协议状态码映射，且有 `downcast_through_anyhow_context` 测试锁住行为。
- 并发模型想得很细：singleflight 的"锁内 enable 等待 + 先写缓存再 notify"避开了丢失唤醒；`spawn_blocking` + 信号量 + 增量路径不占槽位。
- 增量承接有真正的等价性测试：追加、等长原地更新、前缀失配拒绝、结构-only 步骤保持评估器同步，全部与全量路径对照。
- 算法规则（中枢前后端裁剪、段、9 段升级/扩张、严格中枢可见性、407/408 锚点）都有针对性回归测试，`cargo test` 105 项全绿。

---
未做的检查：没有逐条对照缠论原文核对 `segs.rs`/`signals.rs` 的算法正确性（只审了 panic/边界/接口）；只在 dev profile 下跑，未验证 release（`overflow-checks` 关闭后第 5 条会从 panic 变成巨量分配）；DLL↔Pipe 全链路需要在真实 Windows + 通达信环境验证。风险点：第 1 条一旦被触发，表现是"某些股票/周期每次刷新都卡 30 秒"，很容易被误判成行情慢。

## 修复记录

`rust_service/Cargo.toml` 已加 `[lints.clippy]`：`pedantic = "warn"`，另定向 allow `doc_markdown`（中文注释噪音）、`cast_possible_truncation` / `cast_precision_loss`（数值口径需人工判断）、`module_name_repetitions`（Python 版 1:1 端口）、`must_use_candidate`。

| # | 结论 | 落点 |
|---|------|------|
| 1 | 已修复：`JoinError` 也撤销在途登记 | `handlers/analyze.rs` `handle` |
| 2 | 已修复：并发上限钳到 ≥1 | `state.rs` `AppState::new` |
| 3 | 已修复：`panic!` 改为 `debug_assert` + 重置状态 | `analyzer/segs.rs` `EigenFX::add` |
| 4 | 已修复：统一 `lock()` 辅助函数（中毒恢复） | `handlers/analyze.rs` |
| 5 | 已修复：入口复用 `validate_series_lengths` + 窗口钳位 | `analyzer/rawbars.rs` `build_raw_bars` |
| 6 | 已修复：`get_mut` 边界检查；`fx_mid_index` 改用 `usize::try_from` | `analyzer/channels.rs`、`analyzer/objects.rs` |
| 7 | 已修复：`qualified` 纳入下标可对齐条件 | `analyzer/strict_pivots.rs` `to_move` |
| 8 | 已修复：`AlignError` 实现 `Display` + `Error` | `analyzer/objects.rs` |
| 9 | 已修复：抽出 `analyzer::pick`，4 处统一调用 | `analyzer/mod.rs`、`handlers/analyze.rs` |
| 10 | 已修复：`request` 直接 move 进闭包 | `handlers/analyze.rs` |
| 11 | 已修复：`same_bar` 增加 `id` 比较 | `analyzer/incremental.rs` |
| 12 | 已修复：注明 `params` 恒为空表、走默认参数 | `analyzer/signals.rs` |
| 13 | 部分修复：`capacity == 0` 不再存 1 条；`Arc<AnalysisSnapshot>` 按报告建议暂缓 | `cache/mod.rs`、`analyzer/incremental.rs` |
| 14 | 已修复：删掉过期 `ALL` 副本，并入 `supported_ids_are_all_resolvable` | `analyzer/channels.rs` 测试 |

验证：`cargo fmt --check` 通过；`cargo test` 104 通过 0 失败（原 105 项，删掉 1 个重复测试）；`cargo clippy --all-targets` 的 lib 告警 215 → 61，剩余均为既有风格类（`missing_errors_doc`、`float_cmp`、`redundant_closure` 等），`cast_sign_loss` 只剩 3 处测试内断言代码。

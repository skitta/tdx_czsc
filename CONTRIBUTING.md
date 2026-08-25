# Contributing

感谢关注 `tdx_czsc`。

当前仓库仍处于 **原型 / 联调阶段**，在这个阶段，贡献的核心目标不是“功能越多越好”，而是：

- 保证通达信 x64 接入链路稳定
- 保证 Rust `czsc-core` 通道输出正确
- 保证 Windows 联调过程可复现、可排障
- 保证代码结构后续便于扩展

---

## 1. 贡献范围建议

当前最欢迎的贡献方向：

### A. Rust 服务侧
- `analysis.proto` 演进
- `rust_service/` 请求处理
- `czsc-core` 通道映射
- 单元测试 / 离线样例
- 缓存策略优化
- 错误处理与日志增强

### B. Bridge DLL 侧
- x64 Windows 编译修正
- Protobuf / Named Pipe 客户端完善
- 通道回填逻辑优化
- 日志、超时、降级策略增强
- DLL 本地缓存增强

### C. 联调与验证
- Windows 实机验证
- 通达信版本兼容性验证
- 公式模板改进
- 排错文档补充

### D. 文档
- README 完善
- Windows 联调手册完善
- 通道定义说明补充
- 构建和发布流程文档

---

## 2. 不建议的改动方式

在当前阶段，请尽量避免：

1. **大范围重命名目录/模块**，除非能显著提升结构清晰度
2. **一次引入大量外部依赖**，尤其是在 DLL 侧
3. **把复杂业务逻辑堆进通达信公式**
4. **在尚未验证主链路前做过度优化**
5. **未经讨论直接加入与项目目标不相干的功能**

项目当前优先级是：

> **先把“能稳定跑通”做好，再做“更复杂、更优雅”。**

---

## 3. 代码贡献原则

### Rust
- 优先小步提交、结构清晰
- 新增通道时尽量同步补测试
- 避免把协议耦合到算法内部
- 优先保证 `AnalyzeRequest -> snapshot channels` 逻辑清晰可验证

### Rust 代码结构约定

模块职责（`rust_service/src/`，新代码遵循既有划分，不要继续堆进单文件）：

| 模块 | 职责 | 依赖方向 |
|---|---|---|
| `analyzer/mod.rs` | 快照编排（`build_snapshot`）+ `AnalysisParams` | 只依赖子模块 |
| `analyzer/rawbars.rs` | 请求 → RawBar（频率/量能/时间轴） | → error |
| `analyzer/pivots.rs` | 中枢构建（前端/后端优化）+ 三买卖严格化 | → objects（仅下标对齐） |
| `analyzer/segs.rs` | 线段识别（特征序列分型法） | 无 |
| `analyzer/signals.rs` | 401~406 买卖点信号 | → objects（锚点对齐） |
| `analyzer/channels.rs` | func_id → float[] 通道 + 单一映射表 | → objects/segs/signals |
| `analyzer/objects.rs` | K线下标对齐 + proto 对象映射（`TryFrom`） | 无 |
| `analyzer/error.rs` | `AnalysisError`（thiserror） | 无 |
| `handlers/` | 信封分发、请求协调（缓存/单飞）、调试通道 | → analyzer |
| `cache/` `state/` `config/` `ipc/` | 运行期支撑 | 按需 |

- **错误分层**：分析管线返回 `AnalysisError`（`analyzer/error.rs`），协议层
  （`handlers/mod.rs`）映射状态码（INVALID_REQUEST / ANALYZE_FAILED /
  INTERNAL_ERROR）；应用胶水层用 `anyhow` 携带上下文；
- **抽象门槛（规则三）**：至少 2~3 个真实实现才引入 trait；单一实现的
  trait 化视为过度设计。表驱动（const 数组 + 函数指针，如 `SIGNAL_SPECS`、
  通道映射表）优先于 trait；
- **转换用标准 trait**：对象映射走 `TryFrom`（`objects.rs`），转换失败返回
  `AlignError`，调用方按跳过处理；
- **质量门**：提交前 `cargo clippy --all-targets -- -D warnings` 与
  `cargo fmt -- --check` 必须通过（本机无 crates 网络时加 `--offline`）。

### C++ / DLL
- 优先保证宿主安全
- 所有失败路径都应安全降级
- 避免异常穿透 DLL 边界
- 对 Windows 特有行为保持保守设计

### 文档
- 路径、命令、前置条件尽量写全
- 对联调/排错类文档，优先写“现象 + 解决步骤”

---

## 4. 测试建议

如果你修改的是：

### Rust 服务
优先至少验证：

- `cargo check`
- 新增/已有离线测试样例
- 协议生成链是否正常

### Bridge DLL
优先至少验证：

- Windows x64 下能编译
- `tdx_czsc_ipc_smoke_test.exe` 能跑通
- `logs/tdx_czsc_bridge.log` 有合理输出

### 通达信公式
优先至少验证：

- `debug_ready.tn6`
- `debug_latency.tn6`
- `fx_marks.tn6`
- `bi_points.tn6`
- `zs_band.tn6`

---

## 5. 提交问题时建议包含的信息

如果你要提交 bug/问题说明，建议尽量提供：

1. 操作系统与版本
2. 通达信版本（明确 x64）
3. Rust / CMake / MSVC / Protobuf 版本
4. 触发问题的步骤
5. 控制台输出
6. `logs/tdx_czsc_bridge.log` 关键内容
7. 是否能通过 `ipc_smoke_test.exe`
8. 使用了哪个公式模板

这样能显著提高排障效率。

---

## 6. 分支与提交建议

如果你在自己的分支上开发，建议：

- 一个改动点一个主题
- 协议修改与实现修改尽量配套
- 不要把无关格式化、重命名、功能修改混在一起

推荐提交主题示例：

- `rust_service: add analyzer tests for zs channels`
- `bridge_dll: implement named pipe response parsing`
- `docs: add windows troubleshooting checklist`

---

## 7. License 说明

当前仓库采用：

- **MIT License**

许可证文件见仓库根目录：

- `LICENSE`

这意味着仓库整体采取较宽松的开源授权方式，便于传播、试用和二次开发；但请保留许可证与版权声明。

---

## 8. 当前最有价值的贡献

如果你想快速做出高价值贡献，建议优先从以下事项中选：

1. 在 Windows x64 上真实跑通 `rust_service`
2. 在 Windows x64 上编译通过 `bridge_dll`
3. 跑通 `tdx_czsc_ipc_smoke_test.exe`
4. 验证 `debug_ready.tn6` / `fx_marks.tn6` / `zs_band.tn6`
5. 补充 Windows 联调中的真实错误样本与修复记录

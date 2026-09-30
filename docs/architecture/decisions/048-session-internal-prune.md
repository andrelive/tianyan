# ADR-048: 会话内 prune（老工具输出降级）

**日期**: 2026-09-30
**状态**: ✅ 已采纳（已实施）
**影响范围**: context（新增 `core/src/context/prune.rs`；`ContextPipeline` 携带
`PruneConfig`）、config（`[tool_output]` 新增 4 个 `prune_*` 字段）、agent
（`agent_core::assemble_context` 应用 + `builder` 映射配置）

---

## 背景

1. **问题**：ADR-047 解决了"**单次**结果过大"（落盘 + 预览回会话）。但工具结果
   进入会话链后**长期占位**——即便其结论早已被后续 assistant 消息消化，原文仍每轮
   随请求发送，直到触发**昂贵且更有损**的 LLM 摘要压缩。
2. **参照实现**（opencode `session/compaction.ts`，2026-09-30 核实）：
   - 保留最近 `PRUNE_PROTECT = 40_000` token 的工具输出；
   - 更老的截到 `TOOL_OUTPUT_MAX_CHARS = 2000` 字符 + `[truncated]`；
   - **仅当修剪量 > `PRUNE_MINIMUM = 20_000` 才动手**（避免频繁扰动前缀）；
   - `skill` 输出受保护（`PRUNE_PROTECTED_TOOLS`）。
3. **定位**：prune 是"**压缩的廉价前置**"——零 LLM 成本回收空间；若 prune 后不再
   超阈值，就不必调用 LLM 摘要。与 ADR-047 的分工明确：
   - ① spill（047）管"单次过大"→ 落盘 + 路径（**可回读**完整内容）；
   - ② prune（048）管"老结果长期占位"→ 就地降级（原文仍在存储里，**需要时重新
     调用工具**）。

## 决策

### 1. 落点：组装视图（不改存储、不耦合压缩）

`context/prune.rs` 提供纯函数
`prune_tool_outputs(chain, cfg) -> Option<Vec<StructuredMessage>>`，在
`agent_core::assemble_context` 中于 `ContextAssembler::assemble` **之前**应用。

- 与 ADR-027 一致：**存储层永远返回完整链**，裁剪只发生在组装视图；
- 与 ADR-043（`normalize_tool_pairs`）**同层同纪律**：只改请求视图，库与缓存不动；
- 无需改动时返回 `None`，调用方经 `prune_or_borrow` 走**借用的原链**（零克隆、
  零改写）——常态零成本。

### 2. 算法

从链尾向前累计工具输出的估算 token（`common::token_estimator::estimate_tokens`）：

- 累计未超 `protect_tokens` 的（即**较近**的）→ 完整保留；
- 超出之后的（**较老**的）→ 裁到 `max_chars` 字符 + `PRUNE_MARKER`；
- 受保护工具（`call_skill`：技能方法论可能被反复参考，裁掉直接导致行为退化）跳过；
- 总节省 < `min_tokens` → **不改写**（返回 `None`）。

裁剪采用**惰性克隆**：只有真正被裁的消息才产生新对象；其余消息原样借用。

### 3. 配置（`[tool_output]` 节，与 ADR-047 同节）

| 字段 | 默认 | 对齐 |
|---|---|---|
| `prune_enabled` | `true` | — |
| `prune_protect_tokens` | `40_000` | opencode `PRUNE_PROTECT` |
| `prune_min_tokens` | `20_000` | opencode `PRUNE_MINIMUM` |
| `prune_max_chars` | `2000` | opencode `TOOL_OUTPUT_MAX_CHARS` |

**配置 → 引擎参数的映射点在 `agent::builder`**（`context` 模块不依赖 `config`：
引擎层保持"无配置依赖"的边界，与既有 `CompressionConfig` 同风格）。

### 4. 幂等与确定性

- **幂等**：已含 `PRUNE_MARKER`、或长度已 ≤ `max_chars` 的内容不再裁 →
  重复运行结果稳定（不叠加标记）；
- **确定性**：判定只依赖链内容 + 配置（无时间/随机）→ 同一链产出同一视图 →
  前缀缓存可预测（这是它能放在组装层的前提）。

### 5. 前缀缓存代价（明示，不隐藏）

裁剪会改变历史消息内容 → **跨过受保护边界的那一轮**前缀变化（一次缓存未命中）。
`min_tokens` 阈值即为此而设：收益不足不动手。这是"用一次未命中换长期预算回收"
的取舍——与 opencode 同款纪律。

## 边界（明确不做）

- ❌ **不改存储**：原文保留在 `session_messages`（与 ADR-047 的落盘文件）里；
- ❌ **不与压缩耦合**：prune 是纯文本、零 LLM；压缩是摘要、有 LLM——各自独立可配
  （prune 先回收，压缩是后续兜底）；
- ❌ **不裁非工具消息**：user / assistant 文本永不动；
- ❌ **不提供"老内容回读"**：需要细节时重新调用工具（工具仍在）。

## 影响

- 长会话的工具输出占用显著下降（老输出从"原始大小"降到 ≤ `max_chars` 字符/条）；
- 压缩触发推迟（prune 后可能已不再超阈值 → 免一次 LLM 摘要）；
- 前缀缓存：跨边界轮一次未命中（由 `min_tokens` 控制频率）；
- 存量配置零迁移（新字段全 `#[serde(default)]`，默认启用）。

## 实施记录（2026-09-30）

| 层 | 文件 | 内容 |
|---|---|---|
| 引擎 | `core/src/context/prune.rs`（新） | `PruneConfig`（含 `disabled()`）+ `prune_tool_outputs`（惰性克隆 / 幂等 / 收益阈值 / 受保护工具）+ `prune_or_borrow`（借用包装）+ `tool_output_tokens`（诊断）+ `PRUNE_MARKER` / `PROTECTED_TOOLS` |
| 引擎 | `core/src/context/mod.rs` | `pub mod prune` + `pub use prune::{prune_or_borrow, prune_tool_outputs, PruneConfig}` |
| 管线 | `core/src/context/pipeline.rs` | `ContextPipeline` 增 `prune: PruneConfig` 字段 + `with_prune_config()` + `prune_config()` |
| 组装 | `core/src/agent/agent_core.rs` | `assemble_context`：assemble 前应用 `prune_or_borrow`（组装视图，先于工具对规范化） |
| 装配 | `core/src/agent/builder.rs` | `[tool_output]` 的 `prune_*` → `PruneConfig`（配置→引擎映射单点） |
| 配置 | `core/src/config/tool_output.rs` | 4 个 `prune_*` 字段 + 默认函数（对齐 opencode 三常量）+ 默认值测试 |
| 测试 | `core/src/context/prune.rs`（内联 9 项） | 裁老保新、收益不足不动、保护额度内不动、幂等、受保护工具（`call_skill`）、非工具消息、关闭/借用路径、无工具结果链、空链健壮性 |

**验证**：`cargo fmt --all` 干净；`cargo clippy --workspace --all-targets`
**0 error / 0 warning**；`cargo test -p tianyan-core --lib` 全绿；
`cargo test -p tianyan-server` 全绿；`.\scripts\test.ps1 lint` 统一门禁通过。

**判别力**：注入"保护额度失效（判定恒真）"→ `test_prunes_old_output_keeps_recent_intact`
与 `test_all_within_protect_budget_untouched` 立即变红（失败信息"受保护区内（最近）的
输出必须完整"），还原后复绿——证明测试锚在"最近输出完整"，而非"有东西被裁"。

## 验收

- 判别力：见上（注入式，2 测变红后复绿）；
- 全量：core / server 测试 + clippy + `scripts/test.ps1 lint`。

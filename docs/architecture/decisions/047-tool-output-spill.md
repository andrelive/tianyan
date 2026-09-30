# ADR-047: 工具输出统一 spill（超预算落盘 + 预览回会话）

**日期**: 2026-09-30
**状态**: ✅ 已采纳（已实施）
**影响范围**: 配置（`core/src/config/tool_output.rs` 新增 `[tool_output]` 节 +
`config/mod.rs` 挂载）、截断层（`core/src/executor/truncate.rs` 阈值参数化）、
工具管线（`core/src/agent/tool_registry/spill.rs` 新增监听器 + `mod.rs` 注册与
注入链 + `agent/builder.rs`）、server（`server/src/agent_builder.rs` 注入目录与阈值）

---

## 背景

1. **问题：工具输出一旦返回，就永久留在会话历史里。** tool result 被写入
   `session_messages` 后，**从那一轮起每轮请求都会重复携带**（直到被压缩掉）。
   一次 50KB 的 grep / repo_map / 子代理结果不只是"一次输入"，而是持续推高
   后续每一轮的输入，并**提前触发有损且不可逆的压缩**。
2. **天演现状：只有 `execute_command` 做了落盘。** `executor/command.rs` 的
   `log_file` 机制（同步命令输出超 50KB / 2000 行 → 完整输出写
   `{log_dir}/exec-{uuid8}.log` + 尾部截断 + 路径指引）覆盖命令输出；**其余工具
   的超大输出直接钉进历史**，没有任何兜底。
3. **能力早已预留、从未接线**：
   - `executor/truncate.rs` 的 `truncate_spill`（全文写 `{spill_dir}/{tag}-{unix_ms}.txt`
     + 标记追加 `全文已保存至 {path}`）自 ADR-009 就作为"配套能力"存在，
     **全库零调用方**（除单测）；
   - `agent/tool_registry/pipeline.rs` 的 post-execute 文档已明写其用途：
     "可观察结果（审计）或改写结果（**如给大输出挂 spill locator**）"。
4. **参照实现（2026-09-30 核实，非猜测）**：
   - **opencode**：`tool/truncate.ts` 提供通用 `Truncate.output`，由 `tool/tool.ts`
     在**所有工具的出口统一调用**——超 2000 行 / 50KB → 全文写
     `{Global.Path.data}/tool-output/tool_*`，返回预览 + `outputPath` + 读取指引
     （有 task 工具时升级为"委托子代理处理该文件，不要自己读全文"）；保留 7 天。
     此外会话内还有一层 prune（`session/compaction.ts`：老工具输出压到 2000 字符，
     保留最近 40k token 的工具输出，`skill` 受保护）。
   - **DSH**：`spill-policy` 包挂 `tools/post-execute` 钩子（作用于**所有工具**，
     `read` 除外）——按 **token** 预算 `maxInlineTokens` 判定，超限则"头尾保留 +
     中间挖空（`[...]`）+ spill notice"，完整内容经 `dsh-spill` 的 `SpillRef
     { locator, bytes, retrievalHint }` 落盘。
   - **共同点**：落点都在**工具输出的统一出口**，而不是逐个工具内部；
     `read` 类（模型主动选的精确内容）被显式排除。
5. **本机制的意义（明确边界）**：它**不是**"把一次大输入拆成多次小输入"
   （真需要全量时反而更贵——要多一次发现→判断→读取的往返）。它是**按需加载**：
   让不该进历史的东西根本不进历史——常态（绝大多数工具输出远低于预算）零成本，
   只有病态案例（超大输出）才落盘，而那部分内容本来就不该占用上下文预算。
   附带收益：不再静默截断（无损可回溯）、模型知道"还有多少没看到"（决策更准）、
   压缩触发更晚（有损损失更小）。

## 决策

### 1. 单点：post-execute 监听器（不逐工具打补丁）

新增 `ToolOutputSpillListener`（`agent/tool_registry/spill.rs`），在
`ToolRegistry::new()` 中注册为**第二个** post-execute 监听器——
**必须在 `ToolObservabilityListener` 之后**：观测监听器要把**原始结果**写进
Trace / GEPA 数据层（`ExecutionHistory.result = format!("{v}")`），spill 是改写者，
排最后才能保证数据层不失真。

所有工具自动受益，未来新增工具零额外工作——与 opencode 的出口统一、
DSH 的全局钩子同构，符合"已有链路不叠加抽象"。

### 2. 预算：字节 + 行数双上限，可配

新增顶层配置节 `[tool_output]`（`core/src/config/tool_output.rs`）：

| 字段 | 默认 | 说明 |
|---|---|---|
| `enabled` | `true` | 总开关（false = 完全旁路） |
| `max_lines` | `2000` | 单次结果行数上限 |
| `max_bytes` | `51200` | 单次结果字节上限（50 KiB） |

默认值即 `executor/truncate.rs` 的既有常量（`MAX_LINES` / `MAX_BYTES`），
对齐 opencode。为让配置生效，`truncate.rs` 的阈值**参数化**：
`truncate_spill_with(text, dir, tag, max_lines, max_bytes)`，原 `truncate_spill`
委托之（保持既有语义与单测）。

未注入目录（`with_tool_output` 未调用）= 不落盘——与
`command_logs_dir: Option<PathBuf>` 同一模式（未配置 = 仅内存行为，不报错）。

### 3. 白名单跳过（可重取 / 已有机制）

| 工具 | 理由 |
|---|---|
| `read_file` / `vfs_read` | 模型**主动选择**的精确内容（DSH 同款排除）；且文件/VFS 本身是事实源，随时可重读 |
| `list_dir` / `vfs_list` / `glob` | 目录列举，可随时重取，且通常很小 |
| `execute_command` | 已有 `log_file` 落盘机制（`command.rs`），避免双重落盘 |

### 4. 改写形态

超预算 → 完整序列化内容落盘（复用 `truncate_spill_with`），结果对象替换为：

```json
{
  "spilled": true,
  "spill_path": "<完整内容路径>",
  "spill_bytes": 123456,
  "spill_lines": 3210,
  "note": "工具输出过大已落盘（完整内容 123456 字节）。用 read_file 的 offset/limit 分页读取该文件，或委托子代理（delegate_to_agent）处理，勿凭预览下结论。",
  "preview": "<头部截断预览 + 「全文已保存至 …」标记>"
}
```

- `preview` 由 `truncate_spill_with` 产出的 `Truncated.text` 提供（头部截断 +
  统一截断标记，复用既有形态）；
- 结构化包装（而非裸文本）让模型能：① 知道发生了什么（`spilled`）；
  ② 拿到路径去分页读；③ 看到总量（`spill_bytes` / `spill_lines`）判断是否值得读。

### 5. 失败降级（best effort）

落盘失败（目录不可写 / IO 错误）→ `tracing::warn!` + **保持原结果不变**，
不阻塞工具调用、不新增 `TianyanError` 变体——对齐 `command.rs`
"落盘 best effort，结果不受影响"的既有语义与 ADR-014 错误分类纪律。

### 6. 目录

`{data_dir}/tool_output/`（与 `command_logs` / `task_results` 同级同模式，
由 `server/src/agent_builder.rs` 注入）。

**首期不做 TTL 清理**——与 `command_logs` 现状一致（两者都不清理），
避免引入第二套 GC 机制；后续可并入既有 GC 任务统一治理（另案）。

## 边界（明确不做）

- ❌ **不新建存储抽象 / trait / Manager**——复用 `truncate_spill` + 目录约定，
  符合 VFS 约束条目"在 VFS 之外引入新的存储抽象"的禁令精神；
- ❌ **不给单个工具打补丁**（不在工具内写落盘）——统一出口已经覆盖；
- ❌ **不改存储层**（不裁剪历史消息）——② 会话内 prune 属**组装层视图**
  （ADR-027：存储永远返回完整链，截断只发生在组装视图），另案处理；
- ❌ **不动 `execute_command` 的既有落盘**（带 background 日志语义，先并存）。

### grep 工具层的评估结论（不做，2026-09-30）

原计划再给 grep 补一层"截断时全量落盘"（对齐 DSH 的 grep 内联 250 条 + 溢出落盘）。
评估后**不做**，理由：

1. **成本问题已由统一层覆盖**：grep 早有 `head_limit` 收窄（默认 100 / 上限 200，
   见输出治理 `62fa0f6`）——返回体积与 opencode（100 条）／DSH（250 条）同量级，
   通常 10~20KB，本就在统一层 50KB 预算之内；极端场景（模型把 head_limit 调到 200
   且命中行都超长）由统一层兜住。
2. **已有更轻的续查契约**：`offset` 分页是 grep 既有能力（模型可续查），
   比"落盘 + read_file"少一次落盘与一次往返。
3. **不给单个工具打补丁**：为 grep 单独把 spill 目录穿进
   `SearchOptions → search_engine`，会让"统一出口"原则出现例外，
   收益（罕见场景少一次分页）不抵结构成本。

重新评估的触发条件：实测 grep 结果频繁超预算，且模型确实需要全量（而非收窄搜索）。

## 影响

- **常态零变化**：未超预算的工具调用只多一次序列化测长（可忽略）。
- **病态案例**：超大输出不再钉进历史，改为"预览 + 可回读路径"。
- **工具目录不变**：未改任何工具描述 / schema——`tool-catalog.md` 无漂移
  （无需重生成）。
- **`truncate_spill` 由死能力变活链路**（ADR-009 配套能力完成接线）。
- **存量配置零迁移**：新节全 `#[serde(default)]`，旧配置行为不变（默认启用）。

## 实施记录（2026-09-30）

| 层 | 文件 | 内容 |
|---|---|---|
| 配置 | `core/src/config/tool_output.rs`（新） | `ToolOutputConfig { enabled, max_lines, max_bytes }` + 默认（2000 / 50 KiB）+ serde 缺省兼容 + 与截断层常量的一致性守卫测试 |
| 配置 | `core/src/config/mod.rs` | `mod tool_output` + `pub use` + `TianyanConfig.tool_output`（`#[serde(default)]`） |
| 截断 | `core/src/executor/truncate.rs` | `truncate_spill_with(text, dir, tag, max_lines, max_bytes)`（阈值参数化；**未超阈值不写文件**）；`truncate_spill` 委托默认常量；`sanitize_tag` 文件名净化；`truncate_head_with_marker_limited` 拆分 |
| 管线 | `core/src/agent/tool_registry/spill.rs`（新） | `ToolOutputSpillListener`：预算检查 → `truncate_spill_with` → 结果替换为 `{spilled, spill_path, spill_bytes, spill_lines, note, preview}`；`SKIP_TOOLS` 白名单；best-effort 降级；未配置/未启用旁路 |
| 管线 | `core/src/agent/tool_registry/mod.rs` | 字段 + `new()` 注册为**第二个** post-execute 监听器（观测之后）+ `with_tool_output(dir, &config)` |
| Agent | `core/src/agent/builder.rs` | `with_tool_output(dir, config)` + build 注入 |
| Server | `server/src/agent_builder.rs` | 注入 `{data_dir}/tool_output` + `config.tool_output` |
| 测试 | `core/src/agent/tool_registry/spill_tests.rs`（新，10 项） | 超限替换 + **完整原文**落盘、预算内原样、白名单（`read_file` / `execute_command`）、错误结果不改写、`enabled=false`、未配置旁路、落盘失败降级、注册顺序、阈值下限钳制 |
| 文档 | 本 ADR / `AGENTS.md` / `config.example.toml` | ADR 索引 + 配置示例 |

**验证**：`cargo fmt --all` 干净；`cargo clippy --workspace --all-targets`
**0 error / 0 warning**；`cargo test -p tianyan-core --lib` **1483 passed / 0 failed**
（含新增 13 项）；`cargo test -p tianyan-server` 全 target 通过（180 / 5 / 13 / 1 / 2，0 failed）。

**判别力**：注入"落盘写空文件"后 `test_oversized_result_spills_and_replaces` 与既有
`spill_writes_full_text_and_sets_path` 立即变红（失败信息"落盘文件应含被预览截掉的
尾部内容（完整原文）"），还原后复绿——证明测试锚在"落盘的是完整原文"而非"有个文件"。

## 验收

- **判别力**（注入式，逐项断言）：
  1. 超限结果 → 被替换为 `spilled` 对象 + 目标文件存在 + 文件内容为**完整**原文；
  2. 白名单工具（`read_file` 等）超限**不**落盘；
  3. 未超限结果**原样返回**（零改写）；
  4. 落盘目录不可写 → 结果**保持原样**（降级不报错）；
  5. 注册顺序：observability 先于 spill（数据层看到原始结果）。
- **全量**：`cargo fmt --all --check` / `cargo clippy --workspace` /
  `cargo test -p tianyan-core --lib` / `cargo test -p tianyan-server` +
  `.\scripts\test.ps1 lint` 统一门禁。

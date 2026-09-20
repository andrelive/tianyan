# ADR-009: 语义化编辑双原语 —— 内容匹配 + unified diff 信封

> **2026-08 修订**：`apply_edit` 由早期 hashline 行锚点方案改为**内容匹配**（old_string/new_string，对齐 DSH / Claude Code），解决行号漂移导致的反复失败与 read_file 哈希前缀的输入 token 开销。文件名保留旧名以稳定链接。

**日期**: 2026-08
**状态**: ✅ 已采纳
**影响范围**: 执行 — `executor` 模块（`core/src/executor/`）+ 工具注册

---

## 背景

编程助手差距评估（`docs/archive/coding-assistant-gap-analysis.md`，已归档）识别出 G1（语义化编辑）与 G2（diff 生成/解析/应用）两项基础差距：`write_file` 仅支持整文件覆盖写入（大文件重写 Token 爆炸且易出错），全 workspace 无 diff 相关代码。选型研究（D1/D2）对比两条原语路线后确定双原语并存：

- **old_string/new_string 精确替换**（Claude Code、opencode `edit`、Continue `multiEdit`）—— 单文件小改
- **unified diff freeform**（Aider `udiff`、Codex / opencode `apply_patch`）—— 多文件大改 + move/delete

业界关键共识（详见 gap-analysis §4.1/§4.2）：

1. 失败必须显式回喂模型（Aider `reflected_message`、Codex `RespondToModel`），指导模型"提供更多上下文"或改用 `replace_all`
2. **不做无锚点的纯 fuzzy**：Aider 的 edit-distance fuzzy 已在源码中禁用（`return` 短路），Continue 的 Jaro-Winkler 因坐标映射 bug 被注释；高相似度匹配容易改错位置
3. 应用前全量验证（Codex `verify_apply_patch_args`）：解析 → 读文件 → 计算 → 确认差异非空 → 才写盘
4. 批量原子性：Continue `multiEdit`（同文件多编辑一次调用、失败全回滚）+ Codex patch 信封（多文件）
5. diff 应用层全部自研（Codex/opencode/Continue/Aider 均如此），GNU patch 式严格匹配对 LLM 输出失败率不可接受

## 决策

**语义化编辑采用双原语**：`apply_patch`（unified diff 信封 + 上下文锚定）为**主力编辑工具**（对齐 Aider/Codex/opencode）；`apply_edit`（内容匹配 old_string/new_string）仅保留给能精确复现原文的极小改动。二者作为 `write_file` 整文件覆盖的补充。`apply_edit` 采用内容匹配，取代早期 hashline 行锚点方案。

1. **内容匹配**（`executor/edit.rs`）：`apply_edit` 以 `ContentEdit { old_string, new_string, replace_all }` 在文件内容中查找 `old_string`（须唯一，多处出现需 `replace_all`）替换为 `new_string`。内容匹配天然免疫行号漂移；`read_file` 输出纯内容（无行号/哈希前缀），省输入 token。`old_string` 未找到/不唯一即拒绝（409 conflict），提示重新 read_file。
2. **不做无锚点的纯 fuzzy**：`apply_edit` 用唯一 `old_string` 精确匹配（不做 fuzzy，避免误改位置）；`apply_patch` 仅以 similar 相似度（`FUZZY_RATIO_THRESHOLD = 0.75`）做 hunk 头模糊定位（fuzzy seek），命中后逐行内容仍需匹配。不做全局模糊替换。
3. **批量原子**：`apply_edits_to_content` 纯函数自底向上应用（bottom-up，行号从大到小，避免前序编辑破坏后续行号），全部校验通过才写盘；`apply_patch_action` 多文件批量，任一文件失败整体回滚。
4. **审批门控**：`Action` 新增 `ApplyEdit` / `ApplyPatch` 变体（`executor/types.rs`），风险分级 Medium，走既有 `ApprovalWorkflow`。
5. **配套能力**：`executor/truncate.rs` 统一截断层（`MAX_LINES = 2000` / `MAX_BYTES = 50KB`，保头/保尾/落盘 spill）；`executor/fs.rs` 文件浏览（`execute_glob` / `execute_list_dir`，`MAX_GLOB_RESULTS = 200`）；read_file 增强（offset/limit、截断消息、二进制嗅探、目录模式）。
6. **写入目录语义**（2026-09 补）：`write_file` 新增 `create_dirs` 参数（默认 `false`）——父目录不存在时**默认报错**并回喂修法指引（"如确需新建目录，请重新调用并传 `create_dirs=true`"），**不自动创建**；显式 `true` 时在 `write_file_atomic` 内 `create_dir_all`。理由：路径写错（本该写已有目录却拼了新目录名）时宁可失败，也不静默"凭空多出一棵树"——目录创建是**调用方的显式声明**，不是隐式副作用。`apply_edit`（语义要求文件已存在）与 `apply_patch`（目标位于既有工程结构内）不暴露该开关，一律传 `false`。

diff 库选型（D2）：引入 `similar`（3.1.2，零依赖）用于 unified diff 生成与 patch 模糊定位；patch 解析/应用为自研薄层（`parse_patch`，参考 codex apply-patch 模式）。

## 后果

### 正面
- 大文件局部修改不再整文件重写，Token 成本与出错面大幅下降
- 双原语覆盖小改（apply_edit）与大改/多文件（apply_patch）场景，与业界工具能力对齐
- 内容匹配免疫行号漂移，模型编辑不再因行号漂移反复失败；read_file 无哈希前缀，输入 token 更省

### 负面 / 代价
- 双原语增加模型工具选择负担，需在工具描述中明确适用边界
- similar 模糊定位（0.75 阈值）存在误定位风险，依靠命中后逐行内容校验兜底
- 自研 patch 解析器需持续维护对 LLM 生成 patch 的容错

### 边界条件（违反即重新评估）
- 禁止引入无锚点的纯 fuzzy 全局替换（坐标回射不可靠，业界已多次踩坑）
- `write_file` 保留整文件覆盖语义，不替换为 patch 拼装
- 编辑类工具一律 Medium 审批，不得降级

## 关键文件

- `core/src/executor/truncate.rs` — `truncate_head` / `truncate_tail` / `truncate_spill`、`Truncated`
- `core/src/executor/mod.rs` — `write_file_atomic`（原子写 + `create_dirs`：缺目录默认不建）
- `core/src/agent/tool_registry/file_ops.rs` — 工具层目录校验（缺目录 → `not_found` + 修法指引）
- `core/src/executor/edit.rs` — `ContentEdit` / `apply_edits_to_content` / `apply_edit_action` / `detect_eol`（CRLF 保留）
- `core/src/executor/patch.rs` — `parse_patch` / `apply_patch_to_content` / `apply_patch_action`、`FUZZY_RATIO_THRESHOLD`
- `core/src/executor/fs.rs` — `execute_glob` / `execute_list_dir`
- `core/src/executor/types.rs` — `Action::ApplyEdit` / `Action::ApplyPatch`
- `core/src/agent/tool_registry/mod.rs` — `register_builtin_tools()`（apply_edit / apply_patch / glob / list_dir 注册）


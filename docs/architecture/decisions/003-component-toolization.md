# ADR-003: 组件工具化

**日期**: 2026-06  
**状态**: ✅ 已采纳  
**影响范围**: Agent Loop 工具调用、技能桥接

---

## 背景

知识库查询、技能调用、文件操作等能力需要被 LLM 自主调用。若硬编码在 Agent Loop 中，扩展性差且无法利用 LLM 的推理能力选择合适的工具。

## 决策

将所有能力封装为 OpenAI function calling 兼容的工具，由 LLM 通过 `tool_call` 自主调用。

## 当前工具清单

`ToolRegistry` 注册以下工具：

| 工具 | 功能 |
|------|------|
| `read_file` | 读取文件 |
| `write_file` | 写入文件 |
| `execute_command` | 执行系统命令 |
| `search_code` | 代码搜索 |
| `search_vfs` | VFS 语义检索（文档/记忆/规则/技能） |
| `vfs_read` | VFS 读取 |
| `vfs_list` | VFS 列表 |
| `call_skill` | 技能调用（桥接到 `SkillExecutor`） |
| `run_tests` | 运行测试 |
| `verify_build` | 验证构建 |
| `ask_user` | 向用户提问 |
| `self_check` | 自我检查 |
| `delegate_to_agent` | 委托给子 Agent |

## 设计原则

- **工具不做自主多轮决策**：LLM 每次响应要么给出最终答案，要么请求 tool_calls。工具只是执行者 + 信使。
- `delegate_to_agent` 采用有界循环（默认 5 轮）防止失控。
- `call_skill` 桥接到 `SkillExecutor`：参数验证 + 安全检查 + 超时控制。

## 后果

- Agent → skills 形成单向依赖（`agent/tool_registry.rs` → `skills::SkillExecutor`）
- 新能力通过添加工具而非修改 Agent Loop 核心逻辑
- 工具可并行执行（`ToolRegistry::execute_parallel()` 通过 tokio JoinSet）

## 关键文件

- `core/src/agent/tool_registry.rs` — 工具注册 + 并行执行
- `core/src/skills/executor.rs` — `SkillExecutor`（call_skill 桥接目标）

## 后续演进

- **技能执行语义移除**：技能回归纯方法论文档（VFS `skill/` 命名空间）。
  6 个桥接技能（file_read/file_write/file_delete/file_list/system_command/http_request）
  与 `SkillExecutor`/`SkillHandler`/`SkillRegistry` 全部删除——能力由内置工具直接覆盖；
  `call_skill` 改为读 VFS 技能文档（L0 摘要 + L2 详情）返回，由 LLM 参考后自行执行；
  planning 预置进 VFS（bootstrap 写入），与 GEPA 学习技能同构。

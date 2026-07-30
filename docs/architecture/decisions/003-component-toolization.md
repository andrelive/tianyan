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
| `search_knowledge` | 知识库检索 |
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

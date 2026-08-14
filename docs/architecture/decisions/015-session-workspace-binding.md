# ADR-015: 会话工作区绑定 —— 工作区是会话的父级分组

**日期**: 2026-08-15
**状态**: ✅ 已采纳
**影响范围**: 会话模型（`session/types.rs`）、快照体系（`snapshot/mod.rs`）、Agent 快照捕获（`agent/agent_core.rs`）、会话/工作区 API（`server/src/api/{sessions,workspace,chat}/`）、前端侧边栏与工作区面板（`gui-vite/src/components/{sidebar,workspace}/`）

---

## 背景

工作区（`working_directory`）最初的形态是**全局配置**（`[agent] working_directory`）：所有会话共享同一个工作目录，Agent 的 `execute_command` / 文件工具与快照回退都以它为根；GUI 提供一个"工作区面板"做文件浏览（树/读/diff/apply-patch）。

对照主流智能体（Claude Code 的文件夹、Cursor 的 Projects、Codex 的 workspaces），工作区的实际语义是**会话的父级分组**：一组相关会话的上下文容器（项目上下文、文件边界、回退快照），切换工作区 = 切换项目上下文。两个结构性偏差：

1. **会话不归属工作区**：`SessionHeader` 无工作区字段，会话与工作区无绑定关系——用户无法表达"这个会话在做项目 A、那个会话在做项目 B"；
2. **文件浏览 UI 定位错误**：Agent 操作文件的能力在工具层（read/write/exec），格式兼容由工具层解决；用户浏览面板的实际价值是**审计 Agent 改了什么**（diff/回退），而非管理文件。

## 决策

### 1. 会话级工作区字段（绑定关系显式化）

`SessionHeader.working_directory: Option<String>`（JSONL 首行持久化，serde 缺省 None，旧会话双向兼容）。`Session::working_directory(fallback)` 解析生效目录：**会话级绑定优先（目录必须存在），缺省回退全局配置**。

- 新建会话：`ChatRequest.working_directory`（仅新会话生效）→ 固化到会话头部；
- 修改会话：`PUT /sessions/{id}/workspace`（空串清除绑定；目录必须存在）；
- 会话列表/详情 API 暴露 `working_directory`，前端侧边栏**按工作区分组**（未绑定归入"默认工作区"组）。

### 2. 快照根随会话解析

`SnapshotManager` 增加 `with_workdir`（root/excludes 共享、workdir 替换的轻量克隆）；所有快照入口（capture/restore/save_redo/load_redo/diff）由调用方先解析会话生效工作目录再绑定：

- Agent 轮前捕获：`resolve_working_directory(session_id)`（会话绑定 → 全局默认）；
- 回退/重做（`DELETE /sessions/{id}/messages/delete`、redo）：同解析规则；
- 快照树/对象库仍按 session_id 组织，内容寻址对象全局去重（跨工作目录共享）。

### 3. 工作区 API 会话化

`tree/read/diff/apply-patch/apply-edit` 接受可选 `session_id`：有会话 → 解析会话绑定目录（不存在 → 400）；无会话 → 全局配置兜底（向后兼容）。工作区面板转型为**当前会话工作区的审计视图**（树 + diff + 回退），无会话时显示全局工作区。

### 4. 前端交互

- 新建对话 → 先打开**目录选择器**（`GET /workspace/dirs` 逐级浏览真实目录，不手动输入），可选"不绑定工作区"；
- 侧边栏会话列表按工作区分组（工作区头 + 会话条目）；
- 工作区面板"选择目录"绑定到**当前会话**（无会话时作为下一新对话的待绑定工作区）。

## 后果

### 正面
- 会话归属清晰：侧边栏分组即项目边界，切会话 = 切上下文，与主流智能体心智一致
- 快照回退与工作区归属解耦：不同项目会话各自快照各自目录，互不污染
- 工作区面板定位收敛为审计视图，避免"通用文件浏览器"的过度工程
- 全局配置退化为默认兜底，旧部署零迁移

### 负面 / 代价
- 会话头部新增字段（JSONL 格式向后兼容，旧会话自动 None）
- 快照调用方需解析工作目录（每轮一次会话读取，可接受——快照本身全量遍历更重）
- 工作区 API 增加 session_id 参数（缺省行为不变，向后兼容）

### 边界条件（违反即重新评估）
- 会话绑定目录被删除 → Agent 捕获静默回退全局默认；工作区 API 明确 400（审计视图必须暴露问题）
- 绑定目录校验只做存在性检查（不强制绝对路径语义；调用方保证绝对路径）

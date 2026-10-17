# HTTP API 端点

天演本地服务的完整 REST API 端点列表（从 README 迁移，2026-10-16）。

全部业务接口挂载于 `/api/v1` 前缀下；服务默认监听 `127.0.0.1:3000`
（`TIANYAN_PORT` 可覆盖端口；独立启动不解析 CLI 参数）。
接口变更（新增 / 删除路由）时请同步更新本清单。

| 端点 | 方法 | 说明 |
|------|------|------|
| `/health` | GET | 健康检查 |
| `/api/v1/chat/stream` | POST | 流式对话（增量事件经 `GET /events` 下发） |
| `/api/v1/chat/streams/{session_id}/cancel` | POST | 显式停止对话流（断线不取消，仅主动停止） |
| `/api/v1/chat/answer` | POST | 追问回答提交（`ask_user` 工具的回答入口） |
| `/api/v1/sessions` | GET | 会话列表 |
| `/api/v1/sessions/{id}` | GET / DELETE | 获取 / 删除会话 |
| `/api/v1/sessions/{id}/messages` | GET | 会话消息列表 |
| `/api/v1/sessions/{id}/messages/delete` | POST | 删除指定消息 |
| `/api/v1/sessions/{id}/messages/redo` | POST | 重做指定消息 |
| `/api/v1/sessions/{id}/title` | POST | 更新会话标题 |
| `/api/v1/sessions/{id}/workspace` | PUT | 更新会话工作目录 |
| `/api/v1/sessions/{id}/compress` | POST | 手动压缩会话（压缩点同步刷新 learned rules） |
| `/api/v1/knowledge/ingest` | POST | 文档导入（multipart） |
| `/api/v1/knowledge/search` | GET | 知识搜索 |
| `/api/v1/knowledge/search/suggestions` | GET | 搜索建议 |
| `/api/v1/knowledge/entries` | GET | 知识条目列表 |
| `/api/v1/knowledge/entries/read` | GET | 读取知识条目内容 |
| `/api/v1/knowledge/entries/delete` | POST | 删除知识条目（递归子条目 + 同步清理向量索引） |
| `/api/v1/memory` | GET | 记忆列表（三源聚合：`memory/` + `user/` + `agent/patterns`，系统子路径排除） |
| `/api/v1/stats` | GET | 使用统计摘要（技能调用 / 文档访问 / 搜索热度） |
| `/api/v1/usage/stats` | GET | 用量统计（模型调用 token 消耗等） |
| `/api/v1/traces` | GET | 结构化轨迹查询（Trace 回放查询） |
| `/api/v1/scheduler/status` | GET | 定时任务状态（任务列表 / 执行次数 / 距上次执行） |
| `/api/v1/events` | GET | 统一事件流（SSE 常驻，事件带 type 区分） |
| `/api/v1/events` | POST | 事件 webhook 接收（事件总线外部投递入口） |
| `/api/v1/events/subscribe` | POST | 事件订阅（快照恢复触发点，ADR-029） |
| `/api/v1/approval/status` | GET | 审批状态（风险配置 / 待处理 / 待确认 / 审计记录） |
| `/api/v1/approval/respond` | POST | 响应待处理审批请求（GUI 审批面板） |
| `/api/v1/clipboard/capture` | POST | 剪贴板桥接：请求捕获系统剪贴板（agent 触发 → GUI 响应） |
| `/api/v1/clipboard/respond` | POST | 剪贴板桥接：提交捕获结果（GUI → agent） |
| `/api/v1/clipboard/pending` | GET | 剪贴板桥接：获取待处理捕获请求 |
| `/api/v1/clipboard/outbox` | GET | 剪贴板桥接：获取写入队列（`clipboard_write` 工具输出） |
| `/api/v1/skills` | GET | 技能列表 |
| `/api/v1/skills/stats` | GET | 技能使用统计 |
| `/api/v1/skills/{id}` | GET | 技能详情 |
| `/api/v1/skills/{id}/execute` | POST | 读取技能文档（方法论文档，无执行语义） |
| `/api/v1/tasks` | GET | 后台任务列表（delegate_to_agent background 任务） |
| `/api/v1/tasks/stream` | GET | 后台任务流（兼容入口；推荐用 `GET /events`） |
| `/api/v1/tasks/{id}/cancel` | POST | 取消后台任务（终态幂等；不存在 404） |
| `/api/v1/tasks/{id}/log` | GET | 后台任务命令输出日志 |
| `/api/v1/todos` | GET / POST | 会话待办清单（列表 / 创建） |
| `/api/v1/todos/{id}` | PATCH / DELETE | 更新 / 删除待办 |
| `/api/v1/goals` | GET / POST | 会话目标（列表 / 创建） |
| `/api/v1/goals/{id}` | PATCH / DELETE | 更新 / 删除目标 |
| `/api/v1/tools` | GET | 工具目录（Agent 可见工具及描述） |
| `/api/v1/roles` | GET | 子智能体角色列表（ADR-016） |
| `/api/v1/roles/stats` | GET | 角色使用统计 |
| `/api/v1/roles/{name}` | GET / DELETE | 角色详情 / 退役 |
| `/api/v1/roles/{name}/reset` | POST | 回退内置种子（恢复出厂定义） |
| `/api/v1/scheduled-tasks` | GET / POST | 定时智能体任务（间隔制 + 补跑，ADR-024） |
| `/api/v1/scheduled-tasks/{id}` | DELETE | 删除定时任务 |
| `/api/v1/workspace/tree` | GET | 工作区文件树 |
| `/api/v1/workspace/dirs` | GET | 工作区目录列表 |
| `/api/v1/workspace/read` | GET | 工作区文件读取 |
| `/api/v1/workspace/diff` | GET | 工作区差异 |
| `/api/v1/workspace/apply-patch` | POST | 工作区补丁应用（unified diff 信封） |
| `/api/v1/workspace/apply-edit` | POST | 工作区编辑应用（内容匹配 old/new） |
| `/api/v1/config` | GET / PUT | 读取 / 更新配置（PUT 持久化写入 tianyan.toml） |
| `/api/v1/config/status` | GET | 配置状态 |
| `/api/v1/config/{section}` | GET | 获取指定配置节 |
| `/api/v1/config/models` | GET | 模型列表 |
| `/api/v1/config/models/switch` | POST | 切换当前模型 |
| `/api/v1/config/test-connection` | POST | 测试模型连接 |
| `/api/v1/config/migrate-data-dir` | POST | 数据目录搬迁（ADR-023） |
| `/api/v1/config/soul` | GET / PUT | 读取 / 更新 Agent 人格设定 |
| `/api/v1/config/soul/default` | GET | 获取默认人格设定 |
| `/api/v1/config/mcp/servers` | GET / POST | 列出 / 添加 MCP 服务器 |
| `/api/v1/config/mcp/servers/{name}` | DELETE / PUT | 移除 / 启停 MCP 服务器 |
| `/api/v1/config/mcp/servers/{name}/test` | POST | 测试 MCP 服务器连接 |
| `/api/v1/config/providers/test` | POST | 测试 Provider 连接（protocol: openai/ollama） |
| `/api/v1/config/providers/scan` | POST | 扫描 Provider 模型（protocol: openai/ollama） |
| `/api/v1/config/providers/add-model` | POST | 注册模型到 Provider 配置 |

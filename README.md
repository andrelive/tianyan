# Tianyan（天演）

一个基于大语言模型的本地智能代理系统。

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

## 概述

Tianyan（天演）是一个本地智能代理系统，旨在通过自然语言交互帮助用户完成各种任务。它以大语言模型（LLM）为核心智能引擎，实现了统一的上下文管理架构。

### 核心特性

- **统一上下文架构**：所有上下文（用户偏好、记忆、知识库、技能）通过统一的 URI 系统管理
- **三层摘要结构**：L0/L1/L2 分层结构，实现高效的上下文加载和 Token 优化
- **OpenAI API 标准**：统一支持所有兼容 OpenAI API 的模型服务
- **本地优先**：数据本地存储，确保隐私和离线能力
- **GUI 桌面应用**：基于 Tauri + React + TypeScript + Axum 的跨平台桌面应用
- **可扩展技能**：技能 = VFS 方法论文档（planning 预置 + GEPA 自动进化）
- **记忆自迭代**：自动从交互中学习和改进
- **对话图片输入**：聊天中粘贴/拖拽/选择图片（多模态消息链路，需 vision 模型支持）
- **浏览器感知**：通过 MCP 接入 Playwright 等浏览器服务器，Agent 可导航/点击/截图（截图自动落盘）

## 项目架构

Tianyan 采用 Workspace 多 Crate 架构：

```
tianyan/
├── Cargo.toml              # Workspace 根配置
├── core/                   # 核心库（lib 名: tianyan）
│   ├── Cargo.toml          # 库名: tianyan
│   └── src/
│       ├── lib.rs          # 核心模块导出
│       ├── common/         # 通用类型与错误（TianyanError 语义分类，ADR-014）
│       ├── db/             # 统一数据库门面（Database + SqliteDb + 业务 Repository）
│       ├── agent/          # Agent 协调器（AgentLoop + ToolRegistry；工具执行可插拔管线）
│       ├── config/         # 配置管理（含 MCP 服务器配置）
│       ├── context/        # 上下文工程（检索 + 压缩 + 组装）
│       ├── events/         # 事件通道
│       ├── executor/       # 工具执行支撑（安全策略、审批、LLM-as-Judge 验证门控）
│       ├── goals/          # 会话目标
│       ├── knowledge/      # 知识库导入管道
│       ├── lsp/            # LSP 客户端（诊断/跳转/符号）
│       ├── memory/         # 记忆提取
│       ├── model/          # 模型服务容器（Chat/Embedding/Vision）
│       ├── notification/   # 消息通知与唤醒原语
│       ├── observability/  # 可观测性（AgentMetrics/UsageStats/Trace/ExecutionLog）
│       ├── roles/          # 子智能体角色基础类型（ADR-016）
│       ├── scheduler/      # 定时任务调度（摘要/进化/GC/提醒/统计落盘）
│       ├── session/        # 会话管理（SQLite 权威存储，ADR-018）
│       ├── skills/         # 技能 = VFS 方法论文档（发现/读取 + GEPA 进化）
│       ├── snapshot/       # 工作区快照（回退/重做；gzip + GC）
│       ├── todos/          # 会话待办清单
│       ├── vfs/            # 虚拟文件系统（L0/L1/L2 + LanceDB 向量，RRF 融合检索）
│       ├── role_store.rs   # 角色 VFS 存储
│       └── ...
├── server/                 # Axum HTTP 后端服务
│   ├── Cargo.toml          # 库名: tianyan-server
│   └── src/
│       ├── lib.rs          # start_server() + bootstrap_app_vfs()
│       ├── main.rs         # 独立运行入口（默认 127.0.0.1:3000，TIANYAN_PORT 可覆盖；不解析 CLI 参数）
│       ├── state.rs        # 共享 AppState（热重载、组件装配、剪贴板桥接）
│       ├── agent_builder.rs  # Agent 组装（动态工具注入）
│       ├── mcp_bridge.rs     # MCP 桥接（浏览器截图等产物落盘 mcp_images/）
│       ├── notification.rs   # 消息通知与唤醒原语
│       ├── event_push.rs     # 统一事件推送（SSE 常驻流）
│       ├── evolution_executor.rs  # 演化智能体执行
│       ├── migration.rs      # 数据目录搬迁（ADR-023）
│       ├── scheduled_tasks/  # 定时智能体任务（间隔制 + 补跑，ADR-024）
│       └── api/            # HTTP API（每个域 handler/routes/services 结构）
│           ├── chat/       # 对话接口（SSE 流式、停止、追问回答）
│           ├── sessions/   # 会话管理（消息回退/重做、标题编辑、压缩）
│           ├── knowledge/  # 知识导入、检索、条目浏览
│           ├── skills/     # 技能列表与详情（VFS 方法论文档）
│           ├── config/     # 配置管理（soul/MCP/Provider 发现/数据迁移）
│           ├── clipboard/  # 剪贴板桥接（capture/respond/pending/outbox）
│           ├── events/     # 事件 webhook 接收
│           ├── insights/   # 统计/用量/审批状态
│           ├── tasks/      # 后台任务 + 统一事件流（GET /events）
│           ├── todos/      # 待办清单
│           ├── goals/      # 会话目标
│           ├── tools/      # 工具目录
│           ├── roles/      # 子智能体角色管理（ADR-016）
│           ├── traces/     # 结构化轨迹查询
│           ├── workspace/  # 工作区文件树/读取/差异/编辑
│           └── shared/     # 通用错误类型与 DTO
├── gui-vite/                # React TypeScript 前端
│   ├── package.json          # npm 依赖
│   ├── vite.config.ts        # Vite 构建配置
│   └── src/
│       ├── main.tsx          # React 应用入口
│       ├── App.tsx           # 路由 + 主题
│       ├── lib/              # 类型、API 客户端、状态管理
│       └── components/       # UI 组件
│           ├── chat/         # 聊天面板（SSE 流式，EventSource 常驻）
│           ├── sidebar/      # 侧边栏 + 会话管理
│           ├── settings/     # 设置面板（14 个 Tab）
│           ├── knowledge/    # 知识管理
│           ├── skills/       # 技能浏览 + 详情
│           └── wizard/       # 初次配置向导
├── tauri/                  # Tauri 桌面包装
│   ├── Cargo.toml          # 库名: tianyan-tauri
│   ├── tauri.conf.json     # Tauri 配置
│   ├── build.rs            # 构建脚本
│   ├── icons/              # 应用图标
│   └── src/
│       ├── lib.rs          # Tauri 库入口
│       ├── main.rs         # 桌面应用入口
│       └── server.rs       # 内嵌服务器启动
└── mcp/                   # MCP 协议客户端（stdio + streamable HTTP 传输）
    ├── Cargo.toml         # 库名: tianyan-mcp
    └── src/
        ├── client.rs      # 单连接 MCP 客户端
        ├── manager.rs     # 多连接管理器
        └── types.rs       # 服务器配置（复用 core 类型）
```

### 技术栈

| 组件 | 版本 | 说明 |
|-----|------|------|
| **Rust** | stable（由 `rust-toolchain.toml` 管理） | 2021 Edition |
| **React** | 18.3 | TypeScript 前端框架 |
| **Vite** | 5.4 | 前端构建工具 |
| **Axum** | 0.8 | HTTP 后端框架 |
| **Tauri** | 2.2+ | 桌面应用包装 |
| **Tokio** | 1.35+ | 异步运行时 |
| **Zustand** | 4.5 | 前端状态管理 |
| **Tailwind CSS** | 3.4 | 原子化 CSS 框架 |
| **LanceDB** | 0.33 | 嵌入式向量数据库 |
| **SQLite** | rusqlite 0.31 | 主存储后端（VFS 内容 + 会话 + 统计，单文件单连接） |

## 安装

### 前置要求

- **Rust**（stable 通道，由 `rust-toolchain.toml` 管理）
- **Node.js** 18+（用于前端构建）
- **LanceDB** 嵌入式向量数据库（无需外部服务）
- **Tauri CLI**: `cargo install tauri-cli`
- **Windows**: Microsoft Edge WebView2 Runtime

### 快速安装

**Linux/macOS：**
```bash
curl -fsSL https://raw.githubusercontent.com/andrelive/tianyan/master/scripts/install.sh | bash
```

**Windows (PowerShell)：**
```powershell
irm https://raw.githubusercontent.com/andrelive/tianyan/master/scripts/install.ps1 | iex
```

### 从源码构建

```bash
# 克隆仓库
git clone https://github.com/andrelive/tianyan.git
cd tianyan

# 构建整个 Workspace
cargo build --release
```

### GUI 桌面应用构建

**快速构建（推荐）：**

```bash
# Linux/macOS
./scripts/build.sh

# Windows PowerShell
.\scripts\build.ps1
```

**手动构建：**

```bash
# 1. 构建前端 (React/TypeScript)
cd gui-vite
npm install
npm run build
cd ..

# 2. 构建 Tauri 桌面应用
cd tauri
cargo tauri build

# 安装包输出位置：
# Windows: tauri/target/release/bundle/msi/
# Linux:   tauri/target/release/bundle/deb/ 或 appimage/
# macOS:   tauri/target/release/bundle/dmg/
```

**开发模式：**

```bash
# 完整开发模式（热重载）
cd tauri && cargo tauri dev

# 仅后端服务
cargo run -p tianyan-server

# 仅前端（需要后端已启动）
cd gui-vite && npm run dev
```

## 快速开始

### 1. 启动 GUI 桌面应用

构建完成后，运行安装包或直接启动：

```bash
# Windows
.\tauri\target\release\tianyan-tauri.exe

# Linux
./tauri/target/release/tianyan-tauri

# macOS
open ./tauri/target/release/tianyan-tauri.app
```

启动流程：
1. Tauri 启动
2. 内嵌 Axum 服务器启动（127.0.0.1:3000）
3. React/TypeScript 前端加载
4. 显示主界面

### 2. 初始化配置

首次启动后，在设置面板中配置：

- **模型服务**：任意 OpenAI 兼容 API（OpenAI / Anthropic / DeepSeek / Ollama 本地等）
- **API 密钥**：在 Provider 配置中填写，支持 `${ENV_VAR}` 引用环境变量
- **知识库路径**：文档存储位置

> **注意：** 天演仅提供 GUI 桌面应用和 HTTP API 服务，不支持 CLI 交互模式。所有配置请在 GUI 设置页面中完成。

### 3. 配置 API 密钥

配置文件位于 `~/.tianyan/tianyan.toml`（首次启动后自动生成）。
API 密钥可写明文，或用 `${ENV_VAR}` 引用环境变量：

```toml
[[models.providers]]
name = "openai"
endpoint = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"

[models.preferences]
chat = { provider = "openai", model = "gpt-4" }
embedding = { provider = "openai", model = "text-embedding-3-small" }
```

## 基础用法

### GUI 桌面应用

启动后，你可以：

- **对话聊天**：在主界面输入消息，支持 Markdown 渲染
- **会话管理**：左侧边栏查看历史会话，点击切换
- **知识导入**：设置面板中导入文档到知识库
- **实时流式**：对话响应实时显示，支持打字机效果
- **设置管理**：模型配置、API 密钥、界面主题

### 独立 API 服务

```bash
# 启动 HTTP API 服务（默认监听 127.0.0.1:3000；TIANYAN_PORT 可覆盖端口；不解析 --host/--port 参数）
cargo run -p tianyan-server

# 发布版本同样无需任何命令行参数
./target/release/tianyan-server
```

API 端点（全部业务接口挂载于 `/api/v1` 前缀下）：

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
| `/api/v1/memory` | GET | 记忆列表（VFS memory 命名空间浏览） |
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

## 配置

配置文件固定位于 **`~/.tianyan/tianyan.toml`**（首次启动自动生成）；
环境变量 `TIANYAN_CONFIG` 可显式指定其他路径（e2e/CI 用）。
数据目录与配置目录分离（ADR-023）：数据目录由配置 `storage.data_dir` 决定
（默认 `%LOCALAPPDATA%/tianyan` / `~/.local/share/tianyan`），
搬迁数据目录不会移动配置文件。

详见 [config.example.toml](./config.example.toml) 获取完整示例。

### 环境变量

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `TIANYAN_CONFIG` | 配置文件路径（覆盖固定位置） | `~/.tianyan/tianyan.toml` |
| `TIANYAN_DATA_DIR` | 数据存储目录（未在配置中指定时） | `%LOCALAPPDATA%/tianyan` / `~/.local/share/tianyan` |

API 密钥在配置文件中以 `${ENV_VAR}` 形式引用（如 `api_key = "${OPENAI_API_KEY}"`），
运行时从环境变量解析；直接写明文密钥也支持。

> **日志配置**：级别与格式通过 `tianyan.toml` 的 `[logging]` 节配置（`level` / `format`）。
> `RUST_LOG` 环境变量可覆盖日志级别（tracing 标准行为）。

## 架构

Tianyan 采用四层架构（Tauri → React/TypeScript 前端 → Axum Server → Core Library）。
核心设计基于一系列架构决策记录（ADR-001~031，见 [架构决策记录](./docs/architecture/decisions/)）：

- **VFS 双层摘要索引**（ADR-001）— L0/L1/L2 分层 + RRF 融合检索，统一存储与检索基础
- **StructuredMessage**（ADR-002）— 贯穿持久化、会话组装、Token 统计的单一真相源
- **组件工具化**（ADR-003）— 30 个内置工具 + 动态注册，OpenAI function calling 兼容
- **前缀匹配上下文组装**（ADR-004）— soul→rules→memories→history 固定顺序
- **SQLite 主存储后端**（ADR-005）— 单文件、单连接，生产默认；`[storage] backend = "local"` 可回退本地文件
- **工作区快照独立存储**（ADR-006/008）— 会话回退时恢复文件修改（VFS 例外，gzip + GC）
- **会话权威存储迁至 SQLite**（ADR-018）— 流式追加与文档存储结构性不匹配；`SessionStore` 统一读写
- **统一事件推送**（ADR-028/029）— SSE 常驻流 + 快照恢复，消息落库即推送
- **配置目录与数据目录分离**（ADR-023）— 配置固定 `~/.tianyan/tianyan.toml`，数据目录可搬迁
- **间隔调度**（ADR-024）— 定时任务从 cron 改为间隔 + 宕机补跑

详见 [系统架构文档](./docs/system-architecture.md) 和 [架构决策记录](./docs/architecture/decisions/)。

## 文档

- [系统架构](./docs/system-architecture.md)
- [模块索引](./docs/architecture/module-map.md)
- [设计原则](./docs/architecture/principles.md)
- [架构决策记录](./docs/architecture/decisions/)
- [更新日志](./CHANGELOG.md)

## 支持的模型

天演不绑定特定模型厂商：模型配置采用 **Provider → Model 二级结构**，
任何提供 **OpenAI 兼容 API** 的服务均可接入（chat / embedding / vision 按能力标签选型），
包括 OpenAI、Anthropic、DeepSeek、本地 Ollama（同时支持 OpenAI 兼容端点与原生协议）等。

每个模型条目可选配 `context_length` / `max_output_tokens` / `max_input_tokens` 规格字段；
不配置时按内置规格表自动匹配主流模型（gpt-4o、claude、qwen、llama、deepseek 等），
回退全局默认（32K 窗口 / 8K 输出）。支持 `protocol: openai|ollama` 的
Provider 模型扫描（`POST /api/v1/config/providers/scan`）一键接入本地服务。

## 项目状态

Tianyan 正在积极开发中。详见 [系统架构文档](./docs/system-architecture.md)。

### 核心能力

- Agent Loop 架构（LLM 自主工具调用 + 流式响应；主 agent 与子代理同构，ADR-030）
- VFS 双层摘要索引（L0/L1/L2 三层内容 + RRF 融合检索 + LanceDB 向量）
- StructuredMessage 单一真相源（持久化 + 会话组装 + Token 统计）
- 30 个内置工具 + 动态注册工具（文件操作、代码搜索、语义化编辑、网页搜索/抓取、知识导入、技能调用、子 Agent 委托、GEPA 统计、会话回忆、定时任务、剪贴板桥接等）
- 多模态对话（图片输入全链路：粘贴/拖拽 → 持久化 → 历史重放；需 vision 模型支持）
- MCP 工具桥接（外部 MCP 服务器工具进入 Agent 循环；stdio + streamable HTTP 传输；浏览器截图自动落盘 `{data_dir}/mcp_images/`）
- 子 Agent 编排（`delegate_to_agent`：并行委托 + 嵌套委托（深度上限 3）+ 角色注册表（ADR-016）+ **后台任务**（fire-and-forget，完成自动通知））
- 后台任务管理（`task_status`/`task_cancel` 工具 + `GET /api/v1/tasks`，统一面板，ADR-026）
- 剪贴板桥接（agent `clipboard_write` 工具 → 系统剪贴板；`/api/v1/clipboard/*` 四端点）
- LLM-as-Judge 验证门控（命令输出语义质量评估，构建验证自动判定）
- 技能 = 方法论（planning 软约束 + GEPA 自动学习技能）；文件/命令/网络等能力由内置工具直接覆盖，不重复封装为技能
- 上下文压缩（自动阈值触发 + 手动 API；压缩点 = 会话内唯一免费刷新点）
- 定时任务调度（摘要生成、规则提炼/GEPA 进化、记忆提取、存储 GC、快照 GC、用量统计落盘）
- 定时智能体任务（`schedule_task` 工具 + REST 管理，间隔制 + 宕机补跑，ADR-024）
- 统一事件推送（SSE 常驻流 + 快照恢复 + 任务状态实时更新，ADR-028/029）
- 审批降级链路（危险操作询问用户，类型化确认信号）

## 参与方式

本项目以**个人长期维护**的方式运作，**不征集外部贡献**：

- 遇到问题或想要新功能，请**提 Issue**（[GitHub Issues](https://github.com/andrelive/tianyan/issues)）
- 想基于本项目做自己的改造，欢迎 **Fork 后独立演进**——项目核心架构（VFS 统一存储、Agent 循环、会话模型）经过大量迭代已适配自身开发方式，合并外部 PR 的协调成本大于收益

详见 [开发指南](./docs/development.md)。

## 许可证

本项目采用 **MIT** 许可证 - 详见 [LICENSE](LICENSE) 文件。

## 致谢

- 灵感来源于 OpenViking 的统一文件系统范式
- 工程实践参考（设计对标、格式对齐与教训吸收）：
  - **openclaw** —— 定时任务（周期 AI 工作）与工具执行审批机制的对标
  - **opencode** —— 模型请求重试策略、语义化编辑方案（`edit` / `apply_patch`）的参考
  - **oh-my-openagent (omo)** —— `apply_patch` 信封格式（`*** Update File:`）、通知唤醒与
    角色专门化设计的对标
  - **deepseek-harness (DSH)** —— 重试策略、内容匹配编辑与任务面板优化的参考
- 使用 Rust 和优秀的开源社区构建

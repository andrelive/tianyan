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
- **可扩展技能**：内置文件操作、系统命令等技能
- **记忆自迭代**：自动从交互中学习和改进
- **对话图片输入**：聊天中粘贴/拖拽/选择图片（多模态消息链路，需 vision 模型支持）
- **浏览器感知**：通过 MCP 接入 Playwright 等浏览器服务器，Agent 可导航/点击/截图（截图自动落盘）

## 项目架构

Tianyan 采用 Workspace 多 Crate 架构：

```
tianyan/
├── Cargo.toml              # Workspace 根配置
├── core/                   # 核心库（原 src/ 迁移至此）
│   ├── Cargo.toml          # 库名: tianyan
│   └── src/
│       ├── lib.rs          # 核心模块导出
│       ├── agent/          # Agent 协调器（AgentLoop + ToolRegistry）
│       ├── config/         # 配置管理（含 MCP 服务器配置）
│       ├── context/        # 上下文工程（检索 + 压缩 + 组装）
│       ├── executor/       # 工具执行支撑（安全策略、审批、验证门控）
│       ├── knowledge/      # 知识库导入管道
│       ├── memory/         # 记忆提取
│       ├── model/          # 模型服务容器（Chat/Embedding/Vision）
│       ├── observability/  # 可观测性统计（SQLite）
│       ├── scheduler/      # 定时任务调度
│       ├── session/        # 会话管理（JSONL 持久化）
│       ├── skills/         # 技能定义 + 执行 + GEPA 进化引擎
│       ├── snapshot/       # 工作区快照（回退/重做）
│       ├── vfs/            # 虚拟文件系统（L0/L1/L2 + LanceDB 向量）
│       └── ...
├── server/                 # Axum HTTP 后端服务
│   ├── Cargo.toml          # 库名: tianyan-server
│   └── src/
│       ├── lib.rs          # 暴露 start_server() + bootstrap_app_vfs()
│       ├── main.rs         # 独立运行入口
│       ├── state.rs        # 共享 AppState（热重载、组件装配）
│       └── api/            # HTTP API（每个域 handler/routes/services/types 四件套）
│           ├── chat/       # 对话接口（含 SSE 流式、追问澄清）
│           ├── sessions/   # 会话管理（消息回退/重做、标题编辑）
│           ├── knowledge/  # 知识导入、检索、条目浏览
│           ├── skills/     # 技能列表与执行
│           ├── config/     # 配置管理（含 soul/Ollama/MCP 子模块）
│           └── shared/     # 通用错误类型与 DTO
├── gui-vite/                # React TypeScript 前端
│   ├── package.json          # npm 依赖
│   ├── vite.config.ts        # Vite 构建配置
│   └── src/
│       ├── main.tsx          # React 应用入口
│       ├── App.tsx           # 路由 + 主题
│       ├── lib/              # 类型、API 客户端、状态管理
│       └── components/       # UI 组件
│           ├── chat/         # 聊天面板（SSE 流式）
│           ├── sidebar/      # 侧边栏 + 会话管理
│           ├── settings/     # 设置面板（13 个 Tab）
│           ├── knowledge/    # 知识管理
│           ├── skills/       # 技能浏览 + 执行
│           └── wizard/       # 初次配置向导
└── tauri/                  # Tauri 桌面包装
    ├── Cargo.toml          # 库名: tianyan-tauri
    ├── tauri.conf.json     # Tauri 配置
    ├── build.rs            # 构建脚本
    ├── icons/              # 应用图标
    └── src/
        ├── lib.rs          # Tauri 库入口
        ├── main.rs         # 桌面应用入口
        └── server.rs       # 内嵌服务器启动
└── mcp/                   # MCP 协议客户端（stdio 传输）
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
| **LanceDB** | - | 嵌入式向量数据库 |

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

- **API 密钥**：OpenAI / Anthropic / DeepSeek 等
- **模型选择**：GPT-4、Claude 3、DeepSeek 等
- **知识库路径**：文档存储位置

> **注意：** 天演仅提供 GUI 桌面应用和 HTTP API 服务，不支持 CLI 交互模式。所有配置请在 GUI 设置页面中完成。

### 3. 配置 API 密钥

**环境变量方式：**

```bash
# Linux/macOS
export OPENAI_API_KEY='your-api-key'

# Windows (PowerShell)
$env:OPENAI_API_KEY = 'your-api-key'
```

**配置文件方式：**

编辑 `~/.config/tianyan/tianyan.toml`（首次启动后自动生成），填入你的 API 密钥：

```toml
[[models.providers]]
name = "openai"
endpoint = "https://api.openai.com/v1"
api_key = "your-api-key"

[[models.providers.models]]
name = "gpt-4"
capabilities = ["chat"]

[[models.providers.models]]
name = "text-embedding-3-small"
capabilities = ["text-embedding"]

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
# 启动 HTTP API 服务
cargo run -p tianyan-server -- --host 127.0.0.1 --port 3000

# 或使用发布版本
./target/release/tianyan-server --host 127.0.0.1 --port 3000
```

API 端点（全部业务接口挂载于 `/api/v1` 前缀下）：

| 端点 | 方法 | 说明 |
|------|------|------|
| `/health` | GET | 健康检查 |
| `/api/v1/chat` | POST | 对话请求 |
| `/api/v1/chat/stream` | POST | 流式对话（SSE） |
| `/api/v1/chat/clarify` | POST | 追问澄清（提交澄清问题回答） |
| `/api/v1/sessions` | GET | 会话列表 |
| `/api/v1/sessions/{id}` | GET / DELETE | 获取 / 删除会话 |
| `/api/v1/sessions/{id}/messages` | GET | 会话消息列表 |
| `/api/v1/sessions/{id}/messages/delete` | POST | 删除指定消息 |
| `/api/v1/sessions/{id}/messages/redo` | POST | 重做指定消息 |
| `/api/v1/sessions/{id}/title` | POST | 更新会话标题 |
| `/api/v1/knowledge/ingest` | POST | 文档导入（multipart） |
| `/api/v1/knowledge/search` | GET | 知识搜索 |
| `/api/v1/knowledge/search/suggestions` | GET | 搜索建议 |
| `/api/v1/knowledge/entries` | GET | 知识条目列表 |
| `/api/v1/knowledge/entries/read` | GET | 读取知识条目内容 |
| `/api/v1/knowledge/entries/delete` | POST | 删除知识条目（递归删除子条目 + 同步清理向量索引） |
| `/api/v1/memory` | GET | 记忆列表（VFS memory 命名空间浏览） |
| `/api/v1/stats` | GET | 使用统计摘要（技能调用 / 文档访问 / 搜索热度） |
| `/api/v1/retrieval/traces` | GET | 最近检索轨迹（单次检索完整过程，FIFO 500 条） |
| `/api/v1/scheduler/status` | GET | 定时任务状态（任务列表 / 执行次数 / 距上次执行） |
| `/api/v1/approval/status` | GET | 审批状态（风险配置 / 待处理 / 待确认 / 审计记录） |
| `/api/v1/skills` | GET | 技能列表 |
| `/api/v1/skills/{id}/execute` | POST | 执行技能（同步执行，响应即最终结果） |
| `/api/v1/config` | GET / PUT | 读取 / 更新配置 |
| `/api/v1/config/status` | GET | 配置状态 |
| `/api/v1/config/{section}` | GET | 获取指定配置节 |
| `/api/v1/config/models` | GET | 模型列表 |
| `/api/v1/config/models/switch` | POST | 切换当前模型 |
| `/api/v1/config/test-connection` | POST | 测试模型连接 |
| `/api/v1/config/soul` | GET / PUT | 读取 / 更新 Agent 人格设定 |
| `/api/v1/config/soul/default` | GET | 获取默认人格设定 |
| `/api/v1/config/mcp/servers` | GET / POST | 列出 / 添加 MCP 服务器 |
| `/api/v1/config/mcp/servers/{name}` | DELETE / PUT | 移除 / 启停 MCP 服务器 |
| `/api/v1/config/mcp/servers/{name}/test` | POST | 测试 MCP 服务器连接 |
| `/api/v1/config/ollama/test` | POST | 测试 Ollama 连接 |
| `/api/v1/config/ollama/scan` | POST | 扫描 Ollama 模型 |
| `/api/v1/config/ollama/add-model` | POST | 添加 Ollama 模型 |

## 配置

Tianyan 按以下顺序查找配置文件：

1. `./tianyan.toml`（当前目录）
2. `~/.config/tianyan/tianyan.toml`（用户配置）
3. `~/.tianyan/tianyan.toml`（备用位置）

详见 [config.example.toml](./config.example.toml) 获取完整示例。

### 环境变量

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `TIANYAN_CONFIG` | 配置文件路径 | - |
| `TIANYAN_DATA_DIR` | 数据存储目录 | `~/.local/share/tianyan` |
| `OPENAI_API_KEY` | OpenAI API 密钥 | - |
| `ANTHROPIC_API_KEY` | Anthropic API 密钥 | - |
| `DEEPSEEK_API_KEY` | DeepSeek API 密钥 | - |

> **日志配置**：级别与格式通过 `tianyan.toml` 的 `[logging]` 节配置（`level` / `format`）。
> `RUST_LOG` 环境变量可覆盖日志级别（tracing 标准行为）。

详见 [.env.example](./.env.example) 获取更多选项。

## 架构

Tianyan 采用四层架构（Tauri → React/TypeScript 前端 → Axum Server → Core Library），核心设计基于 6 个架构决策：

- **VFS 双层摘要索引**（L0/L1/L2 + RRF 融合检索）— 统一存储与检索基础
- **StructuredMessage** — 贯穿持久化、会话组装、Token 统计的单一真相源
- **组件工具化** — 23 个 OpenAI function calling 兼容工具
- **前缀匹配上下文组装** — soul→rules→memories→history 固定顺序
- **SQLite 主存储后端** — 单文件、单连接，替代本地文件后端
- **工作区快照独立存储** — 会话回退时恢复文件修改（VFS 例外）

另见：[架构决策记录](./docs/architecture/decisions/)（含 ADR-010 对话多模态链路）。

详见 [系统架构文档](./docs/system-architecture.md) 和 [架构决策记录](./docs/architecture/decisions/)。

## 文档

- [系统架构](./docs/system-architecture.md)
- [模块索引](./docs/architecture/module-map.md)
- [设计原则](./docs/architecture/principles.md)
- [架构决策记录](./docs/architecture/decisions/)
- [更新日志](./CHANGELOG.md)

## 支持的模型

| 提供商 | 模型 | 状态 |
|--------|------|------|
| OpenAI | GPT-4, GPT-4 Turbo, GPT-3.5 | 支持 |
| Anthropic | Claude 3 Opus, Claude 3 Sonnet | 支持（OpenAI 兼容接口） |
| DeepSeek | DeepSeek Chat, DeepSeek Coder | 支持 |
| 自定义 | OpenAI 兼容 API | 支持 |

## 项目状态

Tianyan 正在积极开发中。详见 [系统架构文档](./docs/system-architecture.md)。

### 核心能力

- Agent Loop 架构（LLM 自主工具调用 + 流式响应）
- VFS 双层摘要索引（L0/L1/L2 三层内容 + RRF 融合检索）
- StructuredMessage 单一真相源（持久化 + 会话组装 + Token 统计）
- 23 个内置工具（文件操作、代码搜索、语义化编辑、网页搜索/抓取、知识导入、技能调用、子 Agent 委托）
- 多模态对话（图片输入全链路：粘贴/拖拽 → 持久化 → 历史重放；需 vision 模型支持）
- MCP 工具桥接（外部 MCP 服务器工具进入 Agent 循环；浏览器截图自动落盘 `{data_dir}/mcp_images/`）
- 子 Agent 编排（`delegate_to_agent`：并行委托 + 嵌套委托（深度上限 3）+ `max_turns`/`timeout_secs`）
- 回答质量评测（LLM-as-Judge 评分式四维度评测 + 黄金用例，离线基准）
- 6 个内置技能 + GEPA 进化引擎自动学习（学习回路：VFS 存储 → 启动时注册 → 可发现/执行指引）
- 定时任务调度（记忆提取、规则提炼、摘要生成）
- 审批降级链路（危险操作询问用户，类型化确认信号）

## 贡献

欢迎贡献！详见 [开发指南](./docs/development.md)。

## 许可证

本项目采用 MIT 许可证 - 详见 [LICENSE](LICENSE) 文件。

## 致谢

- 灵感来源于 OpenViking 的统一文件系统范式
- 使用 Rust 和优秀的开源社区构建

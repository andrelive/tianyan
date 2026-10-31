# Tianyan（天演）

一个基于大语言模型的本地智能体系统。

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

## 概述

Tianyan（天演）是一个本地优先的通用智能体：以大语言模型（LLM）为核心智能引擎，通过自然语言交互帮助用户完成各类任务。

所有上下文（用户偏好、记忆、知识库、技能）统一由自研 **VFS（虚拟文件系统）** 管理——L0/L1/L2 双层摘要索引 + 向量融合检索；记忆与技能由**自演化综述**定期归纳演进（每日一次，读用户原话 + 执行数据 + 使用统计后产出增删改计划）。数据本地存储（SQLite + LanceDB），隐私优先。项目持续开发中，发布记录见 [Releases](https://github.com/andrelive/tianyan/releases) 与 [CHANGELOG](./CHANGELOG.md)。

## 核心特性

- **统一上下文（VFS）**：偏好 / 记忆 / 知识 / 技能同构存储与检索，分层摘要 + RRF 融合；上下文自动 / 手动压缩
- **自进化（ADR-017）**：单一「自演化综述」任务统一接管记忆 / 技能 / 规则 / 角色的归纳与淘汰（读用户原话 + GEPA 执行数据 + 技能使用统计，产出增删改计划并记账）；`planning` 等预置技能 + 技能导入工具
- **桌面应用**：Tauri + React + TypeScript + Axum 跨平台应用（Windows 提供 MSI 安装包）
- **工具系统**：**25 个内置 + 13 个插件工具**（文件读写 / 语义化编辑 / 代码结构 / 搜索 / 网页抓取 / 知识导入 / 待办与目标 / 委托 / 定时任务 / 剪贴板……），OpenAI function calling 兼容，支持动态注册（完整清单见 [tool-catalog](./docs/architecture/tool-catalog.md)）
- **子智能体编排**：`delegate_to_agent` 并行委托 + 嵌套（深度上限 3）+ 角色注册表；后台任务完成自动通知
- **MCP 桥接**：接入外部 MCP 服务器（Playwright 浏览器自动化等；stdio + streamable HTTP；截图自动落盘）
- **多模态**：对话图片输入（粘贴 / 拖拽 / 选择，需 vision 模型支持）
- **模型无关**：任意 OpenAI 兼容 API 服务（OpenAI / Anthropic / DeepSeek / 本地 Ollama 等）

## 快速开始

### 安装 / 构建

- **Windows**：从 [Releases](https://github.com/andrelive/tianyan/releases) 下载 MSI 安装
- **Linux / macOS**：从源码构建：

```bash
git clone https://github.com/andrelive/tianyan.git && cd tianyan

cargo build --release    # 整个 workspace

# GUI 桌面应用 + 安装包（推荐）
.\scripts\build.ps1      # Windows
./scripts/build.sh       # Linux/macOS
# 安装包产物：target/release/bundle/（msi / deb / appimage / dmg）
```

前置要求：Rust（stable，由 `rust-toolchain.toml` 管理）、Node.js 18+（前端构建）、Tauri CLI（`cargo install tauri-cli`）；Windows 需 Edge WebView2 Runtime。

### 运行

```bash
# 桌面应用（开发模式，热重载）
cd tauri && cargo tauri dev

# 运行构建产物（需先执行 scripts/build 脚本）
.\target\release\tianyan-tauri.exe        # Windows
./target/release/tianyan-tauri            # Linux

# 仅后端服务（默认 127.0.0.1:3000，TIANYAN_PORT 可覆盖端口；不解析 CLI 参数）
cargo run -p tianyan-server

# 仅前端（需后端已启动）
cd gui-vite && npm run dev
```

### 首次配置

在 GUI 设置面板中完成：

- **模型服务**：任意 OpenAI 兼容 API + API 密钥（支持 `${ENV_VAR}` 引用环境变量）
- **知识库路径**：文档存储位置

配置文件固定 `~/.tianyan/tianyan.toml`（首次启动自动生成；`TIANYAN_CONFIG` 可覆盖路径）。数据目录与配置目录分离（ADR-023），可在设置页搬迁；完整配置见 [config.example.toml](./config.example.toml)，日志配置见 `[logging]` 节（`RUST_LOG` 可覆盖级别）。

> 天演提供 **GUI 桌面应用 + HTTP API 服务**，不支持 CLI 交互模式。

## 架构

四层：**Tauri 桌面壳 → React/TypeScript 前端 → Axum 服务 → Rust 核心库**。Workspace 结构：

| 目录 | 说明 |
|------|------|
| `core/` | 核心库（lib 名 `tianyan`）：Agent 循环、工具系统、VFS、上下文工程（检索/压缩/组装）、会话、快照、可观测性等 |
| `server/` | Axum HTTP 服务：REST API + SSE 事件流 + MCP 桥接 + 定时任务（**经类型擦除插槽接收能力，不认识任何插件**） |
| `assembly/` | 装配清单（ADR-049 期 4）：插件工厂与 `full()` / `policies()` 两份清单的唯一实现 |
| `app/` | headless 装配器（bin `tianyan-server`）：配置加载 + 启动 + 搬迁重启 |
| `plugins/*` | 15 个编译期插件（工具 / 存储 / 任务 / 策略 / provider / 工具管线），只依赖 `core` |
| `service/config/` | 服务层：配置加载 / 持久化 / 数据目录迁移 / 校验 / 向导 DTO |
| `tools-ops/` | 运维工具（服务停止后直接修数据的终端脚本，**不被任何 crate 依赖**） |
| `gui-vite/` | React + TypeScript 前端（Vite） |
| `tauri/` | Tauri 桌面包装（内嵌服务器） |
| `mcp/` | MCP 协议客户端（stdio + streamable HTTP） |

技术栈：Rust（stable）· Axum 0.8 · Tauri 2.12+ · Tokio · **React 19 + Vite 8（Rolldown）+ Zustand 5 + Tailwind 3.4** · LanceDB（嵌入式向量，0.33 刻意未升）· SQLite（主存储）。

核心设计基于一系列架构决策记录（[ADR](./docs/architecture/decisions/)）：VFS 双层摘要索引（ADR-001）、StructuredMessage 单一真相源（ADR-002）、组件工具化（ADR-003）、前缀匹配上下文组装（ADR-004）、统一事件推送（ADR-028/029）、统一 Agent 循环（ADR-030）等。

## 支持的模型

天演不绑定特定模型厂商：模型配置采用 **Provider → Model 二级结构**，任何提供 **OpenAI 兼容 API** 的服务均可接入（chat / embedding / vision 按能力标签选型），包括 OpenAI、Anthropic、DeepSeek、本地 Ollama（同时支持 OpenAI 兼容端点与原生协议 `protocol: openai|ollama`，可一键扫描接入）。

模型规格（`context_length` / `max_output_tokens` 等）可显式配置，缺省按内置规格表自动匹配主流模型（gpt-4o / claude / qwen / llama / deepseek 等）。

## HTTP API

全部业务接口挂载于 `/api/v1` 前缀下。常用入口（完整端点列表见 [docs/api-endpoints.md](./docs/api-endpoints.md)）：

| 端点 | 说明 |
|------|------|
| `GET /health` | 健康检查 |
| `POST /api/v1/chat/stream` | 流式对话（增量事件经 `GET /events` 下发） |
| `POST /api/v1/chat/answer` | 追问回答提交（`ask_user` 工具的回答入口） |
| `/api/v1/sessions/*` | 会话管理与消息操作（回退 / 重做 / 标题 / 压缩） |
| `/api/v1/knowledge/*` | 知识导入 / 检索 / 条目管理 |
| `GET /api/v1/events`（SSE） | 统一事件流（常驻） |
| `GET/PUT /api/v1/config` 等 | 配置读写 / 模型切换 / 数据目录搬迁 |
| `GET /api/v1/tasks` 等 | 后台任务（列表 / 取消 / 日志） |

## 文档

**全部文档的入口 → [`docs/README.md`](./docs/README.md)**（按「我要做什么」索引：模块 / 决策 / 机制 / 运维 / 归档）。

- 架构：[模块索引](./docs/architecture/module-map.md) · [设计原则](./docs/architecture/principles.md) · [ADR 索引（52 份）](./docs/architecture/decisions/README.md)
- 机制：[上下文管线](./docs/architecture/context-pipeline.md) · [事件协议](./docs/architecture/event-protocol.md) · [Provider 备注](./docs/architecture/model-provider-notes.md) · [任务运行时](./docs/architecture/task-runtime.md)
- 开发与运维：[开发指南](./docs/development.md) · [运维手册](./docs/operations/) · [更新日志](./CHANGELOG.md)

## 参与方式

本项目以**个人长期维护**的方式运作，不征集外部贡献：

- 遇到问题或想要新功能，请提 [Issue](https://github.com/andrelive/tianyan/issues)
- 想基于本项目做改造，欢迎 **Fork 后独立演进**（核心架构经大量迭代已适配自身开发方式，合并外部 PR 的协调成本大于收益）

## 许可证

本项目采用 **MIT** 许可证——详见 [LICENSE](./LICENSE) 文件。

## 致谢

- 灵感来源于 OpenViking 的统一文件系统范式
- 工程实践对标（设计参考与教训吸收）：openclaw（定时任务 / 审批）、opencode（请求重试 / 语义化编辑）、oh-my-openagent (omo)（apply_patch 信封 / 通知唤醒）、deepseek-harness (DSH)（重试策略 / 任务面板）
- 使用 Rust 和优秀的开源社区构建

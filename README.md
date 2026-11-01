# Tianyan（天演）

本地优先的通用智能体。**全栈 Rust —— UI、服务、智能体在同一份代码、同一个二进制里。**

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)&nbsp;
[![Rust](https://img.shields.io/badge/Rust-2021-orange.svg)](https://www.rust-lang.org/)

## 为什么是「前后端同栈」

桌面 AI 应用通常是三段式：WebView 壳（Electron / Tauri）+ TypeScript 前端 + 另起的后端进程。天演把三段合成**一段**：

| | 典型 Electron / Tauri 应用 | 天演 |
|---|---|---|
| 界面 | HTML/CSS/TS，跑在 WebView 里 | **GPUI 原生自绘（Rust）** |
| 服务 | Node / Python / 另起的子进程 | **Axum（Rust），与界面同进程** |
| 智能体与存储 | 再一个进程，或在云上 | **同一个进程里的 `core` 库** |
| 发布的成品 | 安装器 + Node 运行时 + WebView2 | **一个 exe**（MSI 约 60 MB） |
| 同一次会话的内存实测 | **837 MB**（主进程 113.5 + `msedgewebview2` × 6 共 725.6） | **76 MB** 空窗口 / **131 MB** 带 3000 行编辑器 + 5000 条列表 / 269 MB 打开 10 万行代码文件 |

同栈换来的不只是"少一个进程"：

- **改一个功能只动一处。** 工具定义、数据结构、错误类型在界面与后端之间**没有第二份** —— 不需要手写 IPC schema，也不需要让 `dto.ts` 跟服务端结构手工对齐。
- **`cargo check` 就是全链路检查。** 界面与服务一起编译；字段名改错当场红，不用等运行时才发现。
- **协议也共用一份类型。** 界面经**本地 HTTP / SSE** 与内嵌服务通信（同进程、不是跨进程 IPC，也不经 WebView 桥），两端由 `core` 的同一组类型生成。
- **不依赖 WebView。** 不用 Edge WebView2 / Chromium，安装包里没有浏览器引擎，启动即窗口。

> **代价（诚实记账）**：GPUI 生态比 React 年轻，富文本渲染、IME、无障碍等细节需要自己补；Rust 编译比 TS 慢（本机增量约 2 秒/次，冷编译数分钟）。这两条都是"已用实测数据换来的确定成本"，不是猜测。

## 核心特性

- **全栈 Rust 桌面**：界面（GPUI）+ 服务（Axum）+ 智能体（core）同进程、同二进制；发布物是单个 exe（Windows 提供 MSI，perUser 静默安装）
- **统一上下文（VFS）**：偏好 / 记忆 / 知识 / 技能同构存储与检索 —— L0/L1/L2 三层摘要 + 双向量 RRF 融合；上下文自动 / 手动压缩（ADR-001）
- **自进化（ADR-017）**：单一「自演化综述」任务统一接管记忆 / 技能 / 规则 / 角色的归纳与淘汰（读用户原话 + 执行数据 + 技能使用统计，产出增删改计划并记账）；附 `planning` 等预置技能与技能导入工具
- **工具系统**：50+ 内置与插件工具（文件读写 / 语义化编辑 / 代码结构 / 搜索 / 网页抓取 / 知识导入 / 待办与目标 / 委托 / 定时任务 / 剪贴板……），OpenAI function calling 兼容，支持动态注册（完整清单见 [tool-catalog](./docs/architecture/tool-catalog.md)）
- **子智能体编排**：`delegate_to_agent` 并行委托 + 嵌套（深度上限 3）+ 角色注册表；后台任务完成自动唤醒主会话
- **MCP 桥接**：接入外部 MCP 服务器（Playwright 浏览器自动化等；stdio + streamable HTTP；截图自动落盘）
- **多模态**：对话图片输入（粘贴 / 拖拽 / 选择，需 vision 模型支持）
- **模型无关**：任意 OpenAI 兼容 API（OpenAI / Anthropic / DeepSeek / 本地 Ollama 等）

## 快速开始

### 安装（Windows）

从 [Releases](https://github.com/andrelive/tianyan/releases) 下载 `tianyan-native-<ver>-x64.msi` 双击安装（perUser，无需管理员权限；带 `SHA256SUMS.txt` 可先校验）。

### 从源码构建

```bash
git clone https://github.com/andrelive/tianyan.git && cd tianyan

cargo build --release                 # 后端 + 核心库（主 workspace）

cd gui-native && cargo build --release   # 原生客户端（独立 workspace，自带 [workspace]）
cd .. && pwsh -File gui-native/installer/build-msi.ps1 -SkipBuild   # 打 MSI
```

前置要求：Rust stable（由 `rust-toolchain.toml` 管理）；打 MSI 另需 WiX v6（`dotnet tool install --global wix --version 6.0.2`）。
**不需要 Node.js** —— 那是 `gui-vite/`（保留作对照的 Web 前端）才需要的东西。

### 运行

```powershell
.\gui-native\target\release\tianyan-gui-native.exe   # 桌面应用：内嵌 server，端口自动避让

cargo run -p tianyan-server                          # 只跑后端（默认 127.0.0.1:3000，TIANYAN_PORT 可覆盖；不解析 CLI 参数）
```

### 首次配置

在客户端设置面板里完成：

- **模型服务**：任意 OpenAI 兼容 API + 密钥（支持 `${ENV_VAR}` 引用环境变量）
- **知识库分类**：导入文档时选择存放分类

配置文件固定 `~/.tianyan/tianyan.toml`（首次启动自动生成；`TIANYAN_CONFIG` 可覆盖路径）。数据目录与配置目录分离（ADR-023），**设置页里数据目录只读**、只能在「搬迁」入口整库搬走；完整配置见 [config.example.toml](./config.example.toml)。

> 天演提供 **桌面应用 + HTTP API**，不支持 CLI 交互模式。

## 架构

**一个进程两半：GPUI 界面（主线程）+ 内嵌 Axum 服务（tokio 运行时），共享同一个 `tianyan` core 库。**

| 目录 | 说明 |
|------|------|
| `core/` | 核心库（lib 名 `tianyan`）：Agent 循环、工具系统、VFS、上下文工程（检索 / 压缩 / 组装）、会话、快照、可观测性 |
| `server/` | Axum HTTP 服务：REST API + SSE 事件流 + MCP 桥接 + 定时任务（**经类型擦除插槽接收能力，不认识任何插件**） |
| `assembly/` | 装配清单（ADR-049 期 4）：插件工厂与 `full()` / `policies()` 两份清单的唯一实现 |
| `app/` | headless 装配器（bin `tianyan-server`）：配置加载 + 启动 + 搬迁重启 |
| `plugins/*` | 15 个编译期插件（工具 / 存储 / 任务 / 策略 / provider / 工具管线），只依赖 `core` |
| `service/config/` | 服务层：配置加载 / 持久化 / 数据目录迁移 / 校验 / 向导 DTO |
| `tools-ops/` | 运维工具（服务停止后直接修数据的终端脚本，**不被任何 crate 依赖**） |
| **`gui-native/`** | **GPUI 原生客户端（主发布渠道）**：独立 workspace，复用 `desktop/` 做单实例发现与端口避让 |
| `desktop/` | 桌面启动编排：发现「同一数据目录」的已有实例并复用，未命中才自起（ADR-051） |
| `gui-vite/` | React + TypeScript 前端 —— **保留作对照**，不随包发布 |
| `mcp/` | MCP 协议客户端（stdio + streamable HTTP） |

技术栈：**Rust（stable）· GPUI（原生 UI）· Axum 0.8 · Tokio · LanceDB 0.33（嵌入式向量）· SQLite（主存储）**。

核心设计基于一系列架构决策记录（[ADR 索引](./docs/architecture/decisions/README.md)，共 52 份）：VFS 双层摘要索引（ADR-001）、StructuredMessage 单一真相源（ADR-002）、组件工具化（ADR-003）、前缀匹配上下文组装（ADR-004）、统一事件推送（ADR-028/029）、统一 Agent 循环（ADR-030）、微内核插件化（ADR-049）等。

## 支持的模型

天演不绑定特定模型厂商：模型配置采用 **Provider → Model 二级结构**，任何提供 **OpenAI 兼容 API** 的服务均可接入（chat / embedding / vision 按能力标签选型），包括 OpenAI、Anthropic、DeepSeek、本地 Ollama（同时支持 OpenAI 兼容端点与原生协议 `protocol: openai|ollama`，可一键扫描接入）。

模型规格（`context_length` / `max_output_tokens` 等）可显式配置，缺省按内置规格表自动匹配主流模型（gpt-4o / claude / qwen / llama / deepseek 等）。

## HTTP API

全部业务接口挂载于 `/api/v1` 前缀下。常用入口（完整端点列表见 [docs/api-endpoints.md](./docs/api-endpoints.md)）：

| 端点 | 说明 |
|------|------|
| `GET /health` | 健康检查（含 `service` / `data_dir` / `pid`，桌面端据此发现并复用已有实例） |
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
- 原生 UI 由 [GPUI](https://github.com/zed-industries/zed)（Zed 的渲染引擎）驱动；使用 Rust 与优秀的开源社区构建

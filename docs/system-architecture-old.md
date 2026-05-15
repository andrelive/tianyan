# 天演 (Tianyan) 系统架构文档

本文档详细描述天演本地智能代理系统的整体架构、运行流程和各组件间的交互。

## 目录

- [1. 系统概述](#1-系统概述)
- [2. 应用架构](#2-应用架构)
- [3. 系统初始化流程](#3-系统初始化流程)
- [4. 模型路由系统](#4-模型路由系统)
- [5. 双层检索流程 (L0 + L1)](#5-双层检索流程-l0--l1)
- [6. ReAct 智能体循环](#6-react-智能体循环)
- [7. 技能执行系统](#7-技能执行系统)
- [8. 存储架构](#8-存储架构)
  - [8.1 存储层次架构](#81-存储层次架构)
  - [8.2 核心组件职责](#82-核心组件职责)
  - [8.3 Session 与 VFS 的交互](#83-session-与-vfs-的交互)
  - [8.4 数据流示例](#84-数据流示例)
  - [8.5 存储设计原则](#85-存储设计原则)
  - [8.6 关键文件索引](#86-关键文件索引)
- [9. 记忆管理系统](#9-记忆管理系统)
  - [9.1 记忆架构](#91-记忆架构)
  - [9.2 记忆持久化架构](#92-记忆持久化架构)
  - [9.3 异步持久化流程](#93-异步持久化流程)
  - [9.4 优雅关闭机制](#94-优雅关闭机制)
  - [9.5 记忆处理流程](#95-记忆处理流程)
  - [9.6 记忆存储](#96-记忆存储)
  - [9.7 记忆检索](#97-记忆检索)
- [10. 完整数据流图](#10-完整数据流图)
  - [10.1 主数据流](#101-主数据流)
  - [10.2 记忆持久化数据流](#102-记忆持久化数据流)
  - [10.3 优雅关闭流程](#103-优雅关闭流程)
- [11. 配置结构](#11-配置结构)

---

## 1. 系统概述

天演 (Tianyan) 是一个基于 Rust 开发的本地 AI 助手桌面应用，采用 **Tauri v2 + Yew + Axum** 技术栈，支持：

- **双层向量检索**：基于摘要和概览向量的快速过滤 + 精确重排序
- **多模型路由**：支持 OpenAI 和 OpenAI Compatible API 的智能路由与故障转移
- **ReAct 智能体**：基于 Reasoning + Acting 模式的主动检索和工具执行
- **技能系统**：可扩展的工具执行能力（文件操作、HTTP 请求、系统命令等）
- **记忆管理**：对话历史的自动提取、重要性评估和持久化
- **虚拟文件系统 (VFS)**：统一的分层知识库管理（Abstract/Overview/Detail）
- **流式响应**：实时的流式输出，支持思考过程、工具调用、观察结果和最终回答

---

## 2. 应用架构

天演采用 **Tauri v2** 桌面应用架构，结合 **Yew** (Rust WASM 前端) 和 **Axum** (Rust 后端服务器)。

### 2.1 整体架构

```
┌─────────────────────────────────────────────────────────────┐
│                    Tauri Desktop App                        │
│  ┌───────────────────────────────────────────────────────┐  │
│  │              Yew Frontend (gui/)                      │  │
│  │  • 聊天界面组件                                        │  │
│  │  • 配置向导组件                                        │  │
│  │  • 设置界面                                            │  │
│  │  • 侧边栏导航                                          │  │
│  └───────────────────────────────────────────────────────┘  │
│                            ↓ HTTP Calls                     │
│  ┌───────────────────────────────────────────────────────┐  │
│  │              Axum Server (server/)                    │  │
│  │  • REST API 端点                                       │  │
│  │  • 核心桥接层 (Core Bridge)                            │  │
│  │  • 静态文件服务 (gui/dist)                             │  │
│  └───────────────────────────────────────────────────────┘  │
│                            ↓ Rust Calls                     │
│  ┌───────────────────────────────────────────────────────┐  │
│  │              Core Library (core/)                     │  │
│  │  • Agent 协调器 (ReAct 模式)                            │  │
│  │  • 双层检索器                                          │  │
│  │  • 模型路由器                                          │  │
│  │  • 记忆协调器                                          │  │
│  │  • 技能执行器                                          │  │
│  │  • 虚拟文件系统 (VFS)                                  │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 项目结构

```
tianyan/
├── core/           # 核心库 (AI Agent, 模型，记忆，检索)
│   ├── agent/      # Agent 协调器和 ReAct 执行器
│   ├── config/     # 配置管理
│   ├── context/    # 上下文检索系统
│   ├── knowledge/  # 知识库导入和解析
│   ├── memory/     # 记忆管理系统
│   ├── model/      # 模型服务和路由器
│   ├── skills/     # 技能定义和执行
│   └── storage/    # 存储后端和 VFS
├── server/         # HTTP API 服务器 (Axum)
│   ├── api/        # API 端点
│   └── core_bridge.rs  # 核心桥接层
├── gui/            # Yew 前端 (WASM)
│   ├── src/
│   │   ├── api/    # API 客户端
│   │   ├── components/  # UI 组件
│   │   └── state/  # 状态管理
│   └── dist/       # 编译后的前端资源
├── tauri/          # Tauri 桌面应用
│   ├── src/
│   │   ├── lib.rs  # Tauri 应用入口
│   │   └── server.rs  # 服务器启动
│   └── tauri.conf.json  # Tauri 配置
└── docs/           # 项目文档
```

### 2.3 启动流程

```rust
// tauri/src/lib.rs
pub fn run() {
    // 1. 初始化日志系统
    init_logging();
    
    // 2. 加载配置
    let tianyan_config = TianyanConfig::load().unwrap_or_default();
    
    // 3. 启动 Axum 服务器在后台线程
    rt.spawn(async move {
        start_axum_server(config_clone).await;
    });
    
    // 4. 等待服务器就绪（健康检查）
    wait_for_server_ready().await?;
    
    // 5. 启动 Tauri 应用
    tauri::Builder::default()
        .setup(|app| {
            // 启用开发者工具
            app.get_webview_window("main").unwrap().open_devtools();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Error while running Tauri application");
}
```

### 2.4 API 端点

服务器提供以下 REST API 端点：

| 端点 | 方法 | 功能 |
|------|------|------|
| `/health` | GET | 健康检查 |
| `/api/chat` | POST | 聊天对话（支持流式） |
| `/api/sessions` | GET/POST/DELETE | 会话管理 |
| `/api/ingest` | POST | 导入文档到知识库 |
| `/api/search` | GET | 搜索知识库 |
| `/api/config` | GET/PUT | 配置管理 |
| `/api/config/wizard` | POST | 配置向导 |
| `/api/skills` | GET | 技能列表 |

---

## 3. 系统初始化流程

### 3.1 服务器启动流程

当 Tauri 应用启动时，按以下步骤初始化：

```
┌─────────────────────────────────────────────────────────────┐
│  1. 初始化日志系统                                            │
│     • 配置日志级别和格式                                      │
│     • 设置文件日志输出                                        │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  2. 加载配置 (TianyanConfig)                                  │
│     • 从配置文件加载                                          │
│     • 支持环境变量覆盖                                        │
│     • 提供默认配置作为回退                                    │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  3. 启动 Axum 服务器 (后台线程)                               │
│     • 创建 API 路由器                                         │
│     • 配置 CORS                                               │
│     • 设置静态文件服务 (gui/dist)                             │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  4. 健康检查                                                  │
│     • 轮询检查 /health 端点                                    │
│     • 超时时间：30 秒                                          │
│     • 检查间隔：500 毫秒                                       │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  5. 启动 Tauri 应用                                           │
│     • 创建主窗口                                              │
│     • 加载 Yew 前端资源                                        │
│     • 启用开发者工具（可选）                                  │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 核心组件初始化

系统采用**应用层统一初始化 VFS**的架构设计，确保整个应用生命周期内只有一个 VFS 实例（单一事实来源）。

#### 3.2.1 架构设计原则

```
┌─────────────────────────────────────────────────────────┐
│  应用启动层 (server/src/lib.rs)                         │
│  - create_app()                                         │
│    - 1. initialize_vfs_for_app() ← 单一实例             │
│    - 2. AppState::new(config, vfs)                      │
│    - 3. 创建 Router                                     │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│  应用状态层 (server/src/api/chat.rs - AppState)         │
│  - 持有：MemoryCoordinator, SessionManager, Agent, VFS  │
│  - 所有组件共享同一个 VFS 实例                            │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│  组件构建层 (core_bridge.rs, chat.rs)                   │
│  - build_agent(vfs, memory_coordinator)                 │
│  - build_memory_components(vfs)                         │
│  - 不再创建 VFS，只接收已创建的 VFS                      │
└─────────────────────────────────────────────────────────┘
```

**核心原则**：
- **单一事实来源**：整个应用只有一个 VFS 实例
- **依赖注入**：VFS 从应用层注入到各组件
- **职责分离**：应用启动层负责创建 VFS，组件构建层只接收 VFS
- **生命周期管理**：VFS 生命周期与 AppState 一致

#### 3.2.2 应用启动层初始化

```rust
// server/src/lib.rs

/// 应用级 VFS 初始化
/// 
/// 创建并初始化单一的 VFS 实例，供整个应用使用
fn initialize_vfs_for_app(
    config: &tianyan::config::TianyanConfig
) -> anyhow::Result<Arc<dyn tianyan::storage::VirtualFileSystem>> {
    use tianyan::storage::{LocalStorageBackend, QdrantVectorStore, VirtualFileSystemBuilder, VectorStorage};
    use tianyan::model::{ModelRouter, ModelConfig, ModelProvider};
    use std::collections::HashMap;
    
    // 1. 创建存储后端
    let storage = Arc::new(LocalStorageBackend::new(config.storage.clone()));
    let vector_storage = Arc::new(QdrantVectorStore::new(&config.storage)?);
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            vector_storage.initialize().await
        })
    })?;

    // 2. 创建 ModelRouter 并注册服务（用于嵌入服务）
    let mut model_configs: HashMap<String, ModelConfig> = HashMap::new();
    for service_config in &config.models.services {
        if !service_config.enabled {
            continue;
        }
        let provider = match service_config.service_type {
            tianyan::config::ModelServiceType::OpenAI => ModelProvider::OpenAI,
            tianyan::config::ModelServiceType::Custom => ModelProvider::OpenAICompatible,
        };
        let api_key = service_config.api_key.clone().unwrap_or_default();
        let model_cfg = ModelConfig::new(provider, &api_key)
            .with_base_url(&service_config.endpoint)
            .with_chat_model(&service_config.default_model)
            .with_timeout(service_config.timeout)
            .with_priority(service_config.priority);
        model_configs.insert(service_config.name.clone(), model_cfg);
    }

    let mut model_router = ModelRouter::with_defaults();
    model_router.register_services(model_configs).await
        .map_err(|e| anyhow::anyhow!("Failed to register model services: {}", e))?;

    let model_router = Arc::new(model_router);
    let embedding_service: Arc<dyn tianyan::model::EmbeddingService> = model_router.clone();
    let embedding_model = config.models.default_embedding_model.clone();

    // 3. 初始化 VFS（单一实例）
    let vfs = VirtualFileSystemBuilder::new()
        .with_storage(storage)
        .with_vector_storage(vector_storage)
        .with_config(config.storage.clone())
        .with_embedding_service(embedding_service, embedding_model)
        .build()
        .map_err(|e| anyhow::anyhow!("VFS 构建失败：{}", e))?;
    
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            vfs.initialize().await
        })
    })?;
    
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            tianyan::storage::ensure_vfs_structure(&vfs).await
        })
    })?;
    
    Ok(Arc::new(vfs) as Arc<dyn tianyan::storage::VirtualFileSystem>)
}

fn create_app(config: tianyan::config::TianyanConfig) -> anyhow::Result<(Router, Arc<AppState>)> {
    // 在应用层初始化 VFS（单一实例）
    let vfs = initialize_vfs_for_app(&config)?;
    
    // Create shared application state（传入 VFS）
    let state = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            AppState::new(config, vfs).await
        })
    })?;
    
    // ... 创建 Router ...
    Ok((router, state))
}
```

#### 3.2.3 应用状态管理

AppState 持有所有核心组件的引用，包括 VFS：

```rust
// server/src/api/app_state.rs
pub struct AppState {
    agent: Arc<RwLock<Arc<dyn tianyan::agent::AgentCoordinator>>>,
    config: Arc<RwLock<tianyan::config::TianyanConfig>>,
    memory_coordinator: Arc<dyn tianyan::memory::MemoryCoordinator>,
    session_manager: Arc<dyn tianyan::memory::SessionManager>,
    pending_tasks: Arc<Semaphore>,
    /// 虚拟文件系统（所有组件共享）
    vfs: Arc<dyn tianyan::storage::VirtualFileSystem>,
}

impl AppState {
    pub async fn new(
        config: tianyan::config::TianyanConfig,
        vfs: Arc<dyn tianyan::storage::VirtualFileSystem>,
    ) -> anyhow::Result<Self> {
        // 使用传入的 VFS 构建记忆组件
        let (memory_coordinator, session_manager) = 
            Self::build_memory_components(&config, vfs.clone()).await?;
        
        // 使用传入的 VFS 构建 Agent（内部实现，不再依赖 core_bridge::build_agent）
        let agent = Self::build_agent_or_wizard(&config, memory_coordinator.clone(), vfs.clone()).await?;
        
        Ok(Self {
            agent: Arc::new(RwLock::new(agent)),
            config: Arc::new(RwLock::new(config)),
            memory_coordinator,
            session_manager,
            pending_tasks: Arc::new(Semaphore::new(1000)),
            vfs,
        })
    }
}
```

#### 3.2.4 模块职责划分

**重构后**（新架构）：

| 模块 | 职责 | 关键内容 |
|------|------|----------|
| `app_state.rs` | 组件生命周期管理 | AppState、WizardModeAgent、try_build_agent() |
| `chat.rs` | HTTP 路由处理 | chat_handler、chat_stream_handler、routes() |
| `core_bridge.rs` | 类型转换和适配 | convert_message、convert_token_usage、create_model_client |

**优势**：
- ✅ 职责清晰，每个模块专注于单一职责
- ✅ 减少耦合，core_bridge 不再包含业务构建逻辑
- ✅ 代码更简洁，总代码量减少 8%
- ✅ 调用链更短（2 层 vs 3 层）

### 3.3 虚拟文件系统 (VFS) 初始化

VFS 在**应用启动层**（`server/src/lib.rs`）统一初始化，作为单一实例通过依赖注入传递给各组件。

#### 3.3.1 初始化流程

```
┌─────────────────────────────────────────────────────────┐
│  应用启动：create_app(config)                            │
│  (server/src/lib.rs)                                     │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│  1. initialize_vfs_for_app(&config)                      │
│     • 创建 LocalStorageBackend（文件存储）               │
│     • 创建 QdrantVectorStore（向量存储）                 │
│     • 初始化 ModelRouter（提供嵌入服务）                 │
│     • 使用 VirtualFileSystemBuilder 构建 VFS             │
│     • 调用 vfs.initialize() 初始化                       │
│     • 调用 ensure_vfs_structure() 创建目录结构           │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│  2. AppState::new(config, vfs)                          │
│     (server/src/api/chat.rs)                             │
│     • 接收已创建的 VFS 实例                               │
│     • 调用 build_memory_components(vfs)                  │
│     • 调用 build_agent_or_wizard(vfs, ...)               │
│     • 存储 VFS 引用到 AppState.vfs 字段                   │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│  3. 组件共享 VFS                                         │
│     • MemoryCoordinator → VfsBackedMemoryStore(vfs)     │
│     • SessionManager → PersistentSessionManager(vfs)    │
│     • Agent → AgentBuilder.with_vfs(vfs)                │
│     • 所有组件使用同一个 VFS 实例                         │
└─────────────────────────────────────────────────────────┘
```

#### 3.3.2 关键设计变更

**重构前**（旧架构）：
- ❌ VFS 在 `build_memory_components()` 和 `build_agent()` 中重复初始化
- ❌ 违反"单一事实来源"原则
- ❌ 资源浪费，潜在状态不一致

**重构后**（新架构）：
- ✅ VFS 在应用启动层统一初始化
- ✅ 通过参数传递（依赖注入）给各组件
- ✅ 确保单一实例，状态一致
- ✅ 职责清晰：应用层创建，组件层使用

#### 3.3.3 VFS Trait 接口

VFS 提供统一的存储抽象接口，支持内容读写和向量检索：

```rust
// core/src/storage/traits.rs
#[async_trait]
pub trait VirtualFileSystem: Send + Sync {
    /// 初始化 VFS（创建必要的内部结构）
    async fn initialize(&self) -> Result<()>;
    
    /// 检查条目是否存在
    async fn exists(&self, uri: &TianyanUri) -> Result<bool>;
    
    /// 创建目录
    async fn create_directory(&self, uri: &TianyanUri) -> Result<()>;
    
    /// 读取内容
    async fn read_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String>;
    
    /// 写入内容（自动为 Abstract/Overview 生成向量）
    async fn write_content(&self, uri: &TianyanUri, level: ContentLevel, content: &str) -> Result<()>;
    
    /// 删除条目
    async fn delete_entry(&self, uri: &TianyanUri) -> Result<()>;
    
    /// 列出目录内容
    async fn list_directory(&self, uri: &TianyanUri) -> Result<Vec<TianyanUri>>;
    
    /// 获取向量存储（供 Agent 使用）
    fn get_vector_storage(&self) -> Arc<dyn VectorStorage>;
}
```

#### 3.3.4 嵌入服务集成

VFS 通过构建器模式集成嵌入服务，实现自动向量生成：

```rust
// core/src/storage/vfs.rs
pub struct VirtualFileSystemBuilder {
    config: Option<StorageConfig>,
    storage: Option<Arc<dyn StorageBackend>>,
    vector_storage: Option<Arc<dyn VectorStorage>>,
    embedding_service: Option<Arc<dyn EmbeddingService>>,
    embedding_model: Option<String>,
}

impl VirtualFileSystemBuilder {
    /// 设置嵌入服务
    pub fn with_embedding_service(
        mut self,
        service: Arc<dyn EmbeddingService>,
        model: impl Into<String>,
    ) -> Self {
        self.embedding_service = Some(service);
        self.embedding_model = Some(model.into());
        self
    }

    /// 构建虚拟文件系统
    pub fn build(self) -> Result<VirtualFileSystemImpl> {
        let mut vfs = VirtualFileSystemImpl::new(storage, vector_storage, config);
        
        if let (Some(service), Some(model)) = (self.embedding_service, self.embedding_model) {
            vfs.set_embedding_service(service, model);
        }

        Ok(vfs)
    }
}
```

#### 3.3.5 自动向量生成

当写入 Abstract 或 Overview 层级内容时，VFS 自动生成向量：

```rust
// core/src/storage/vfs.rs
async fn write_content(
    &self,
    uri: &TianyanUri,
    level: ContentLevel,
    content: &str,
) -> Result<()> {
    // 写入内容到存储
    self.storage.write_content(uri, level, content).await?;

    // 为 Abstract 和 Overview 层级生成向量
    if matches!(level, ContentLevel::Abstract | ContentLevel::Overview) {
        if let Err(e) = self.generate_and_store_embedding(uri, level, content).await {
            tracing::warn!(
                "向量生成失败但不影响内容写入: {} {:?} - {}",
                uri, level, e
            );
        }
    }

    Ok(())
}
```

### 3.4 VFS 目录结构

```
tianyan://
├── user/               # 用户数据
│   └── profile/        # 用户档案
│       └── basic_info/
│           ├── .abstract.md      # 摘要层
│           ├── .overview.md      # 概览层
│           └── content.md        # 详情层（Markdown 格式）
├── memory/             # 记忆数据（系统自动管理）
│   ├── sessions/       # 会话历史
│   │   └── session_001/
│   │       ├── .abstract.md
│   │       ├── .overview.md
│   │       └── content.json      # 详情层（JSON 格式）
│   └── long_term/      # 长期记忆
│       └── preference/
│           └── pref_001/
│               ├── .abstract.md
│               ├── .overview.md
│               └── content.json  # 详情层（JSON 格式）
├── knowledge/          # 知识库（用户导入的文档）
│   └── documents/
│       └── api_spec/
│           ├── .abstract.md
│           ├── .overview.md
│           └── content.md        # 详情层（Markdown 格式）
└── agent/              # Agent 配置
    ├── soul/           # 核心提示词 (System Prompt)
    │   ├── .abstract.md
    │   ├── .overview.md
    │   └── content.md
    ├── config/         # Agent 配置
    └── skills/         # 自定义技能
```

#### 3.4.1 文件结构说明

每个条目目录包含以下文件：

| 文件 | 说明 | 向量化 |
|------|------|--------|
| `.abstract.md` | 摘要层内容（~100 tokens），用于快速筛选 | ✓ |
| `.overview.md` | 概览层内容（~2K tokens），用于导航和重排序 | ✓ |
| `content.md/json` | 详情层内容（无限制），完整内容 | ✗ |

**元数据存储**：所有条目的元数据统一存储在 Qdrant 向量数据库的 payload 中，不再生成独立的元数据文件。

#### 3.4.2 元数据字段

每个条目的元数据包含以下字段（存储在 Qdrant payload 中）：

| 字段 | 类型 | 说明 | 示例 |
|------|------|------|------|
| `uri` | String | 条目的唯一标识符 | `tianyan://memory/long_term/preference/pref_001` |
| `category` | String | 主分类 | `memory`, `knowledge`, `user` |
| `sub_category` | Option\<String\> | 子分类 | `preference`, `decision`, `documents` |
| `entry_type` | String | 条目类型 | `file`, `directory` |
| `is_directory` | bool | 是否为目录 | `false` |
| `content_type` | String | 内容类型 | `unknown`, `markdown`, `json` |
| `source` | ContentSource | 内容来源 | `user_upload`, `agent_generated`, `external_import` |
| `original_name` | Option\<String\> | 原始文件名 | `document.pdf` |
| `file_size` | Option\<u64\> | 文件大小（字节） | `2048` |
| `importance` | f32 | 重要性评分 (0.0-1.0) | `0.85` |
| `tags` | Vec\<String\> | 标签列表 | `["rust", "backend"]` |
| `created_at` | DateTime\<Utc\> | 创建时间 | `2024-03-01T00:00:00Z` |
| `updated_at` | DateTime\<Utc\> | 更新时间 | `2024-03-02T00:00:00Z` |
| `custom` | HashMap\<String, Value\> | 自定义字段 | `{"decay_factor": 0.95}` |

**注意**：已移除的字段：
- ~~`last_accessed`~~ - 最后访问时间（已移除，意义不大）
- ~~`access_count`~~ - 访问次数（已移除，意义不大）

---

## 4. 模型路由系统

模型路由器负责智能选择服务，支持故障转移和健康检查。

### 4.1 服务类型

```
┌─────────────────────────────────────────────────────────────┐
│                    ModelRouter                               │
├─────────────────────────────────────────────────────────────┤
│  支持的服务类型：                                               │
│  • OpenAI (原生)                                              │
│  • OpenAI-Compatible (自定义端点)                             │
├─────────────────────────────────────────────────────────────┤
│  路由策略：                                                    │
│  1. 检查任务类型的显式路由规则                                  │
│  2. 匹配模型提示 (model_hint)                                 │
│  3. 使用默认服务                                              │
│  4. 选择第一个可用服务                                        │
├─────────────────────────────────────────────────────────────┤
│  功能：                                                        │
│  • 自动故障转移 (auto_failover)                               │
│  • 健康检查 (health_check)                                    │
│  • 聊天/嵌入/VLM 服务路由                                     │
└─────────────────────────────────────────────────────────────┘
```

### 4.2 路由决策流程

```rust
// core/src/model/router.rs
pub async fn route(
    &self,
    task_type: TaskType,
    model_hint: Option<&str>,
) -> Result<RoutingDecision> {
    // 1. 检查显式路由规则
    if let Some(rule) = self.config.routing_rules.get(&task_type) {
        if let Some(entry) = services.get(&rule.preferred_service) {
            if entry.status.available {
                return Ok(RoutingDecision {
                    service_name: rule.preferred_service.clone(),
                    model: rule.preferred_model.clone(),
                    fallbacks: rule.fallbacks.clone(),
                    reason: format!("任务类型 {:?} 的路由规则", task_type),
                });
            }
        }
        
        // 尝试备选服务
        for (service_name, model) in &rule.fallbacks {
            // ...
        }
    }
    
    // 2. 如果提供了模型提示则使用
    if let Some(hint) = model_hint {
        // 尝试找到拥有此模型的服务
    }
    
    // 3. 使用默认服务
    // 4. 找到任何可用的服务
}
```

### 4.3 故障转移机制

```rust
// 当主服务失败时，自动尝试备选服务
if self.config.auto_failover && !decision.fallbacks.is_empty() {
    for (fallback_service, fallback_model) in decision.fallbacks {
        match fallback_svc.chat_completion(fallback_request).await {
            Ok(response) => return Ok(response),
            Err(fe) => {
                warn!("备选服务 {} 失败：{}", fallback_service, fe);
            }
        }
    }
}
```

---

## 5. 双层检索流程 (L0 + L1)

这是系统的核心知识检索机制，采用两阶段检索策略。

### 5.1 检索流程概览

```
┌─────────────────────────────────────────────────────────────┐
│  用户查询："如何使用 Rust 的异步特性？"                          │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  步骤 1: 意图分析 (IntentAnalyzer)                            │
│  • 提取查询向量                                               │
│  • 识别类别过滤条件                                           │
│  • 估算 Token 数量                                            │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  步骤 2: 查询向量生成 (EmbeddingService)                       │
│  • 调用嵌入服务生成查询向量                                    │
│  • 使用配置的 default_embedding_model                         │
│  • 向量维度由配置决定（默认 1536）                             │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  步骤 3: L0 搜索 - 快速过滤 (摘要向量)                         │
│  • 搜索 Abstract 向量空间                                     │
│  • 返回 top_k × multiplier 个候选 (默认 3 倍)                 │
│  • 最低分数阈值：0.3                                          │
│  • 应用类别过滤                                               │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  步骤 4: L1 搜索 - 精确重排序 (概览向量)                        │
│  • 在 Overview 向量空间中搜索                                 │
│  • 仅保留 L0 候选集中的结果                                    │
│  • 最低分数阈值：0.5                                          │
│  • 返回最终 top_k 结果                                        │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  步骤 5: 内容加载 (ContentLoader)                             │
│  • 根据分数选择内容层级：                                       │
│    - 高分 (≥0.8): 加载 Detail 层级                            │
│    - 中分 (0.5-0.8): 加载 Overview 层级                       │
│    - 低分 (<0.5): 加载 Abstract 层级                          │
│  • 支持 Token 预算控制                                        │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  步骤 6: 生成检索追踪 (RetrievalTrace)                         │
│  • 记录检索耗时                                               │
│  • 记录 Token 使用量                                          │
│  • 记录检索来源 URI                                           │
└─────────────────────────────────────────────────────────────┘
```

### 5.2 向量嵌入生成流程

系统在写入 Abstract 和 Overview 层级时自动生成向量并存储到 Qdrant：

```
┌─────────────────────────────────────────────────────────────┐
│  VFS.write_content(uri, level, content)                      │
└─────────────────────────────────────────────────────────────┘
                              │
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  判断内容层级                                                  │
│  • Abstract → 生成向量                                        │
│  • Overview → 生成向量                                        │
│  • Detail → 跳过（不生成向量）                                 │
└─────────────────────────────────────────────────────────────┘
                              │
                              ↓ (Abstract/Overview)
┌─────────────────────────────────────────────────────────────┐
│  调用 EmbeddingService.embed_single(model, content)           │
│  • 使用配置的 default_embedding_model                         │
│  • 生成与模型维度匹配的向量                                    │
└─────────────────────────────────────────────────────────────┘
                              │
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  存储向量到 Qdrant                                            │
│  • 点 ID: URI 转换为合法 ID                                   │
│  • 向量存储在命名向量中：                                      │
│    - "abstract" 向量用于 L0 检索                              │
│    - "overview" 向量用于 L1 检索                              │
│  • 负载包含 URI、类别、重要性等元数据                          │
└─────────────────────────────────────────────────────────────┘
```

### 5.3 嵌入服务配置

嵌入服务通过配置文件设置：

```toml
[models]
default_embedding_model = "text-embedding-3-small"  # 嵌入模型

[storage.vector]
url = "http://localhost:6333"           # Qdrant 服务地址
collection_name = "tianyan"              # 集合名称
vector_dimension = 1536                  # 向量维度
```

### 5.4 核心检索代码

```rust
// core/src/context/retrieval/retriever.rs
pub async fn retrieve(&self, query: &str, top_k: usize) -> Result<Vec<RetrievalResult>> {
    let mut trace_builder = RetrievalTraceBuilder::new(query);
    
    // 步骤 1：意图分析
    let intent = self.analyze_intent(query).await?;
    trace_builder.add_intent_analysis(intent.token_count);
    
    // 步骤 2：获取查询向量
    let query_vector = self.get_query_vector(&intent).await?;
    
    // 步骤 3：L0 搜索（快速过滤）
    let candidates = self.l0_search(&query_vector, top_k, &intent).await?;
    for candidate in &candidates {
        trace_builder.add_l0_search(candidate.uri.clone(), candidate.score, 0);
    }
    
    // 步骤 4：L1 搜索（精确重排序）
    let refined = self.l1_search(&query_vector, candidates, top_k).await?;
    for result in &refined {
        trace_builder.add_l1_search(result.uri.clone(), result.score, 0);
    }
    
    // 步骤 5：加载内容
    let results = self.load_content_for_results(refined).await?;
    
    // 构建最终追踪记录
    let trace = trace_builder.build();
    info!(
        query = %query,
        total_time_ms = trace.total_time_ms,
        total_tokens = trace.total_tokens,
        result_count = results.len(),
        "Retrieval completed"
    );
    
    Ok(results)
}
```

### 5.3 L0 搜索实现

```rust
async fn l0_search(
    &self,
    query_vector: &[f32],
    top_k: usize,
    intent: &Intent,
) -> Result<Vec<VectorSearchResult>> {
    let candidate_count = top_k * self.l0_candidate_multiplier; // 默认 3 倍
    
    let query = VectorSearchQuery {
        vector: query_vector.to_vec(),
        vector_type: VectorType::Abstract,  // 使用摘要向量
        limit: candidate_count,
        category_filter: intent.category_filter().map(|s| s.to_string()),
        min_score: Some(self.l0_min_score), // 默认 0.3
    };
    
    let results = self.vector_storage.search(query).await?;
    Ok(results)
}
```

### 5.4 L1 搜索实现

```rust
async fn l1_search(
    &self,
    query_vector: &[f32],
    candidates: Vec<VectorSearchResult>,
    top_k: usize,
) -> Result<Vec<RetrievalResult>> {
    // 提取候选 URI 用于过滤
    let candidate_uris: Vec<String> = candidates.iter()
        .map(|c| c.uri.to_string())
        .collect();
    
    // 在概览向量空间中搜索
    let query = VectorSearchQuery {
        vector: query_vector.to_vec(),
        vector_type: VectorType::Overview,  // 使用概览向量
        limit: top_k,
        category_filter: None, // 已在 L0 中过滤
        min_score: Some(self.l1_min_score), // 默认 0.5
    };
    
    let results = self.vector_storage.search(query).await?;
    
    // 过滤仅包含来自 L0 的候选
    let refined: Vec<RetrievalResult> = results
        .into_iter()
        .filter(|r| candidate_uris.contains(&r.uri.to_string()))
        .map(RetrievalResult::from_search_result)
        .take(top_k)
        .collect();
    
    Ok(refined)
}
```

---

## 6. ReAct 智能体循环

天演采用 **ReAct (Reasoning + Acting)** 模式实现智能体循环，支持主动检索和工具执行。

### 6.1 ReAct 循环流程

```
┌─────────────────────────────────────────────────────────────┐
│  用户输入："帮我查找关于 Rust 异步编程的资料"                   │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  Thought (思考)                                              │
│  • 分析用户意图                                               │
│  • 判断是否需要检索或工具调用                                 │
│  • 生成推理步骤                                               │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  Action (行动)                                               │
│  • 选择行动类型：                                             │
│    - Retrieve: 检索知识库                                    │
│    - Skill: 调用技能                                        │
│    - Answer: 直接回答                                        │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  Observation (观察)                                          │
│  • 收集行动结果                                               │
│  • 检索结果 / 技能执行结果                                    │
│  • 流式输出观察内容                                           │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  循环判断                                                    │
│  • 如果已获取足够信息 → 生成最终回答                           │
│  • 如果需要更多信息 → 返回 Thought 继续循环                     │
└─────────────────────────────────────────────────────────────┘
```

### 6.2 ReAct 执行器实现

```rust
// core/src/agent/react.rs
pub async fn execute_react_loop(
    &self,
    user_input: &str,
    session_id: &str,
) -> Result<AgentResponse> {
    let mut conversation_history = self.load_history(session_id).await?;
    let mut max_iterations = self.config.max_react_iterations;
    
    while max_iterations > 0 {
        // 1. Thought: 分析当前状态
        let thought = self.generate_thought(
            user_input,
            &conversation_history,
        ).await?;
        
        // 2. Action: 决定行动
        match thought.action {
            Action::Retrieve { query } => {
                // 执行检索
                let results = self.retriever.retrieve(&query, 5).await?;
                conversation_history.push_assistant_thought(&thought, Some(&results));
            }
            Action::Skill { skill_id, params } => {
                // 执行技能
                let result = self.skill_executor.execute(&skill_id, &params).await?;
                conversation_history.push_assistant_thought(&thought, Some(&result));
            }
            Action::Answer { content } => {
                // 生成最终回答
                return Ok(AgentResponse::new(content));
            }
        }
        
        max_iterations -= 1;
    }
    
    Err(TianyanError::AgentError("ReAct loop exceeded max iterations".into()))
}
```

### 6.3 流式输出支持

```rust
// core/src/agent/coordinator.rs
pub async fn chat_stream(
    &self,
    input: &str,
    session_id: &str,
) -> Result<impl Stream<Item = StreamEvent>> {
    let (tx, rx) = mpsc::channel(100);
    
    // 启动异步任务处理流式事件
    tokio::spawn(async move {
        // 1. 发送思考事件
        tx.send(StreamEvent::Thinking("正在分析您的问题...".into())).await.ok();
        
        // 2. 执行检索
        let retrieval_results = self.retriever.retrieve(input, 5).await?;
        tx.send(StreamEvent::RetrievalComplete(retrieval_results.clone())).await.ok();
        
        // 3. 构建上下文并调用模型
        let context = self.build_context(&retrieval_results);
        let response = self.model_service.chat_completion(context).await?;
        
        // 4. 流式输出模型响应
        for chunk in response.chunks {
            tx.send(StreamEvent::ContentChunk(chunk)).await.ok();
        }
        
        // 5. 完成
        tx.send(StreamEvent::Complete).await.ok();
    });
    
    Ok(rx)
}
```

---

## 7. 技能执行系统

技能系统提供可扩展的工具执行能力。

### 7.1 技能架构

```
┌─────────────────────────────────────────────────────────────┐
│                    SkillExecutor                             │
├─────────────────────────────────────────────────────────────┤
│  内置技能：                                                   │
│  • file_read: 读取文件内容                                    │
│  • file_write: 写入文件                                       │
│  • http_get: HTTP GET 请求                                    │
│  • http_post: HTTP POST 请求                                  │
│  • shell_exec: 执行系统命令                                   │
│  • web_search: 网络搜索                                       │
├─────────────────────────────────────────────────────────────┤
│  技能注册表：                                                 │
│  • 从 VFS 加载自定义技能                                      │
│  • 支持动态注册/注销                                          │
├─────────────────────────────────────────────────────────────┤
│  安全检查：                                                   │
│  • 权限验证                                                   │
│  • 资源访问限制                                               │
│  • 命令白名单                                                 │
└─────────────────────────────────────────────────────────────┘
```

### 7.2 技能定义格式

```rust
// core/src/skills/mod.rs
#[derive(Debug, Clone)]
pub struct SkillDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub parameters: Vec<SkillParameter>,
    pub return_type: String,
    pub security_level: SecurityLevel,
}

#[derive(Debug, Clone)]
pub struct SkillParameter {
    pub name: String,
    pub param_type: String,
    pub required: bool,
    pub description: String,
}
```

### 7.3 技能执行流程

```rust
// core/src/skills/executor.rs
pub async fn execute(
    &self,
    request: SkillExecutionRequest,
) -> Result<SkillExecutionResult> {
    // 1. 查找技能
    let skill = self.registry.get(&request.skill_id)
        .ok_or_else(|| TianyanError::SkillNotFound(request.skill_id.clone()))?;
    
    // 2. 安全检查
    if !self.security.check_permission(&skill.security_level).await? {
        return Err(TianyanError::SecurityError(
            "Insufficient permissions".into()
        ));
    }
    
    // 3. 参数验证
    self.validate_parameters(&skill.parameters, &request.parameters)?;
    
    // 4. 执行技能
    let result = match request.skill_id.as_str() {
        "file_read" => self.execute_file_read(&request.parameters).await?,
        "file_write" => self.execute_file_write(&request.parameters).await?,
        "http_get" => self.execute_http_get(&request.parameters).await?,
        "shell_exec" => self.execute_shell_exec(&request.parameters).await?,
        _ => {
            // 尝试执行自定义技能
            self.execute_custom_skill(&request).await?
        }
    };
    
    Ok(SkillExecutionResult {
        success: true,
        output: Some(result),
        error: None,
    })
}
```

### 7.4 技能调用语法

模型通过以下格式调用技能：

```
[SKILL:skill_id(param1=value1, param2=value2)]
```

示例：
```
[SKILL:file_read(path="/path/to/file.txt")]
[SKILL:http_get(url="https://api.example.com", headers={"Authorization": "Bearer token"})]
[SKILL:shell_exec(command="ls -la", timeout=30)]
```

---

## 8. 存储架构

本节详细描述 VFS、本地存储、向量存储和 Session 之间的架构关系。

### 8.1 存储层次架构

系统采用分层存储架构，从底层到上层依次为：**本地文件存储** → **向量存储** → **虚拟文件系统 (VFS)** → **业务模块 (Session 等)**。

```
┌─────────────────────────────────────────────────────────────┐
│                    业务层 (Business Layer)                    │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │  SessionManager / MemoryCoordinator / Agent             │ │
│  │  • PersistentSessionManager                             │ │
│  │  • VfsBackedMemoryStore                                 │ │
│  │  • AgentCoordinator                                     │ │
│  └─────────────────────────────────────────────────────────┘ │
│                            ↓ VFS API                         │
├─────────────────────────────────────────────────────────────┤
│                虚拟文件系统层 (VFS Layer)                      │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │  VirtualFileSystem (Trait)                              │ │
│  │  • write_content(uri, content)                          │ │
│  │  • read_content(uri, level)                             │ │
│  │  • append_content(uri, content)                         │ │
│  │  • update_metadata(uri, importance, custom)             │ │
│  │  • create_directory / delete / list / search            │ │
│  └─────────────────────────────────────────────────────────┘ │
│                            ↓                                 │
│  ┌───────────────────────┐    ┌───────────────────────────┐ │
│  │  StorageBackend       │    │  VectorStorage            │ │
│  │  (本地文件存储)        │    │  (Qdrant 向量存储)        │ │
│  └───────────────────────┘    └───────────────────────────┘ │
│                            ↓                                 │
├─────────────────────────────────────────────────────────────┤
│                    物理存储层 (Physical Layer)                │
│  ┌───────────────────────┐    ┌───────────────────────────┐ │
│  │  文件系统              │    │  Qdrant 服务              │ │
│  │  ./data/              │    │  http://localhost:6333    │ │
│  └───────────────────────┘    └───────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

### 8.2 核心组件职责

#### 8.2.1 StorageBackend (本地文件存储)

**职责**：提供底层文件 I/O 操作，管理文件系统上的目录和文件。

**核心方法**：
```rust
// core/src/storage/traits.rs
#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn initialize(&self) -> Result<()>;
    async fn exists(&self, uri: &TianyanUri) -> Result<bool>;
    async fn read_entry(&self, uri: &TianyanUri) -> Result<ContextEntry>;
    async fn write_entry(&self, entry: &ContextEntry) -> Result<()>;
    async fn delete_entry(&self, uri: &TianyanUri) -> Result<()>;
    async fn list_directory(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>>;
    async fn read_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String>;
    async fn write_content(&self, uri: &TianyanUri, level: ContentLevel, content: &str) -> Result<()>;
    async fn append_content(&self, uri: &TianyanUri, level: ContentLevel, content: &str) -> Result<()>;
}
```

**实现**：`LocalStorageBackend` (core/src/storage/local.rs)
- 将 `TianyanUri` 映射到文件系统路径
- 支持三层内容存储：Abstract (`.abstract.md`)、Overview (`.overview.md`)、Detail (`content.md/json`)
- 提供真正的文件追加操作

#### 8.2.2 VectorStorage (向量存储)

**职责**：存储和检索向量嵌入，支持语义搜索。

**核心方法**：
```rust
// core/src/storage/traits.rs
#[async_trait]
pub trait VectorStorage: Send + Sync {
    async fn initialize(&self) -> Result<()>;
    async fn upsert_point(&self, point: &VectorPoint) -> Result<()>;
    async fn delete_point(&self, id: &str) -> Result<()>;
    async fn search(&self, query: VectorSearchQuery) -> Result<Vec<VectorSearchResult>>;
    async fn get_point(&self, id: &str) -> Result<Option<VectorPoint>>;
    async fn update_vector(&self, uri: &TianyanUri, vector_type: VectorType, vector: &[f32]) -> Result<()>;
    async fn search_fused(...) -> Result<Vec<VectorSearchResult>>;  // RRF 融合搜索
}
```

**实现**：`QdrantVectorStore` (core/src/storage/qdrant.rs)
- 使用 Qdrant 向量数据库
- 支持多命名向量：`abstract`、`overview`、`visual`
- 支持 RRF (Reciprocal Rank Fusion) 融合搜索

**VectorPayload 结构**：
```rust
pub struct VectorPayload {
    pub uri: String,
    pub category: String,
    pub sub_category: Option<String>,
    pub entry_type: String,
    pub is_directory: bool,
    pub content_type: String,
    pub source: ContentSource,
    pub importance: f32,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub custom: HashMap<String, Value>,  // 业务自定义元数据
}
```

#### 8.2.3 VirtualFileSystem (虚拟文件系统)

**职责**：统一存储抽象，协调文件存储和向量存储，提供单一入口点。

**核心方法**：
```rust
// core/src/storage/traits.rs
#[async_trait]
pub trait VirtualFileSystem: Send + Sync {
    async fn initialize(&self) -> Result<()>;
    async fn exists(&self, uri: &TianyanUri) -> Result<bool>;
    async fn create_directory(&self, uri: &TianyanUri) -> Result<ContextEntry>;
    async fn write_content(&self, uri: &TianyanUri, content: &str) -> Result<()>;
    async fn read_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String>;
    async fn append_content(&self, uri: &TianyanUri, content: &str) -> Result<()>;
    async fn update_metadata(&self, uri: &TianyanUri, importance: f32, custom: HashMap<String, Value>) -> Result<()>;
    async fn delete(&self, uri: &TianyanUri) -> Result<()>;
    async fn list(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>>;
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;
    
    /// 按类型搜索（重构后新增）
    async fn search_by_type(&self, type_: ContextType, query: &str, limit: usize) -> Result<Vec<SearchResult>>;
    async fn search_session(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;
    async fn search_memory(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;
    async fn search_knowledge(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;
    async fn search_skill(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;
    
    fn storage_backend(&self) -> &dyn StorageBackend;
    fn vector_storage(&self) -> &dyn VectorStorage;
}
```

**实现**：`VirtualFileSystemImpl` (core/src/storage/vfs.rs)

**关键设计决策**：
1. **简化接口**：`write_content` 和 `append_content` 固定写入 Detail 层级，移除 `level` 参数
2. **单一入口**：`update_metadata()` 是更新向量库元数据的唯一入口
3. **职责分离**：VFS 只负责文件 I/O，业务模块负责构建和更新元数据
4. **类型化搜索**：新增 `search_by_type()` 方法，支持按 Session/Memory/Knowledge/Skill 类型搜索

### 8.3 Session 与 VFS 的交互

#### 8.3.1 PersistentSessionManager 架构

```
┌─────────────────────────────────────────────────────────────┐
│                  PersistentSessionManager                    │
├─────────────────────────────────────────────────────────────┤
│  字段:                                                       │
│  - vfs: Arc<dyn VirtualFileSystem>                          │
│  - inner: Arc<InMemorySessionManager>                       │
│  - summary_generator: Option<Arc<dyn SummaryGenerator>>     │
├─────────────────────────────────────────────────────────────┤
│  操作流程:                                                    │
│                                                              │
│  create_session():                                           │
│    1. inner.create_session() → Session                       │
│    2. vfs.create_directory(&session.uri())                   │
│    3. vfs.update_metadata(uri, importance, custom)           │
│                                                              │
│  add_message():                                              │
│    1. inner.add_message(session_id, message)                 │
│    2. vfs.append_content(uri, jsonl_line)  // 追加到文件     │
│    3. vfs.update_metadata(uri, importance, custom)           │
│                                                              │
│  update_session():                                           │
│    1. inner.update_session(session)                          │
│    2. vfs.update_metadata(uri, importance, custom)           │
└─────────────────────────────────────────────────────────────┘
```

#### 8.3.2 Session 元数据存储

Session 的元数据通过 `build_session_custom()` 构建并存储到向量库：

```rust
fn build_session_custom(&self, session: &Session) -> HashMap<String, serde_json::Value> {
    let mut custom = HashMap::new();
    custom.insert("session_id".to_string(), serde_json::json!(session.session_id));
    custom.insert("ended_at".to_string(), serde_json::json!(session.ended_at));
    custom.insert("key_info".to_string(), serde_json::to_value(&session.key_info).unwrap());
    custom.insert("title".to_string(), serde_json::json!(session.title));
    custom.insert("metadata".to_string(), serde_json::to_value(&session.metadata).unwrap());
    custom.insert("message_count".to_string(), serde_json::json!(session.messages.len()));
    custom
}
```

### 8.4 数据流示例

#### 8.4.1 添加消息到 Session

```
用户发送消息: "你好"
                    │
                    ↓
┌─────────────────────────────────────────────────────────────┐
│  PersistentSessionManager.add_message(session_id, message)   │
└─────────────────────────────────────────────────────────────┘
                    │
        ┌───────────┴───────────┐
        ↓                       ↓
┌───────────────────┐   ┌───────────────────────────────────┐
│ 内存更新           │   │ VFS 持久化                         │
│ inner.add_message │   │                                   │
└───────────────────┘   │ 1. 构建 JSONL 行                   │
                        │    record.to_jsonl_line()          │
                        │                                   │
                        │ 2. 追加到文件                       │
                        │    vfs.append_content(uri, line)   │
                        │    → StorageBackend.append_content │
                        │    → 文件系统追加写入               │
                        │                                   │
                        │ 3. 更新向量库元数据                 │
                        │    vfs.update_metadata(uri, ...)   │
                        │    → VectorStorage.upsert_point    │
                        │    → Qdrant 更新 payload           │
                        └───────────────────────────────────┘
```

#### 8.4.2 检索 Session

```
检索请求: "关于 Rust 的对话"
                    │
                    ↓
┌─────────────────────────────────────────────────────────────┐
│  Retriever.retrieve(query)                                   │
└─────────────────────────────────────────────────────────────┘
                    │
                    ↓
┌─────────────────────────────────────────────────────────────┐
│  1. 生成查询向量                                              │
│     embedding_service.embed_single(query)                    │
└─────────────────────────────────────────────────────────────┘
                    │
                    ↓
┌─────────────────────────────────────────────────────────────┐
│  2. L0 + L1 双层检索                                          │
│     vector_storage.search_fused(query_vector, ...)           │
│     → Qdrant RRF 融合搜索                                     │
│     → 返回 VectorSearchResult (含 payload.custom)            │
└─────────────────────────────────────────────────────────────┘
                    │
                    ↓
┌─────────────────────────────────────────────────────────────┐
│  3. 加载内容                                                  │
│     vfs.read_content(uri, ContentLevel::Detail)              │
│     → StorageBackend.read_content                            │
│     → 从文件系统读取 JSONL                                    │
└─────────────────────────────────────────────────────────────┘
```

### 8.5 存储设计原则

#### 8.5.1 单一事实来源

- **VFS 是唯一存储入口**：所有业务模块通过 VFS 访问存储，不直接操作 StorageBackend 或 VectorStorage
- **单一实例**：应用启动时创建一个 VFS 实例，通过依赖注入传递给所有组件

#### 8.5.2 职责分离

| 组件 | 职责 | 不负责 |
|------|------|--------|
| StorageBackend | 文件 I/O | 向量操作、业务逻辑 |
| VectorStorage | 向量存储和检索 | 文件 I/O、业务逻辑 |
| VirtualFileSystem | 协调文件和向量存储 | 业务逻辑 |
| SessionManager | 会话生命周期管理 | 文件 I/O 细节 |

#### 8.5.3 元数据双层存储

```
┌─────────────────────────────────────────────────────────────┐
│                    元数据存储架构                            │
├─────────────────────────────────────────────────────────────┤
│  向量库 Payload (Qdrant)                                     │
│  • 检索相关字段：uri, category, tags, importance             │
│  • 业务自定义字段：custom HashMap                             │
│  • 用途：向量检索、元数据过滤、排序                           │
├─────────────────────────────────────────────────────────────┤
│  文件系统 (Detail 层)                                         │
│  • 完整内容：JSONL 消息记录                                   │
│  • 用途：内容展示、业务逻辑处理                               │
└─────────────────────────────────────────────────────────────┘
```

**设计优势**：
1. 检索高效：元数据与向量一起存储，支持复杂过滤
2. 数据完整：文件系统存储完整内容，便于备份迁移
3. 架构清晰：没有额外的元数据文件同步问题

### 8.6 关键文件索引

| 文件路径 | 说明 |
|---------|------|
| `core/src/storage/traits.rs` | 存储 trait 定义 (StorageBackend, VectorStorage, VirtualFileSystem) |
| `core/src/storage/vfs.rs` | VirtualFileSystem 实现 |
| `core/src/storage/local.rs` | LocalStorageBackend 实现 |
| `core/src/storage/qdrant.rs` | QdrantVectorStore 实现 |
| `core/src/memory/session.rs` | PersistentSessionManager 实现 |
| `core/src/memory/vfs_store.rs` | VfsBackedMemoryStore 实现 |

---

## 9. 记忆管理系统

记忆系统负责对话历史的自动提取、重要性评估和持久化。

### 9.1 会话状态管理（SessionState）

**SessionState** 是统一管理会话所有状态的核心容器，于 2026-03 引入，用于支持 Planner-Executor 架构。

#### 9.1.1 SessionState 结构

```rust
// core/src/agent/session_state.rs
pub struct SessionState {
    /// 会话 ID
    pub session_id: String,
    
    /// 对话历史（用户和助手的问答）
    pub conversation: Vec<Message>,
    
    /// 执行历史（Planner-Executor 循环的结果）
    pub execution_context: ContextManager,
    
    /// 当前目标
    pub current_goal: Option<String>,
    
    /// 待处理的追问（如果有）
    pub pending_clarification: Option<Vec<ClarificationQuestion>>,
    
    /// 最后活动时间
    pub last_activity: Instant,
}
```

**核心职责**：
- **对话历史管理**：存储用户和助手的完整对话历史
- **执行上下文管理**：记录 Planner-Executor 的每轮迭代结果（计划 + 执行结果）
- **目标追踪**：维护当前任务目标，支持多轮迭代
- **追问处理**：暂存需要用户澄清的问题
- **状态清理**：自动清理过期状态，防止内存泄漏

#### 9.1.2 SessionState 数据流

```
用户输入 → SessionState.add_user_message()
              ↓
        Planner.run(input, state)
              ↓
        生成 Plan (DirectAnswer / Clarification / Steps)
              ↓
        Executor.execute_steps(steps)
              ↓
        SessionState.add_turn(plan, results)
              ↓
        更新 execution_context
              ↓
        下一轮迭代 或 返回 DirectAnswer
```

#### 9.1.3 使用示例

```rust
// 创建会话状态
let mut state = SessionState::new("session-001");

// 添加对话历史
state.add_user_message("帮我分析项目性能问题");
state.add_assistant_message("好的，让我先查看项目结构");

// Planner 执行一轮迭代
let plan = Plan::Steps(vec![
    Step {
        step_id: 1,
        description: "读取 Cargo.toml".to_string(),
        action: Action::ReadFile { path: "Cargo.toml".to_string() },
        expected_importance: 0.8,
        on_failure: FailureHandling::Ignore,
    },
]);

let results = vec![StepResult {
    step_id: 1,
    success: true,
    output: json!({"content": "[package]\nname = \"my-project\""}),
    error: None,
    actual_importance: Some(0.85),
}];

// 更新执行历史
state.add_turn(plan, results);

// 构建 Prompt 上下文
let prompt = state.build_prompt_context("继续分析");
// prompt 包含：
// - 对话历史（2 条消息）
// - 执行历史（1 轮，包含步骤执行结果）
// - 当前输入（"继续分析"）
```

#### 9.1.4 与 SessionManager 的关系

```
SessionStateManager (内存中)
    └─ SessionState (会话状态容器)
        ├─ conversation: Vec<Message> (对话历史)
        ├─ execution_context: ContextManager (执行历史)
        └─ current_goal: Option<String> (当前目标)

PersistentSessionManager (VFS 持久化)
    └─ Session (会话数据结构)
        └─ messages: Vec<Message> (消息列表)
```

**关键区别**：
- **SessionState**：内存中的运行时状态，包含执行上下文，支持 Planner-Executor 循环
- **Session**：VFS 持久化的会话数据，仅包含消息历史，用于长期存储

### 9.2 记忆架构（重构后）

**重构目标**：统一 VFS 接口，简化架构层次，明确职责边界。

#### 9.1.1 重构前的问题

```
┌─────────────────────────────────────────────────────────────┐
│                    重构前架构                                │
├─────────────────────────────────────────────────────────────┤
│  Agent → MemoryCoordinator → VfsBackedMemoryStore → VFS     │
│             ↓                                                │
│         SessionManager ──────────────────────────────┘       │
│                                                              │
│  问题：                                                      │
│  • VfsBackedMemoryStore 重复实现 VFS 功能（832 行冗余代码）   │
│  • MemoryCoordinator 绕过 VFS 统一接口                        │
│  • Session 包含不应该有的字段（importance, summary）          │
│  • 职责混乱：存储、摘要、缓存管理混在一起                     │
└─────────────────────────────────────────────────────────────┘
```

#### 9.1.2 重构后架构

```
┌─────────────────────────────────────────────────────────────┐
│                    重构后架构                                │
├─────────────────────────────────────────────────────────────┤
│  Agent → ReAct 循环 → 工具调用 → VFS (统一接口)              │
│                      ↓                                       │
│            ┌─────────┴─────────┐                            │
│            ↓                   ↓                             │
│     Retriever           SummaryService                       │
│     (检索工具)               ↓                                │
│                      MemoryExtractor                         │
│                                                              │
│  优势：                                                      │
│  • VFS 是唯一存储入口，符合"单一事实来源"原则                 │
│  • 职责清晰：VFS 存储、SummaryService 摘要、MemoryExtractor 提取  │
│  • 代码量减少 72%（从 2530 行减少到 ~700 行）                  │
└─────────────────────────────────────────────────────────────┘
```

#### 9.1.3 核心架构优化

**1. 统一 VFS 接口**
- 所有存储操作通过 VFS，不再绕过
- 新增类型化搜索：`search_by_type()` 支持按 Session/Memory/Knowledge/Skill 搜索
- VFS 成为真正的"单一事实来源"

**2. 简化 MemoryStore**
- trait 方法从 10 个减少到 3 个：`store`、`get`、`delete`
- 删除 `VfsBackedMemoryStore`（832 行冗余代码）
- 不再手动管理缓存和生成摘要

**3. 精简 Session 结构**
- 移除 `importance`、`metadata` 字段
- 这些字段由 VFS 的元数据系统统一管理
- Session 只关注对话历史本身

**4. 新增 MemoryExtractor 服务**
- 从 Session 自动提取记忆
- 异步执行，不阻塞主流程
- 集成到 SummaryService 定时扫描

**5. ReAct 集成**
- Agent 通过 ReAct 循环调用工具
- Retriever 工具使用 VFS 的类型化搜索
- Memory 检索是被动的，由模型决定何时检索

### 9.2 记忆持久化架构

系统采用 **虚拟文件系统 (VFS)** 作为长期记忆的存储后端，实现跨会话持久化。

#### 9.2.1 VFS 存储结构

VFS 采用三层内容层级存储，每个条目包含三个详细层级：

```
tianyan://memory/
├── long_term/
│   ├── preference/        # 用户偏好记忆
│   │   ├── pref_session_001/
│   │   │   ├── .abstract.md      # L0: 摘要层 (~100 tokens)
│   │   │   ├── .overview.md      # L1: 概览层 (~2K tokens)
│   │   │   └── content.json      # L2: 详情层 (无限制，自动选择格式)
│   │   └── pref_session_002/
│   ├── decision/          # 重要决策记忆
│   │   └── dec_session_001/
│   ├── successful_case/   # 成功案例
│   ├── failed_case/       # 失败案例
│   ├── pattern/           # 模式记忆
│   ├── entity/            # 实体记忆
│   └── fact/              # 事实记忆
└── sessions/              # 会话历史
    └── session_001/
        ├── .abstract.md
        ├── .overview.md
        └── content.json      # 会话使用 JSON 格式
```

#### 9.2.2 三层内容层级说明

| 层级 | 文件名 | Token 数量 | 用途 | 向量化 |
|------|--------|-----------|------|--------|
| L0 (Abstract) | `.abstract.md` | ~100 | 快速筛选，向量检索 | ✓ |
| L1 (Overview) | `.overview.md` | ~2K | 导航，重排序 | ✓ |
| L2 (Detail) | `content.json/md` | 无限制 | 完整内容 | ✗ |

#### 9.2.3 Detail 层格式自动选择

Detail 层根据 URI 类别自动选择存储格式：

| URI 类别 | 路径前缀 | 格式 | 说明 |
|---------|---------|------|------|
| Memory | `memory/sessions/` | JSON | 会话数据结构化存储 |
| Memory | `memory/long_term/` | JSON | 长期记忆结构化存储 |
| Knowledge | `knowledge/` | Markdown | 文档内容便于阅读 |
| 其他 | - | Markdown | 默认格式 |

#### 9.2.4 记忆条目数据结构

每个记忆条目存储为 JSON 文件，元数据存储在 Qdrant 向量数据库的 payload 中：

**Detail 层内容文件** (`content.json`):
```json
{
  "id": "pref_session_001",
  "uri": "tianyan://memory/long_term/preference/pref_session_001",
  "content": "用户偏好使用 Rust 进行后端开发",
  "category": "Preference",
  "importance": 0.85,
  "created_at": 1709251200,
  "updated_at": 1709337600,
  "tags": ["rust", "programming", "backend"]
}
```

**向量库 Payload** (存储在 Qdrant):
```json
{
  "uri": "tianyan://memory/long_term/preference/pref_session_001",
  "category": "memory",
  "sub_category": "preference",
  "entry_type": "file",
  "is_directory": false,
  "content_type": "unknown",
  "source": "agent_generated",
  "original_name": null,
  "file_size": 256,
  "importance": 0.85,
  "tags": ["rust", "programming", "backend"],
  "created_at": "2024-03-01T00:00:00Z",
  "updated_at": "2024-03-02T00:00:00Z",
  "custom": {
    "decay_factor": 0.95
  }
}
```

**说明**：
- **Detail 层内容文件**：存储实际的内容数据（JSON 格式），包含业务逻辑需要的字段
- **向量库 Payload**：存储用于检索和过滤的元数据，支持基于标签、分类、重要性等的搜索
- 两个存储通过 `uri` 字段关联

#### 9.2.5 简化的 MemoryStore

重构后删除了 `VfsBackedMemoryStore`（832 行），使用简化的 `MemoryStore` trait：

**核心设计**：
- **职责单一**：只负责存储和检索，不处理摘要生成
- **接口精简**：3 个方法（`store`、`get`、`delete`）
- **依赖 VFS**：所有操作通过 VFS，不再有自己的缓存和摘要逻辑

**与重构前的对比**：

| 功能 | 重构前 | 重构后 |
|------|--------|--------|
| 缓存管理 | MemoryStore 自己管理 | 依赖 VFS 缓存 |
| 摘要生成 | MemoryStore 手动生成 | SummaryService 自动生成 |
| 搜索功能 | MemoryStore 提供多个搜索方法 | VFS 统一提供 `search_by_type()` |
| 代码量 | 832 行 | ~100 行 |

### 9.3 MemoryExtractor 服务（重构后新增）

系统新增 `MemoryExtractor` 服务，负责从 Session 自动提取记忆并存储到 VFS。

#### 9.3.1 服务架构

```
┌─────────────────────────────────────────────────────────────┐
│              MemoryExtractor 架构                            │
├─────────────────────────────────────────────────────────────┤
│  输入：Session 内容（JSON 格式对话历史）                       │
│          ↓                                                   │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  LLM 分析（使用 ModelService）                          │  │
│  │  • 提取用户偏好                                         │  │
│  │  • 识别重要决策                                         │  │
│  │  • 发现关键事实                                         │  │
│  └───────────────────────────────────────────────────────┘  │
│          ↓                                                   │
│  输出：MemoryEntry 列表（结构化记忆数据）                     │
│          ↓                                                   │
│  存储：通过 VFS 写入 memory:// 路径                           │
└─────────────────────────────────────────────────────────────┘
```

#### 9.3.2 集成到 SummaryService

`MemoryExtractor` 集成到 `SummaryService` 中，在处理 Session 时异步触发：

```
┌─────────────────────────────────────────────────────────────┐
│         SummaryService.process_uri() 流程                    │
├─────────────────────────────────────────────────────────────┤
│  处理 Session URI                                            │
│          ↓                                                   │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  1. 生成 Session 摘要（Abstract/Overview）               │  │
│  │     • 使用 SummaryEngine                               │  │
│  │     • 同步执行                                         │  │
│  └───────────────────────────────────────────────────────┘  │
│          ↓                                                   │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  2. 异步触发 MemoryExtractor                           │  │
│  │     • tokio::spawn 异步执行                            │  │
│  │     • 不阻塞主流程                                     │  │
│  │     • 失败仅记录警告                                   │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

**关键设计**：
1. **异步执行**：使用 `tokio::spawn` 异步提取记忆，不阻塞 SummaryService 主流程
2. **错误容忍**：提取失败仅记录警告，不影响摘要生成
3. **灵活配置**：通过可选字段支持按需启用记忆提取功能
4. **LLM 驱动**：使用模型分析对话内容，提取结构化记忆

#### 9.3.3 记忆提取流程

```
用户对话 → Session 追加 → SummaryService 定时扫描
                                    ↓
                            ┌───────┴───────┐
                            ↓               ↓
                    生成 Session 摘要    异步触发 MemoryExtractor
                            ↓               ↓
                    VFS: Abstract    LLM 分析对话内容
                    VFS: Overview          ↓
                            ↓         提取记忆条目
                            ↓               ↓
                            └───────┬───────┘
                                    ↓
                            VFS: memory:// 路径
```

### 9.4 异步持久化流程

系统在聊天请求完成后，通过异步任务处理记忆持久化，避免阻塞主流程。

#### 9.3.1 异步持久化触发

```rust
// server/src/api/chat.rs
async fn chat_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ChatRequest>,
) -> Json<ChatResponse> {
    // ... 处理聊天请求 ...
    
    // 在后台异步持久化记忆
    let memory_coordinator = state.memory_coordinator();
    let session_manager = state.session_manager();
    let session_id_clone = session_id.clone();
    
    tokio::spawn(async move {
        if let Err(e) = persist_memory_async(
            memory_coordinator, 
            session_manager, 
            &session_id_clone
        ).await {
            error!("异步记忆持久化失败：{} - {}", session_id_clone, e);
        }
    });
    
    Json(chat_response)
}
```

#### 9.3.2 持久化处理流程

```rust
// server/src/core_bridge.rs
pub async fn persist_memory_async(
    memory_coordinator: Arc<dyn MemoryCoordinator>,
    session_manager: Arc<dyn SessionManager>,
    session_id: &str,
) -> anyhow::Result<()> {
    // 1. 获取会话
    match session_manager.get_session(session_id).await {
        Ok(Some(session)) => {
            // 2. 分析会话
            match memory_coordinator.process_session(&session).await {
                Ok(analysis) => {
                    // 3. 存储记忆
                    match memory_coordinator.store_memories(&analysis, session_id).await {
                        Ok(memories) => {
                            info!(
                                "会话记忆持久化成功：session={}, 存储记忆数={}",
                                session_id,
                                memories.len()
                            );
                            Ok(())
                        }
                        Err(e) => {
                            error!("存储记忆失败：{} - {}", session_id, e);
                            Err(anyhow::anyhow!("存储记忆失败：{}", e))
                        }
                    }
                }
                Err(e) => {
                    error!("处理会话失败：{} - {}", session_id, e);
                    Err(anyhow::anyhow!("处理会话失败：{}", e))
                }
            }
        }
        Ok(None) => {
            warn!("会话不存在，跳过持久化：{}", session_id);
            Ok(())
        }
        Err(e) => {
            error!("获取会话失败：{} - {}", session_id, e);
            Err(anyhow::anyhow!("获取会话失败：{}", e))
        }
    }
}
```

#### 9.3.3 持久化步骤详解

```
┌─────────────────────────────────────────────────────────────┐
│  步骤 1: 获取会话历史                                         │
│  • 从 SessionManager 加载完整会话                            │
│  • 包含所有用户和助手消息                                   │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  步骤 2: 分析会话内容                                         │
│  • 提取用户偏好（Preferences）                               │
│  • 提取重要决策（Decisions）                                 │
│  • 生成会话摘要（Summary）                                   │
│  • 计算重要性分数（Importance Score）                        │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  步骤 3: 记忆分类与合并                                       │
│  • 对记忆条目进行分类（Preference/Decision/Fact 等）         │
│  • 检测并合并重复记忆                                        │
│  • 更新相关记忆的关联关系                                   │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  步骤 4: 持久化到 VFS                                         │
│  • 序列化为 JSON 格式                                         │
│  • 写入对应的类别目录                                        │
│  • 更新内存缓存（可选）                                      │
└─────────────────────────────────────────────────────────────┘
```

### 9.4 优雅关闭机制

系统在关闭时等待所有待处理的记忆持久化任务完成，确保数据完整性。

#### 9.4.1 优雅关闭流程

```rust
// server/src/api/chat.rs
impl AppState {
    /// 优雅关闭应用
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        use tokio::time::{timeout, Duration};
        
        tracing::info!("开始关闭应用...");
        
        let max_wait = Duration::from_secs(30);
        
        match timeout(max_wait, self.wait_for_pending_tasks()).await {
            Ok(_) => {
                tracing::info!("所有任务已完成，应用已关闭");
                Ok(())
            }
            Err(_) => {
                tracing::warn!("关闭超时 ({}s)，仍有任务未完成", max_wait.as_secs());
                Ok(())
            }
        }
    }
    
    /// 等待所有待处理任务完成
    async fn wait_for_pending_tasks(&self) {
        let initial_available = self.pending_tasks.available_permits();
        let total_permits = 1000;
        let pending_count = total_permits - initial_available;
        
        if pending_count == 0 {
            tracing::debug!("没有待处理任务");
            return;
        }
        
        tracing::info!("等待 {} 个待处理任务完成...", pending_count);
        
        while self.pending_tasks.available_permits() < total_permits {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        
        tracing::info!("所有待处理任务已完成");
    }
}
```

#### 9.4.2 信号量控制

```rust
// server/src/api/chat.rs
pub struct AppState {
    /// 待处理任务信号量（控制并发数）
    pending_tasks: Arc<Semaphore>,
}

impl AppState {
    pub async fn new(config: TianyanConfig) -> anyhow::Result<Self> {
        // ...
        let pending_tasks = Arc::new(Semaphore::new(1000));
        // ...
    }
}
```

### 9.5 记忆处理流程

```rust
// core/src/memory/coordinator.rs
pub async fn process_session(
    &self,
    session: &Session,
) -> Result<MemoryAnalysis> {
    // 1. 提取关键信息
    let key_info = self.key_info_extractor.extract(session).await?;
    
    // 2. 生成摘要
    let summary = self.session_summarizer.summarize(session).await?;
    
    // 3. 评估会话重要性
    let importance = if session.messages.len() > 10 {
        0.8
    } else if session.messages.len() > 5 {
        0.6
    } else {
        0.4
    };
    
    // 4. 构建分析结果
    let analysis = SessionAnalysis {
        preferences: key_info.preferences,
        decisions: key_info.decisions,
        entities: key_info.entities,
        summary,
        importance,
    };
    
    Ok(analysis)
}
```

### 9.6 记忆存储

```rust
// core/src/memory/coordinator.rs
pub async fn store_memories(
    &self,
    analysis: &SessionAnalysis,
    session_id: &str,
) -> Result<Vec<MemoryEntry>> {
    let mut stored_entries = Vec::new();
    
    // 1. 存储偏好
    for pref in &analysis.preferences {
        let content = format!("{}: {}", pref.key, pref.value);
        let category = self.classifier.classify(&content).await?;
        let id = format!("pref_{}_{}", session_id, stored_entries.len());
        
        let mut entry = MemoryEntry::new(&id, &content, category);
        entry.importance = pref.confidence;
        entry.source_session = Some(session_id.to_string());
        
        // 2. 检查相似记忆
        let existing = self.memory_store.list_all().await?;
        let similar_indices = self.consolidator.find_similar(&entry, &existing);
        
        if similar_indices.is_empty() {
            // 没有相似记忆，直接存储
            self.memory_store.store(&entry).await?;
            stored_entries.push(entry);
        } else {
            // 合并相似记忆
            let merged = self.consolidator.merge(&entry, &existing[similar_indices[0]]);
            self.memory_store.update(&merged).await?;
            stored_entries.push(merged);
        }
    }
    
    // 3. 存储决定
    for dec in &analysis.decisions {
        let content = if let Some(ref rationale) = dec.rationale {
            format!("{} (理由：{})", dec.description, rationale)
        } else {
            dec.description.clone()
        };
        let id = format!("dec_{}_{}", session_id, stored_entries.len());
        
        let mut entry = MemoryEntry::new(&id, &content, MemoryCategory::Decision);
        entry.importance = dec.importance;
        entry.source_session = Some(session_id.to_string());
        
        self.memory_store.store(&entry).await?;
        stored_entries.push(entry);
    }
    
    Ok(stored_entries)
}
```

### 9.7 记忆检索

```rust
// core/src/memory/coordinator.rs
pub async fn retrieve_memories(
    &self,
    query: &str,
    limit: usize,
) -> Result<Vec<MemoryEntry>> {
    // 1. 按内容搜索
    let mut results = self.memory_store.search_by_content(query).await?;
    
    // 2. 按有效重要性排序（考虑时间衰减）
    results.sort_by(|a, b| {
        b.effective_importance()
            .partial_cmp(&a.effective_importance())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    
    // 3. 限制结果数量
    results.truncate(limit);
    
    // 4. 记录访问
    for entry in &results {
        self.access_tracker.record_access(&entry.id).await?;
    }
    
    Ok(results)
}
```

### 9.8 元数据双层存储设计

系统采用**元数据双层存储**策略，平衡检索性能和数据完整性：

```
┌─────────────────────────────────────────────────────────────┐
│                    元数据存储架构                            │
├─────────────────────────────────────────────────────────────┤
│  第一层：Qdrant 向量数据库 (Payload)                          │
│  • 存储：检索相关字段（uri, category, tags, importance 等）   │
│  • 用途：向量检索、元数据过滤、排序                           │
│  • 优势：与向量数据一起查询，性能高                           │
│  • 字段：12 个标准字段 + custom HashMap                       │
├─────────────────────────────────────────────────────────────┤
│  第二层：文件系统 (Detail 层 JSON 文件)                        │
│  • 存储：业务逻辑字段（id, content, category 等）             │
│  • 用途：内容展示、业务逻辑处理                               │
│  • 优势：独立于向量库，便于备份和迁移                         │
│  • 格式：JSON（结构化数据）或 Markdown（文档）                │
└─────────────────────────────────────────────────────────────┘
```

**设计优势**：
1. **检索高效**：元数据与向量数据一起存储，支持复杂的过滤和排序
2. **架构清晰**：文件系统只包含内容文件，没有额外的元数据文件
3. **数据一致**：避免了文件系统和向量库之间的元数据同步问题
4. **简化代码**：移除了元数据文件的读写逻辑

**已移除的设计**：
- ~~`.metadata.json` 文件~~ - 不再为每个条目生成独立的元数据文件
- ~~`last_accessed` 字段~~ - 访问时间统计意义不大
- ~~`access_count` 字段~~ - 访问次数统计意义不大

---

## 10. 完整数据流图

### 10.1 主数据流

```
┌─────────┐     ┌─────────────┐     ┌─────────────────────────────┐
│  用户输入 │────→│  Yew 前端   │────→│      Axum Server           │
│          │     │  (gui/)     │     │      (server/)             │
└─────────┘     └─────────────┘     └─────────────────────────────┘
                                              │
                                              ↓ HTTP POST /api/chat
                                    ┌─────────────────────────────┐
                                    │      Core Bridge            │
                                    │  (核心桥接层)                │
                                    └─────────────────────────────┘
                                              │
                                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                    Agent 协调器 (ReAct 模式)                      │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  Thought: 分析意图，决定行动                                │  │
│  └───────────────────────────────────────────────────────────┘  │
│                              │                                   │
│              ┌───────────────┼───────────────┐                  │
│              ↓               ↓               ↓                  │
│      ┌──────────────┐ ┌──────────────┐ ┌──────────────┐        │
│      │  检索知识库   │ │  调用技能     │ │  直接回答     │        │
│      └──────────────┘ └──────────────┘ └──────────────┘        │
│              │               │               │                  │
│              └───────────────┴───────────────┘                  │
│                              │                                   │
│                              ↓                                   │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  Observation: 收集结果，流式输出                            │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              │
                ┌─────────────┼─────────────┐
                ↓             ↓             ↓
        ┌───────────┐ ┌───────────┐ ┌───────────┐
        │ 双层检索器 │ │ 技能执行器 │ │ 模型路由器 │
        └───────────┘ └───────────┘ └───────────┘
                │             │             │
                ↓             ↓             ↓
        ┌───────────┐ ┌───────────┐ ┌───────────┐
        │  Qdrant   │ │  文件系统  │ │ OpenAI/  │
        │  向量库    │ │  HTTP 请求  │ │ Compatible│
        └───────────┘ └───────────┘ └───────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │         流式响应 (SSE)                   │
        │  • StreamEvent::Thinking                │
        │  • StreamEvent::RetrievalComplete       │
        │  • StreamEvent::SkillExecuting          │
        │  • StreamEvent::ContentChunk            │
        │  • StreamEvent::Complete                │
        └─────────────────────────────────────────┘
```

### 10.2 Planner-Executor 迭代数据流

```
        ┌─────────────────────────────────────────┐
        │  用户输入："分析项目性能问题"              │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  SessionState.add_user_message()         │
        │  添加用户消息到对话历史                   │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  Planner.run(input, state)               │
        │  第 1 轮迭代                              │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  build_prompt_context()                  │
        │  构建 Prompt：                           │
        │  - 对话历史（2 条消息）                   │
        │  - 执行历史（0 轮）                       │
        │  - 当前输入                              │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  LLM 生成 Plan                            │
        │  Plan::Steps([                           │
        │    Step { action: ReadFile("Cargo.toml") }, │
        │    Step { action: ExecuteCommand("cargo bench") }, │
        │  ])                                      │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  Executor.execute_steps()                │
        │  并行执行 2 个步骤                         │
        │  - 步骤 1: 读取 Cargo.toml → 成功        │
        │  - 步骤 2: 运行 cargo bench → 成功       │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  SessionState.add_turn(plan, results)    │
        │  更新执行历史：                          │
        │  - 添加第 1 轮执行记录                     │
        │  - ContextManager 计算权重               │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  Planner.run() 第 2 轮迭代                │
        │  看到：步骤 1 和 2 的结果                   │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  LLM 生成 Plan                            │
        │  Plan::Steps([                           │
        │    Step { action: SearchCode("slow function") }, │
        │  ])                                      │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  Executor.execute_steps()                │
        │  执行步骤 3：搜索代码                     │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  SessionState.add_turn(plan, results)    │
        │  更新执行历史：第 2 轮                    │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  Planner.run() 第 3 轮迭代                │
        │  看到：所有 3 轮执行结果                    │
        │  决定：信息足够，生成最终报告             │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  LLM 生成 Plan                            │
        │  Plan::DirectAnswer("性能瓶颈在...")      │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  SessionState.add_assistant_message()    │
        │  添加最终回答到对话历史                   │
        │  返回给用户                              │
        └─────────────────────────────────────────┘
```

### 10.3 记忆持久化数据流

```
        ┌─────────────────────────────────────────┐
        │  聊天请求完成                            │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  tokio::spawn 异步任务                   │
        │  (不阻塞主流程)                          │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  SessionManager.get_session()            │
        │  获取会话历史                            │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  MemoryCoordinator.process_session()     │
        │  • 提取关键信息（偏好、决策、实体）      │
        │  • 生成会话摘要                          │
        │  • 计算重要性分数                        │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  MemoryCoordinator.store_memories()      │
        │  • 分类记忆条目                          │
        │  • 检测并合并重复记忆                    │
        │  • 更新关联关系                          │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  VfsBackedMemoryStore.store()            │
        │  • 序列化为 JSON                         │
        │  • 写入 VFS 对应类别目录                  │
        │  • 更新内存缓存                          │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  持久化完成                              │
        │  记录日志：session={}, memories={}       │
        └─────────────────────────────────────────┘
```

### 10.4 优雅关闭流程

```
        ┌─────────────────────────────────────────┐
        │  应用关闭信号                            │
        │  (Ctrl+C / 窗口关闭)                     │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  AppState.shutdown()                     │
        │  • 设置 30 秒超时                          │
        │  • 等待待处理任务完成                    │
        └─────────────────────────────────────────┘
                              │
                              ↓
        ┌─────────────────────────────────────────┐
        │  检查 Semaphore 可用许可                  │
        │  pending_count = 1000 - available        │
        └─────────────────────────────────────────┘
                              │
                    ┌─────────┴─────────┐
                    │                   │
          pending_count > 0    pending_count == 0
                    │                   │
                    ↓                   ↓
        ┌───────────────────┐  ┌───────────────────┐
        │  循环等待任务完成  │  │  直接关闭         │
        │  • sleep(100ms)   │  │  无待处理任务     │
        │  • 检查 permits   │  └───────────────────┘
        └───────────────────┘
                    │
                    ↓ (超时或完成)
        ┌─────────────────────────────────────────┐
        │  关闭完成                                │
        │  • 记录日志                             │
        │  • 释放资源                             │
        └─────────────────────────────────────────┘
```

---

## 11. 配置结构

系统配置存储在 `tianyan.toml` 中。

### 11.1 配置示例

```toml
[models]
default_chat_model = "gpt-4"
default_embedding_model = "text-embedding-3-small"
default_vision_model = "gpt-4-vision-preview"

[[models.services]]
name = "openai"
service_type = "OpenAI"
endpoint = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"  # 支持环境变量
enabled = true
timeout = 60
max_retries = 3

[retrieval]
two_stage_retrieval = true      # 启用 L0+L1 双层检索
max_context_tokens = 4096       # 上下文 Token 上限
l0_multiplier = 3               # L0 候选倍数
min_score = 0.3                 # 最低相似度分数

[memory]
auto_consolidation = true       # 自动记忆合并
importance_threshold = 0.5      # 记忆重要性阈值
auto_decay = true               # 启用时间衰减
decay_half_life_hours = 168     # 半衰期（小时）

[storage]
data_dir = "./data"             # 数据目录
vector_db_url = "http://localhost:6333"  # Qdrant 向量数据库
enable_memory_persistence = true # 启用记忆持久化

[security]
enabled = true                  # 启用安全检查

[logging]
level = "info"                  # 日志级别
format = "text"                 # 日志格式
```

### 11.2 配置加载流程

```rust
// 配置加载优先级（从高到低）：
// 1. 环境变量 (TIANYAN_*)
// 2. 命令行参数 (--data-dir, --log-level)
// 3. 配置文件 (tianyan.toml)
// 4. 默认配置
```

### 11.3 记忆相关配置说明

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `memory.auto_consolidation` | bool | true | 自动合并重复记忆 |
| `memory.importance_threshold` | f32 | 0.5 | 记忆存储的重要性阈值 |
| `memory.auto_decay` | bool | true | 启用时间衰减机制 |
| `memory.decay_half_life_hours` | u32 | 168 | 记忆重要性半衰期（7 天） |
| `storage.enable_memory_persistence` | bool | true | 启用 VFS 持久化存储 |
| `storage.data_dir` | PathBuf | "./data" | 数据存储根目录 |

---

## 附录：核心文件索引

### 核心库 (core/)

| 文件路径 | 说明 |
|---------|------|
| `core/src/agent/coordinator.rs` | Agent 协调器，ReAct 执行流程 |
| `core/src/agent/react.rs` | ReAct 智能体实现 |
| `core/src/model/router.rs` | 模型路由器，支持故障转移 |
| `core/src/context/retrieval/retriever.rs` | 双层检索器 (L0+L1) |
| `core/src/context/retrieval/intent.rs` | 意图分析 |
| `core/src/memory/coordinator.rs` | 记忆协调器 |
| `core/src/memory/vfs_store.rs` | VFS 持久化记忆存储实现 |
| `core/src/memory/long_term.rs` | 长期记忆存储 trait 和默认实现 |
| `core/src/memory/session.rs` | 会话管理和摘要生成 |
| `core/src/memory/decay.rs` | 记忆衰减和清理机制 |
| `core/src/memory/types.rs` | 记忆相关类型定义 |
| `core/src/skills/executor.rs` | 技能执行器 |
| `core/src/storage/vfs.rs` | 虚拟文件系统 |
| `core/src/config/mod.rs` | 配置管理 |

### 服务器 (server/)

| 文件路径 | 说明 |
|---------|------|
| `server/src/lib.rs` | Axum 服务器入口 |
| `server/src/api/app_state.rs` | **新增**：应用状态管理，组件生命周期和构建逻辑 |
| `server/src/api/chat.rs` | 聊天 API，HTTP 路由处理（精简后） |
| `server/src/core_bridge.rs` | 核心桥接层，类型转换和辅助功能（精简后） |
| `server/src/api/sessions.rs` | 会话管理 API |
| `server/src/api/config.rs` | 配置管理 API |
| `server/src/api/config_wizard.rs` | 配置向导 API |
| `server/src/api/ingest.rs` | 文档导入 API |

### Tauri 应用 (tauri/)

| 文件路径 | 说明 |
|---------|------|
| `tauri/src/lib.rs` | Tauri 应用入口 |
| `tauri/src/server.rs` | 服务器启动和生命周期管理 |
| `tauri/tauri.conf.json` | Tauri 配置文件 |

### GUI 前端 (gui/)

| 文件路径 | 说明 |
|---------|------|
| `gui/src/main.rs` | Yew 前端入口 |
| `gui/src/components/chat.rs` | 聊天界面组件 |
| `gui/src/components/config_wizard.rs` | 配置向导组件 |
| `gui/src/api/client.rs` | API 客户端 |
| `gui/src/state/mod.rs` | 状态管理 |

---

*文档版本：3.0*  
*最后更新：2026-03-23*  
*基于 Tauri v2 + Yew + Axum 技术栈*

### 变更历史

#### v3.0 (2026-03-23) - 记忆模块重构

**重构目标**：统一 VFS 接口，简化架构层次，明确职责边界。

**核心优化**：
- ✅ **统一 VFS 接口**：所有存储操作通过 VFS，符合"单一事实来源"原则
- ✅ **简化 MemoryStore**：trait 方法从 10 个减少到 3 个（store、get、delete）
- ✅ **精简 Session 结构**：移除 importance、metadata 字段，由 VFS 元数据系统管理
- ✅ **新增 MemoryExtractor**：从 Session 自动提取记忆，异步执行
- ✅ **扩展 VFS 类型化搜索**：新增 search_by_type() 支持按 Session/Memory/Knowledge/Skill 搜索
- ✅ **简化 AgentCoordinator**：移除 MemoryCoordinator 依赖，所有存储操作通过 VFS

**架构收益**：
| 指标 | 重构前 | 重构后 | 改进 |
|------|--------|--------|------|
| 代码量 | 2530 行 | ~700 行 | -72% |
| MemoryStore 方法 | 10 个 | 3 个 | -70% |
| 存储延迟 | - | < 100ms | 性能目标 |
| 会话追加延迟 | - | < 80ms | 性能目标 |
| 检索延迟 | - | < 150ms | 性能目标 |

**影响文件**：
- 删除：`core/src/memory/vfs_store.rs`（832 行）
- 简化：`memory/long_term.rs`、`memory/session.rs`、`memory/coordinator.rs`
- 新增：`core/src/memory/extractor.rs`（MemoryExtractor 服务）
- 扩展：`core/src/storage/traits.rs`、`core/src/storage/vfs.rs`（类型化搜索）
- 更新：`core/src/agent/coordinator.rs`、`core/src/agent/react.rs`、`server/src/agent_builder.rs`

**测试验证**：
- ✅ `cargo test --workspace`：307 个测试全部通过
- ✅ `cargo clippy --workspace`：仅建议性警告
- ✅ `cargo fmt`：代码已格式化

#### v2.5 (2026-03-21) - 新增存储架构章节

**核心变更**：
- 新增第 8 章「存储架构」，详细描述 VFS、本地存储、向量存储和 Session 之间的架构关系
- 添加存储层次架构图，展示从业务层到物理存储层的数据流
- 详细说明 StorageBackend、VectorStorage、VirtualFileSystem 三大核心组件的职责
- 添加 Session 与 VFS 交互的数据流示例
- 总结存储设计原则：单一事实来源、职责分离、元数据双层存储

**架构要点**：
- VFS 是统一存储入口，协调文件存储和向量存储
- `update_metadata()` 是更新向量库元数据的唯一入口
- `write_content` 和 `append_content` 固定写入 Detail 层级
- Session 通过 `build_session_custom()` 构建元数据并存储到向量库

**影响范围**：
- 目录结构调整：原第 8-10 章顺延为第 9-11 章
- 新增关键文件索引表

#### v2.4 (2026-03-15) - AppState 模块重构与 core_bridge 优化

**核心变更**：
- 新建 `server/src/api/app_state.rs` 模块，包含 AppState 和组件构建逻辑
- 精简 `chat.rs`，从 507 行减少到 ~350 行，只保留 HTTP 路由处理
- 优化 `core_bridge.rs`，从 310 行减少到 ~120 行，移除业务构建逻辑
- 将 `build_agent()` 逻辑迁移到 `app_state.rs::try_build_agent()`
- 将 `WizardModeAgent` 从 `core_bridge.rs` 迁移到 `app_state.rs`
- 将 `validate_config()` 从 `core_bridge.rs` 迁移到 `app_state.rs`
- `core_bridge.rs` 只保留类型转换和辅助功能

**影响范围**：
- `server/src/api/app_state.rs`: **新增**模块，包含 AppState 和组件构建逻辑
- `server/src/api/chat.rs`: 精简 31%，移除组件管理代码
- `server/src/core_bridge.rs`: 精简 61%，移除业务构建逻辑
- `server/src/api/mod.rs`: 添加新模块导出

**架构优势**：
- ✅ 职责清晰：app_state.rs 专注组件管理，chat.rs 专注 HTTP 路由，core_bridge.rs 专注类型转换
- ✅ 减少耦合：core_bridge 不再包含业务构建逻辑，模块间依赖更清晰
- ✅ 代码简洁：总代码量减少 8%，调用链从 3 层减少到 2 层
- ✅ 可维护性提升：模块边界清晰，便于理解和修改

#### v2.3 (2026-03-12) - VFS 初始化架构重构

**核心变更**：
- 将 VFS 初始化提升到应用启动层（`server/src/lib.rs`）
- 采用依赖注入模式，VFS 作为参数传递给各组件
- 确保整个应用只有一个 VFS 实例（单一事实来源）
- 删除 `build_memory_components()` 和 `build_agent()` 中的 VFS 创建逻辑
- 新增 `initialize_vfs_for_app()` 函数统一初始化 VFS
- 更新 `AppState` 结构，持有 VFS 引用

**影响范围**：
- `server/src/lib.rs`: 新增应用级 VFS 初始化函数
- `server/src/api/chat.rs`: AppState 和组件构建函数改用依赖注入
- `server/src/core_bridge.rs`: `build_agent()` 接收 VFS 参数
- `core/src/storage/traits.rs`: VFS trait 新增 `get_vector_storage()` 方法

**架构优势**：
- ✅ 消除重复初始化，符合"单一事实来源"原则
- ✅ 职责清晰：应用层创建，组件层使用
- ✅ 生命周期一致：VFS 与 AppState 共存亡
- ✅ 资源优化：避免多个 VFS 实例占用内存

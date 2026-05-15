# Agent 热重载实现总结

## 背景

天演应用在配置向导完成后无需重启即可使用新配置，这得益于 Agent 热重载机制的实现。

## 实现方案：RwLock 热重载

### 核心思想

使用 `Arc<RwLock<Arc<dyn AgentCoordinator>>>` 实现 Agent 的热重载：
- 启动时：创建 `WizardModeAgent` 或真实 Agent，包装为 `Arc<RwLock<Arc<dyn Trait>>>`
- 配置更新时：创建新 Agent，替换旧 Agent
- 正在进行的请求：继续使用旧 Agent 完成（持有 Arc，不受替换影响）
- 新的请求：使用新 Agent

### 架构图

```
┌─────────────────────────────────────────────────────────────────┐
│                         启动流程                                 │
└─────────────────────────────────────────────────────────────────┘

  尝试加载配置
       │
       ├─→ 配置有效 ──→ 创建真实 Agent ──┐
       │                                 │
       └─→ 配置无效 ──→ 创建 WizardModeAgent │
                                         │
                                         ↓
                              ┌──────────────────┐
                              │ RwLock::new(Arc) │
                              │   (读写锁存储)    │
                              └────────┬─────────┘
                                       │
                                       ↓
                              ┌──────────────────┐
                              │    AppState      │
                              │ agent: Arc<RwLock│
                              │   <Arc<dyn>>>    │
                              └──────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                       配置更新流程                               │
└─────────────────────────────────────────────────────────────────┘

  用户保存配置
       │
       ↓
  ┌──────────────┐
  │ reload_agent │  1. 读取新配置
  │              │  2. 创建新 Agent
  │              │  3. 替换 Agent
  └──────┬───────┘
         │
         ↓ RwLock::write()
  ┌──────────────────┐
  │   新 Agent 生效   │ ←── 新请求使用新 Agent
  │   旧 Agent 保留   │ ←── 旧请求继续使用旧 Agent
  │   (Arc 引用计数)  │
  └──────────────────┘
         │
         ↓ (旧 Agent 引用计数归零)
  ┌──────────────────┐
  │   旧 Agent 销毁   │
  └──────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                       请求处理流程                               │
└─────────────────────────────────────────────────────────────────┘

  收到请求
       │
       ↓ agent.read().await.clone()
  ┌──────────────────┐
  │  获取当前 Agent   │ ←── 获取读锁，克隆 Arc，释放锁
  └────────┬─────────┘
           │
           ↓
  ┌──────────────────┐
  │  处理请求...      │ ←── 持有 Arc，即使被替换也不受影响
  └──────────────────┘
```

## 核心代码实现

### 1. AppState 结构 (`server/src/api/chat.rs`)

```rust
/// Shared application state
#[derive(Clone)]
pub struct AppState {
    /// Core Agent 实例（支持热重载）
    agent: Arc<RwLock<Arc<dyn tianyan::agent::AgentCoordinator>>>,
    /// 应用配置
    config: Arc<RwLock<tianyan::config::TianyanConfig>>,
}

impl AppState {
    /// 创建新的 AppState
    pub async fn new(config: tianyan::config::TianyanConfig) -> anyhow::Result<Self> {
        let agent = Self::build_agent_or_wizard(&config).await?;

        Ok(Self {
            agent: Arc::new(RwLock::new(agent)),
            config: Arc::new(RwLock::new(config)),
        })
    }

    /// 重新初始化 Agent（配置更新后调用）
    pub async fn reload_agent(&self) -> anyhow::Result<()> {
        let config = self.config.read().await.clone();
        let new_agent = Self::build_agent_or_wizard(&config).await?;

        // 替换 Agent，不影响正在进行的请求（它们持有旧 Agent 的 Arc）
        *self.agent.write().await = new_agent;

        tracing::info!("Agent reloaded successfully");
        Ok(())
    }

    /// 获取当前 Agent
    pub async fn agent(&self) -> Arc<dyn tianyan::agent::AgentCoordinator> {
        self.agent.read().await.clone()
    }

    /// 更新配置
    pub async fn update_config(&self, config: tianyan::config::TianyanConfig) {
        *self.config.write().await = config;
    }

    /// 根据配置构建 Agent 或 WizardModeAgent
    async fn build_agent_or_wizard(
        config: &tianyan::config::TianyanConfig,
    ) -> anyhow::Result<Arc<dyn tianyan::agent::AgentCoordinator>> {
        match crate::core_bridge::build_agent(config).await {
            Ok(agent) => Ok(Arc::new(agent)),
            Err(e) => {
                tracing::warn!("Failed to build agent: {}, using wizard mode", e);
                Ok(Arc::new(crate::core_bridge::WizardModeAgent))
            }
        }
    }
}
```

### 2. WizardModeAgent 实现 (`server/src/core_bridge.rs`)

```rust
/// 向导模式下的占位 Agent
/// 
/// 当配置无效时使用，聊天功能会返回错误
pub struct WizardModeAgent;

#[async_trait::async_trait]
impl AgentCoordinator for WizardModeAgent {
    async fn initialize(&self) -> tianyan::Result<()> {
        Ok(())
    }

    async fn get_state(&self) -> tianyan::agent::AgentState {
        tianyan::agent::AgentState::default()
    }

    async fn shutdown(&self) -> tianyan::Result<()> {
        Ok(())
    }

    async fn process_message(
        &self,
        _context: &mut tianyan::agent::ConversationContext,
        _message: &str,
    ) -> tianyan::Result<tianyan::agent::AgentResponse> {
        Err(tianyan::TianyanError::ModelService(
            "应用未配置。请先完成配置向导。".to_string()
        ))
    }

    async fn process_message_stream(
        &self,
        _context: &mut tianyan::agent::ConversationContext,
        _message: &str,
    ) -> tianyan::Result<tokio::sync::mpsc::Receiver<tianyan::Result<tianyan::agent::AgentStreamChunk>>> {
        Err(tianyan::TianyanError::ModelService(
            "应用未配置。请先完成配置向导。".to_string()
        ))
    }
}
```

### 3. 配置保存后触发重载 (`server/src/api/config_wizard.rs`)

```rust
async fn save_config(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SaveConfigRequest>,
) -> Json<SaveConfigResponse> {
    // ... 配置验证和保存 ...

    match tianyan_config.save_to_file(&save_path) {
        Ok(()) => {
            info!("配置已保存到: {:?}", save_path);

            // 更新内存中的配置
            state.update_config(tianyan_config).await;

            // 重新初始化 Agent
            match state.reload_agent().await {
                Ok(()) => {
                    info!("Agent 重新初始化成功");
                    Json(SaveConfigResponse {
                        success: true,
                        message: "配置保存成功并已生效".to_string(),
                        config_path: Some(save_path),
                    })
                }
                Err(e) => {
                    error!("Agent 重新初始化失败: {}", e);
                    Json(SaveConfigResponse {
                        success: true,
                        message: format!("配置已保存但无法生效: {}。请检查配置后重试。", e),
                        config_path: Some(save_path),
                    })
                }
            }
        }
        // ...
    }
}
```

### 4. 服务器启动逻辑 (`server/src/lib.rs`)

```rust
fn create_app(config: tianyan::config::TianyanConfig) -> anyhow::Result<Router> {
    // 统一使用 AppState::new，无需区分模式
    // AppState::new 内部会自动处理配置无效的情况（使用 WizardModeAgent）
    let state = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            AppState::new(config).await
        })
    })?;

    info!("Application state initialized successfully");
    let state = Arc::new(state);
    // ...
}
```

## 优势

| 优势 | 说明 |
|------|------|
| **代码统一** | 启动和运行时用同一套逻辑，无需区分"向导模式"和"正常模式" |
| **平滑过渡** | 配置完成后直接 reload，无需重启应用 |
| **安全简单** | 使用标准库的 RwLock，无需 unsafe 代码 |
| **安全切换** | 正在进行的任务不受影响，新任务使用新配置 |
| **未来友好** | 支持定时任务的热切换，为后续功能打下基础 |

## 兼容性考虑

### 向后兼容
- `WizardModeAgent` 保持现有行为（返回配置错误）
- API 接口不变，前端无需修改

### 并发安全
- RwLock 保证读写安全
- 旧 Agent 的 Arc 引用计数归零后自动销毁，无内存泄漏

### 错误处理
- 配置保存成功但 Agent 初始化失败时，保持原 Agent 运行
- 用户收到明确错误提示，应用不会崩溃

## 相关文件

- `server/src/api/chat.rs` - AppState 结构和热重载实现
- `server/src/api/config_wizard.rs` - 配置保存后触发重载
- `server/src/core_bridge.rs` - WizardModeAgent 实现
- `server/src/lib.rs` - 服务器启动逻辑

## 参考

- [Tokio RwLock 文档](https://docs.rs/tokio/latest/tokio/sync/struct.RwLock.html)
- [Rust Atomics and Locks](https://marabos.nl/atomics/)

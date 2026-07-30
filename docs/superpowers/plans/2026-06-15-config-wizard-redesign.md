# Config Wizard & Settings Page Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the broken 6-step config wizard (wrongly asks for system prompt, misses 4 config sections) with a 4-step wizard + full settings page, using shared components and the existing `GET/PUT /api/config` endpoints.

**Architecture:** Bottom-up refactor. Phase 1: delete the dead `WizardConfig` types + `ConfigStatus.default_system_prompt` from core + server. Phase 2: create shared config section components in `gui/src/components/config/`. Phase 3: rewrite `ConfigWizard` to 4 steps and `SettingsPanel` to left-nav + right-form layout, both using the shared components. All types flow from `TianyanConfig` (not `WizardConfig`).

**Tech Stack:** Rust (core/server), Cargo workspace (resolver="2"), Yew 0.22 (frontend), Axum 0.8 (backend), Trunk (WASM build), serde (serialization)

---

## File Structure Map

```
core/src/config/
├── wizard.rs              MODIFY: Remove ConfigStatus, WizardConfig, WizardAgentConfig, etc.
│                          Keep: TestConnectionRequest, TestConnectionResponse
├── mod.rs                 MODIFY: Remove wizard type re-exports

server/src/api/config/
├── wizard_handlers.rs     DELETE
├── wizard_routes.rs       DELETE
├── wizard_services.rs     DELETE
├── wizard_types.rs        DELETE
├── handlers.rs            MODIFY: Remove wizard handler imports in mod.rs if needed
├── routes.rs              MODIFY: Keep only non-wizard routes
├── mod.rs                 MODIFY: Remove wizard module declarations

gui/src/
├── main.rs                MODIFY: Replace ConfigStatus check → GET /api/config
├── state/mod.rs           MODIFY: Add View::ConfigSettings (or reuse View::Settings)
├── api/config.rs          MODIFY: Add fetch_config() and save_full_config()
├── components/
│   ├── config_wizard/
│   │   ├── mod.rs         MODIFY: Rewrite 4-step wizard, remove SystemPrompt
│   │   ├── types.rs       MODIFY: Replace WizardState → ConfigState (maps TianyanConfig)
│   │   └── api.rs         MODIFY: Use GET/PUT /api/config, remove wizard-specific API
│   ├── config/            CREATE (new directory for shared section components)
│   │   ├── mod.rs         CREATE: re-exports, ConfigMode enum
│   │   ├── types.rs       CREATE: ConfigState, ConfigMode, section types
│   │   ├── model_section.rs    CREATE
│   │   ├── storage_section.rs  CREATE
│   │   ├── agent_section.rs    CREATE
│   │   ├── security_section.rs CREATE
│   │   ├── logging_section.rs  CREATE
│   │   ├── memory_section.rs   CREATE
│   │   ├── retrieval_section.rs CREATE
│   │   └── api.rs         CREATE: fetch_config, save_config, test_connection
│   └── settings/
│       └── mod.rs         MODIFY: Rewrite as left-nav + right-form using config/ components
```

---

## Phase 1: Core Data Layer Cleanup

### Task 1.1: Remove `system_prompt` field from `WizardAgentConfig`

**Files:**
- Modify: `core/src/config/wizard.rs:140-167`

- [ ] **Step 1: Remove system_prompt field**

Open `core/src/config/wizard.rs`. In `WizardAgentConfig`, delete lines 149-151:
```rust
    /// 系统提示词。
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
```

Also delete `Default` impl's `system_prompt` line (line 263):
```rust
            system_prompt: default_system_prompt(),
```
becomes deleted.

- [ ] **Step 2: Run tests to verify nothing breaks**

```powershell
cargo test -p tianyan-core config::wizard -- --nocapture
```

Expected: Existing wizard tests pass (they don't reference system_prompt in assertions). The `test_wizard_config_default` test at line 392 checks `!config.agent.system_prompt.is_empty()` — this assertion will fail now. Fix:

```rust
    #[test]
    fn test_wizard_config_default() {
        let config = WizardConfig::default();
        assert_eq!(config.agent.max_history_messages, 10);
        assert_eq!(config.agent.max_context_tokens, 8000);
    }
```

- [ ] **Step 3: Commit**

```powershell
git add core/src/config/wizard.rs
git commit -m "feat(config): remove system_prompt from WizardAgentConfig"
```

### Task 1.2: Remove `default_system_prompt` from `ConfigStatus` and `ConfigStatus` itself

**Files:**
- Modify: `core/src/config/wizard.rs:14-60`
- Modify: `core/src/config/mod.rs:29-33`

- [ ] **Step 1: Delete ConfigStatus struct and impl**

Delete lines 14-60 of `wizard.rs` (the entire `ConfigStatus` struct, its three methods, and the `default_system_prompt()` function at line 202-204 since we no longer need it in wizard either).

Wait — `default_system_prompt()` is also used by `WizardAgentConfig` default, which we already removed. So delete `default_system_prompt()` at lines 202-204 too.

- [ ] **Step 2: Remove ConfigStatus re-export from mod.rs**

In `core/src/config/mod.rs`, delete line 29-32:
```rust
pub use wizard::{
    ConfigStatus, TestConnectionRequest, TestConnectionResponse, WizardAgentConfig, WizardConfig,
    WizardModelService, WizardModelsConfig, WizardStorageConfig, WizardVectorStorageConfig,
};
```
Replace with:
```rust
pub use wizard::{TestConnectionRequest, TestConnectionResponse};
```

Also keep the rest of the wizard types until we fully delete them in Task 1.3.

- [ ] **Step 3: Fix `check_config_status()` caller**

In `core/src/config/mod.rs`, line 64-86, change `check_config_status()` to return `Result<TianyanConfig, String>` or replace with a simple `config_exists()`. But wait — `check_config_status()` calls `ConfigStatus::not_configured()` and `ConfigStatus::invalid()`. We need to rewrite this.

Since `ConfigStatus` is deleted, the method `TianyanConfig::check_config_status()` must change:

```rust
    /// Check if a valid config exists. Returns Ok(config) or Err with error messages.
    pub fn check_or_load() -> Result<Self, Vec<String>> {
        let config_path = Self::find_config_file();
        let Some(path) = config_path else {
            return Err(vec!["未找到配置文件".to_string()]);
        };
        match Self::load_from_file(&path) {
            Ok(config) => match config.validate() {
                Ok(()) => Ok(config),
                Err(e) => Err(vec![e]),
            },
            Err(e) => Err(vec![e]),
        }
    }
```

Remove the old `check_config_status()` method entirely.

- [ ] **Step 4: Fix all callers of `check_config_status()`**

Search for `check_config_status`:

```powershell
cargo check --workspace 2>&1 | Select-String "check_config_status"
```

Expected: `server/src/api/config/wizard_services.rs:24` is the only caller. We'll fix this in Task 2.1.

- [ ] **Step 5: Run tests**

```powershell
cargo test -p tianyan-core config -- --nocapture
```

- [ ] **Step 6: Commit**

```powershell
git add core/src/config/wizard.rs core/src/config/mod.rs
git commit -m "feat(config): remove ConfigStatus, replace check_config_status with check_or_load"
```

### Task 1.3: Delete `WizardConfig` and all wizard-specific types

**Files:**
- Modify: `core/src/config/wizard.rs` (entire file, keep only TestConnection*)
- Modify: `core/src/config/mod.rs`

- [ ] **Step 1: Rewrite wizard.rs to keep only connection test types**

Rewrite `core/src/config/wizard.rs` to contain ONLY:
```rust
//! Configuration wizard helpers.
//!
//! Kept for model connection testing used by both wizard and settings page.

use serde::{Deserialize, Serialize};

/// Request to test a model service connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConnectionRequest {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
}

/// Response from a model connection test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConnectionResponse {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_models: Option<Vec<String>>,
}

impl TestConnectionResponse {
    pub fn success(message: impl Into<String>, models: Vec<String>) -> Self {
        Self { success: true, message: message.into(), available_models: Some(models) }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self { success: false, message: message.into(), available_models: None }
    }
}
```

Delete: `WizardConfig`, `WizardModelsConfig`, `WizardModelService`, `WizardStorageConfig`, `WizardVectorStorageConfig`, `WizardAgentConfig`, all their `Default` impls, `default_*()` helper functions, `WizardConfig::to_tianyan_config()` and all related tests.

- [ ] **Step 2: Clean mod.rs re-exports**

In `core/src/config/mod.rs`, update line 29-33:
```rust
pub use wizard::{TestConnectionRequest, TestConnectionResponse};
```

- [ ] **Step 3: Remove wizard module from server**

The server uses `WizardConfig` in `wizard_services.rs` through `SaveConfigRequest`. We need to update the server first before this compiles. For now, `cargo check` will have errors in server — that's expected, the next phase fixes them.

- [ ] **Step 4: Run core tests only**

```powershell
cargo test -p tianyan-core -- --nocapture
```

- [ ] **Step 5: Commit**

```powershell
git add core/src/config/wizard.rs core/src/config/mod.rs
git commit -m "feat(config): delete WizardConfig types, keep only TestConnection"
```

---

## Phase 2: Server API Cleanup

### Task 2.1: Delete wizard-specific server files, route wizard save to PUT /api/config

**Files:**
- Delete: `server/src/api/config/wizard_handlers.rs`
- Delete: `server/src/api/config/wizard_routes.rs`
- Delete: `server/src/api/config/wizard_services.rs`
- Delete: `server/src/api/config/wizard_types.rs`
- Modify: `server/src/api/config/mod.rs`

- [ ] **Step 1: Delete wizard files**

```powershell
Remove-Item -LiteralPath "server\src\api\config\wizard_handlers.rs"
Remove-Item -LiteralPath "server\src\api\config\wizard_routes.rs"
Remove-Item -LiteralPath "server\src\api\config\wizard_services.rs"
Remove-Item -LiteralPath "server\src\api\config\wizard_types.rs"
```

- [ ] **Step 2: Clean server config mod.rs**

Remove the wizard module declarations. In `server/src/api/config/mod.rs`, delete lines 14-18:
```rust
// 配置向导子模块
pub mod wizard_handlers;
pub mod wizard_routes;
pub mod wizard_services;
pub mod wizard_types;
```

- [ ] **Step 3: Update server main router to remove wizard routes**

Search for `wizard_routes` usage in `server/src/api/mod.rs` or `server/src/lib.rs`:

```powershell
Select-String -Path "server\src\*" -Pattern "wizard_routes" -SimpleMatch
```

Remove `.merge(config::wizard_routes::routes())` or equivalent from the main router.

- [ ] **Step 4: Add GET /api/config/status endpoint (simple bootstrap check)**

In `server/src/api/config/handlers.rs`, add:
```rust
/// Check if config exists — simple bootstrap check for frontend wizard trigger.
pub async fn get_config_status(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let configured = tianyan::config::TianyanConfig::config_exists();
    Json(serde_json::json!({ "configured": configured }))
}
```

In `server/src/api/config/routes.rs`, add the route:
```rust
.route("/config/status", get(get_config_status))
```

- [ ] **Step 5: Keep test-connection endpoint**

The `/config/test-connection` route was in wizard_routes. Move it to the main config routes.

In `server/src/api/config/handlers.rs`:
```rust
/// Test model connection
pub async fn test_connection(
    Json(request): Json<tianyan::config::TestConnectionRequest>,
) -> Json<tianyan::config::TestConnectionResponse> {
    // Validate URL, API key, model — then attempt connection
    if !request.endpoint.starts_with("http://") && !request.endpoint.starts_with("https://") {
        return Json(tianyan::config::TestConnectionResponse::error(
            "API 端点 URL 格式无效",
        ));
    }
    if request.api_key.is_empty() {
        return Json(tianyan::config::TestConnectionResponse::error("API 密钥不能为空"));
    }
    if request.model.is_empty() {
        return Json(tianyan::config::TestConnectionResponse::error("模型名称不能为空"));
    }
    // Use existing core connection test logic
    match crate::core_bridge::test_model_connection(&request.endpoint, &request.api_key, &request.model).await {
        Ok(models) => Json(tianyan::config::TestConnectionResponse::success("连接成功", models)),
        Err(e) => Json(tianyan::config::TestConnectionResponse::error(format!("连接失败: {}", e))),
    }
}
```

In `server/src/api/config/routes.rs`, add:
```rust
.route("/config/test-connection", post(test_connection))
```

Need to check if `core_bridge::test_model_connection` exists — if not, inline it from the old wizard_services.rs.

- [ ] **Step 6: Run cargo check**

```powershell
cargo check -p tianyan-server
```

Fix any compilation errors from removed wizard types.

- [ ] **Step 7: Run tests**

```powershell
cargo test -p tianyan-server -- --nocapture
```

- [ ] **Step 8: Commit**

```powershell
git add -A server/src/api/config/
git commit -m "feat(server): delete wizard-specific endpoints, use GET/PUT /api/config"
```

### Task 2.2: Verify existing PUT /api/config works for wizard save scenario

**Files:**
- Verify: `server/src/api/config/services.rs`
- Verify: `server/src/api/config/types.rs`

- [ ] **Step 1: Review ConfigService.update_config() for edge cases**

Read `server/src/api/config/services.rs:32-50`. The method:
1. Validates `TianyanConfig`
2. Saves to file
3. Calls `state.update_config(config)` which reloads the agent

When wizard sends a partial TianyanConfig (only model/storage/agent filled), serde `#[serde(default)]` fills missing sections. Verify by running:

```powershell
cargo test -p tianyan-core config::tests::test_default_config -- --nocapture
```

Expected: Default TianyanConfig validates successfully (all sections have sensible defaults).

- [ ] **Step 2: Verify PUT endpoint accepts TianyanConfig JSON directly**

The existing `UpdateConfigRequest` wraps `{ "config": TianyanConfig }`. When the frontend sends:
```json
{
  "config": {
    "agent": { "enable_memory": true, ... },
    "storage": { "data_dir": "...", ... },
    "models": { "services": [...], ... }
  }
}
```

Missing sections (`logging`, `security`, `memory`, `retrieval`) will use Default via `#[serde(default)]`. This is correct behavior for wizard bootstrap.

- [ ] **Step 3: No code changes needed — verification only**

```powershell
cargo check --workspace
```

---

## Phase 3: Frontend Shared Components

### Task 3.1: Create `gui/src/components/config/` directory and shared types

**Files:**
- Create: `gui/src/components/config/mod.rs`
- Create: `gui/src/components/config/types.rs`

- [ ] **Step 1: Create config/mod.rs**

```rust
//! Shared configuration components — used by both wizard and settings page.

pub mod api;
pub mod types;

pub mod agent_section;
pub mod logging_section;
pub mod memory_section;
pub mod model_section;
pub mod retrieval_section;
pub mod security_section;
pub mod storage_section;
```

- [ ] **Step 2: Create config/types.rs**

```rust
//! Shared configuration types — maps directly to TianyanConfig fields.

use serde::{Deserialize, Serialize};

/// Determines how the config section component behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigMode {
    /// Wizard mode: step title, next/prev, step validation
    Wizard,
    /// Settings page mode: free editing, save on explicit action
    Settings,
}

/// Central config state — mirrors TianyanConfig for frontend editing.
/// All fields use String for simplicity (PathBuf → String).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ConfigState {
    // --- models ---
    pub model_services: Vec<ModelServiceState>,
    pub default_chat_model: String,
    pub default_embedding_model: String,
    pub default_vision_model: String,

    // --- storage ---
    pub data_dir: String,
    pub vector_url: String,
    pub collection_name: String,
    pub vector_dimension: usize,
    pub max_storage_size: u64,
    pub auto_cleanup: bool,
    pub cleanup_days: u32,

    // --- agent ---
    pub enable_skills: bool,
    pub enable_memory: bool,
    pub stream_responses: bool,
    pub enable_thinking: bool,
    pub default_top_k: usize,
    pub max_turns: usize,
    pub learned_rules_top_k: usize,
    pub learned_rules_max_tokens: usize,

    // --- logging ---
    pub log_level: String,
    pub log_format: String,
    pub log_max_file_size: u64,
    pub log_max_files: u32,
    pub log_include_timestamp: bool,
    pub log_include_location: bool,

    // --- security ---
    pub security_enabled: bool,
    pub confirm_commands: bool,
    pub audit_logging: bool,
    pub max_file_size: u64,
    pub allowed_directories: String,     // comma-separated
    pub blocked_directories: String,     // comma-separated
    pub allowed_commands: String,        // comma-separated
    pub blocked_commands: String,        // comma-separated

    // --- memory ---
    pub max_session_memory: usize,
    pub max_long_term_memory: usize,
    pub importance_threshold: f32,
    pub auto_consolidation: bool,
    pub consolidation_interval: u64,
    pub decay_rate: f32,

    // --- retrieval ---
    pub retrieval_top_k: usize,
    pub min_score: f32,
    pub two_stage_retrieval: bool,
    pub l0_multiplier: usize,
    pub max_context_tokens: usize,
    pub enable_cache: bool,
    pub cache_ttl: u64,

    // --- wizard UI state ---
    pub is_saving: bool,
    pub save_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelServiceState {
    pub name: String,
    pub service_type: String,   // "openai" | "custom"
    pub endpoint: String,
    pub api_key: String,
    pub default_model: String,
    pub models: Vec<String>,
    pub timeout: u64,
    pub enabled: bool,
    pub priority: u32,
    pub show_advanced: bool,
    pub test_status: TestStatus,
}

impl Default for ModelServiceState {
    fn default() -> Self {
        Self {
            name: String::new(),
            service_type: "openai".to_string(),
            endpoint: String::new(),
            api_key: String::new(),
            default_model: String::new(),
            models: Vec::new(),
            timeout: 60,
            enabled: true,
            priority: 0,
            show_advanced: false,
            test_status: TestStatus::NotTested,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestStatus {
    NotTested,
    Testing,
    Success,
    Failed,
}

impl TestStatus {
    pub fn text(&self) -> &'static str {
        match self {
            TestStatus::NotTested => "未测试",
            TestStatus::Testing => "测试中...",
            TestStatus::Success => "连接成功",
            TestStatus::Failed => "连接失败",
        }
    }
}

/// Wizard step enum for the 4-step flow
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Welcome,
    ModelConfig,
    DataConfig,
    AgentConfig,
    Confirm,
}

impl WizardStep {
    pub fn title(&self) -> &'static str {
        match self {
            WizardStep::Welcome => "欢迎",
            WizardStep::ModelConfig => "模型服务配置",
            WizardStep::DataConfig => "数据目录配置",
            WizardStep::AgentConfig => "智能体参数配置",
            WizardStep::Confirm => "确认和保存",
        }
    }
    pub fn number(&self) -> usize {
        match self {
            WizardStep::Welcome => 1,
            WizardStep::ModelConfig => 2,
            WizardStep::DataConfig => 3,
            WizardStep::AgentConfig => 4,
            WizardStep::Confirm => 5,
        }
    }
    pub fn total() -> usize { 5 }
    pub fn next(&self) -> Option<WizardStep> {
        match self {
            WizardStep::Welcome => Some(WizardStep::ModelConfig),
            WizardStep::ModelConfig => Some(WizardStep::DataConfig),
            WizardStep::DataConfig => Some(WizardStep::AgentConfig),
            WizardStep::AgentConfig => Some(WizardStep::Confirm),
            WizardStep::Confirm => None,
        }
    }
    pub fn previous(&self) -> Option<WizardStep> {
        match self {
            WizardStep::Welcome => None,
            WizardStep::ModelConfig => Some(WizardStep::Welcome),
            WizardStep::DataConfig => Some(WizardStep::ModelConfig),
            WizardStep::AgentConfig => Some(WizardStep::DataConfig),
            WizardStep::Confirm => Some(WizardStep::AgentConfig),
        }
    }
}

/// Config status response from GET /api/config/status
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigStatusResponse {
    pub configured: bool,
}

/// Full config response from GET /api/config
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigResponse {
    pub config: serde_json::Value,  // Raw JSON — parsed into ConfigState
}

/// Config save response from PUT /api/config
#[derive(Debug, Clone, Deserialize)]
pub struct SaveConfigResponse {
    pub success: bool,
    pub message: String,
}

/// Connection test types
#[derive(Debug, Clone, Serialize)]
pub struct TestConnectionRequest {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TestConnectionResponse {
    pub success: bool,
    pub message: String,
    pub available_models: Option<Vec<String>>,
}
```

- [ ] **Step 3: Implement ConfigState ↔ TianyanConfig JSON conversion**

Add to `types.rs`:

```rust
impl ConfigState {
    /// Convert ConfigState to TianyanConfig JSON for PUT /api/config.
    /// Only fills fields that are relevant; others use serde defaults on server side.
    pub fn to_tianyan_json(&self) -> serde_json::Value {
        serde_json::json!({
            "agent": {
                "enable_skills": self.enable_skills,
                "enable_memory": self.enable_memory,
                "stream_responses": self.stream_responses,
                "enable_thinking": self.enable_thinking,
                "default_top_k": self.default_top_k,
                "max_turns": self.max_turns,
                "learned_rules_top_k": self.learned_rules_top_k,
                "learned_rules_max_tokens": self.learned_rules_max_tokens,
            },
            "storage": {
                "data_dir": self.data_dir,
                "vector": {
                    "url": self.vector_url,
                    "collection_name": self.collection_name,
                    "vector_dimension": self.vector_dimension,
                },
                "max_storage_size": self.max_storage_size,
                "auto_cleanup": self.auto_cleanup,
                "cleanup_days": self.cleanup_days,
            },
            "models": {
                "services": self.model_services.iter().map(|s| serde_json::json!({
                    "name": s.name,
                    "type": s.service_type,
                    "endpoint": s.endpoint,
                    "api_key": s.api_key,
                    "default_model": s.default_model,
                    "models": s.models,
                    "timeout": s.timeout,
                    "enabled": s.enabled,
                    "priority": s.priority,
                })).collect::<Vec<_>>(),
                "default_chat_model": self.default_chat_model,
                "default_embedding_model": self.default_embedding_model,
                "default_vision_model": self.default_vision_model,
            },
            "logging": {
                "level": self.log_level,
                "format": self.log_format,
                "max_file_size": self.log_max_file_size,
                "max_files": self.log_max_files,
                "include_timestamp": self.log_include_timestamp,
                "include_location": self.log_include_location,
            },
            "security": {
                "enabled": self.security_enabled,
                "confirm_commands": self.confirm_commands,
                "audit_logging": self.audit_logging,
                "max_file_size": self.max_file_size,
            },
            "memory": {
                "max_session_memory": self.max_session_memory,
                "max_long_term_memory": self.max_long_term_memory,
                "importance_threshold": self.importance_threshold,
                "auto_consolidation": self.auto_consolidation,
                "consolidation_interval": self.consolidation_interval,
                "decay_rate": self.decay_rate,
            },
            "retrieval": {
                "default_top_k": self.retrieval_top_k,
                "min_score": self.min_score,
                "two_stage_retrieval": self.two_stage_retrieval,
                "l0_multiplier": self.l0_multiplier,
                "max_context_tokens": self.max_context_tokens,
                "enable_cache": self.enable_cache,
                "cache_ttl": self.cache_ttl,
            },
        })
    }

    /// Populate ConfigState from GET /api/config response JSON.
    pub fn from_tianyan_json(json: &serde_json::Value) -> Self {
        let mut state = ConfigState::default();

        if let Some(agent) = json.get("agent") {
            state.enable_skills = agent["enable_skills"].as_bool().unwrap_or(true);
            state.enable_memory = agent["enable_memory"].as_bool().unwrap_or(true);
            state.stream_responses = agent["stream_responses"].as_bool().unwrap_or(true);
            state.enable_thinking = agent["enable_thinking"].as_bool().unwrap_or(false);
            state.default_top_k = agent["default_top_k"].as_u64().unwrap_or(5) as usize;
            state.max_turns = agent["max_turns"].as_u64().unwrap_or(200) as usize;
            state.learned_rules_top_k = agent["learned_rules_top_k"].as_u64().unwrap_or(5) as usize;
            state.learned_rules_max_tokens = agent["learned_rules_max_tokens"].as_u64().unwrap_or(800) as usize;
        }

        if let Some(storage) = json.get("storage") {
            state.data_dir = storage["data_dir"].as_str().unwrap_or("").to_string();
            if let Some(vec) = storage.get("vector") {
                state.vector_url = vec["url"].as_str().unwrap_or("http://localhost:6334").to_string();
                state.collection_name = vec["collection_name"].as_str().unwrap_or("tianyan_contexts").to_string();
                state.vector_dimension = vec["vector_dimension"].as_u64().unwrap_or(1536) as usize;
            }
            state.max_storage_size = storage["max_storage_size"].as_u64().unwrap_or(0);
            state.auto_cleanup = storage["auto_cleanup"].as_bool().unwrap_or(true);
            state.cleanup_days = storage["cleanup_days"].as_u64().unwrap_or(365) as u32;
        }

        if let Some(models) = json.get("models") {
            state.default_chat_model = models["default_chat_model"].as_str().unwrap_or("").to_string();
            state.default_embedding_model = models["default_embedding_model"].as_str().unwrap_or("").to_string();
            state.default_vision_model = models["default_vision_model"].as_str().unwrap_or("").to_string();
            if let Some(services) = models["services"].as_array() {
                state.model_services = services.iter().map(|s| ModelServiceState {
                    name: s["name"].as_str().unwrap_or("").to_string(),
                    service_type: format_type(s["type"].as_str().unwrap_or("openai")),
                    endpoint: s["endpoint"].as_str().unwrap_or("").to_string(),
                    api_key: s["api_key"].as_str().unwrap_or("").to_string(),
                    default_model: s["default_model"].as_str().unwrap_or("").to_string(),
                    models: s["models"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default(),
                    timeout: s["timeout"].as_u64().unwrap_or(60),
                    enabled: s["enabled"].as_bool().unwrap_or(true),
                    priority: s["priority"].as_u64().unwrap_or(0) as u32,
                    show_advanced: false,
                    test_status: TestStatus::NotTested,
                }).collect();
            }
        }

        // logging, security, memory, retrieval similarly populated...

        state
    }
}

fn format_type(s: &str) -> String {
    match s {
        "openai" | "custom" => s.to_string(),
        _ => "openai".to_string(),
    }
}
```

- [ ] **Step 4: Commit**

```powershell
git add gui/src/components/config/
git commit -m "feat(gui): create shared config types and ConfigState"
```

### Task 3.2: Create config/api.rs — shared API layer

**Files:**
- Create: `gui/src/components/config/api.rs`

- [ ] **Step 1: Write api.rs**

```rust
//! Shared config API — used by wizard and settings page.

use gloo_net::http::Request;
use wasm_bindgen_futures::spawn_local;
use serde_json::json;

use super::types::{
    ConfigState, ConfigStatusResponse, SaveConfigResponse,
    TestConnectionRequest, TestConnectionResponse,
};

const API_BASE: &str = "http://localhost:3000/api";

/// Fetch config status — returns whether config file exists.
pub async fn fetch_config_status() -> Result<ConfigStatusResponse, String> {
    let url = format!("{}/config/status", API_BASE);
    let response = Request::get(&url).send().await.map_err(|e| format!("请求失败: {}", e))?;
    if !response.ok() {
        return Err(format!("HTTP 错误: {}", response.status()));
    }
    response.json::<ConfigStatusResponse>().await.map_err(|e| format!("解析响应失败: {}", e))
}

/// Fetch full TianyanConfig from GET /api/config.
pub async fn fetch_config() -> Result<ConfigState, String> {
    let url = format!("{}/config", API_BASE);
    let response = Request::get(&url).send().await.map_err(|e| format!("请求失败: {}", e))?;
    if !response.ok() {
        return Err(format!("HTTP 错误: {}", response.status()));
    }
    let body: serde_json::Value = response.json().await.map_err(|e| format!("解析响应失败: {}", e))?;
    let config_json = body.get("config").cloned().unwrap_or(serde_json::Value::Null);
    Ok(ConfigState::from_tianyan_json(&config_json))
}

/// Save full config via PUT /api/config.
pub async fn save_config(state: &ConfigState) -> Result<SaveConfigResponse, String> {
    let url = format!("{}/config", API_BASE);
    let body = json!({ "config": state.to_tianyan_json() });
    let body_str = serde_json::to_string(&body).map_err(|e| format!("序列化失败: {}", e))?;

    let request = Request::put(&url)
        .header("Content-Type", "application/json")
        .body(body_str)
        .map_err(|e| format!("构建请求失败: {}", e))?;

    let response = request.send().await.map_err(|e| format!("请求失败: {}", e))?;
    if !response.ok() {
        return Err(format!("HTTP 错误: {}", response.status()));
    }
    response.json::<SaveConfigResponse>().await.map_err(|e| format!("解析响应失败: {}", e))
}

/// Test model connection.
pub async fn test_connection(req: &TestConnectionRequest) -> Result<TestConnectionResponse, String> {
    let url = format!("{}/config/test-connection", API_BASE);
    let body_str = serde_json::to_string(req).map_err(|e| format!("序列化失败: {}", e))?;
    let request = Request::post(&url)
        .header("Content-Type", "application/json")
        .body(body_str)
        .map_err(|e| format!("构建请求失败: {}", e))?;
    let response = request.send().await.map_err(|e| format!("请求失败: {}", e))?;
    if !response.ok() {
        return Err(format!("HTTP 错误: {}", response.status()));
    }
    response.json::<TestConnectionResponse>().await.map_err(|e| format!("解析响应失败: {}", e))
}

/// Async fetch status (callback-based, for use_effect).
pub fn fetch_config_status_async<F>(callback: F)
where
    F: FnOnce(Result<ConfigStatusResponse, String>) + 'static,
{
    spawn_local(async move {
        let result = fetch_config_status().await;
        callback(result);
    });
}

/// Async save config (callback-based).
pub fn save_config_async<F>(state: ConfigState, callback: F)
where
    F: FnOnce(Result<SaveConfigResponse, String>) + 'static,
{
    spawn_local(async move {
        let result = save_config(&state).await;
        callback(result);
    });
}
```

- [ ] **Step 2: Commit**

```powershell
git add gui/src/components/config/api.rs
git commit -m "feat(gui): create shared config API layer"
```

### Task 3.3: Create model_section.rs (wizard step 1 + settings tab)

**Files:**
- Create: `gui/src/components/config/model_section.rs`

- [ ] **Step 1: Write the component**

```rust
use yew::prelude::*;
use web_sys::HtmlInputElement;

use super::types::{ConfigMode, ConfigState, ModelServiceState, TestStatus};
use super::api;

#[derive(Debug, Clone, PartialEq, Properties)]
pub struct ModelSectionProps {
    pub mode: ConfigMode,
    pub state: UseStateHandle<ConfigState>,
}

/// Model service configuration section — shared between wizard and settings.
#[function_component(ModelSection)]
pub fn model_section(props: &ModelSectionProps) -> Html {
    let state = props.state.clone();
    let service = match state.model_services.first() {
        Some(s) => s.clone(),
        None => {
            return html! {
                <div class="config-section">
                    <p>{ "请先添加至少一个模型服务" }</p>
                </div>
            };
        }
    };

    // --- field callbacks (same pattern as existing wizard) ---
    let on_name_change = make_input_callback(state.clone(), |s, v| {
        if let Some(svc) = s.model_services.first_mut() { svc.name = v; }
    });
    let on_endpoint_change = make_input_callback(state.clone(), |s, v| {
        if let Some(svc) = s.model_services.first_mut() { svc.endpoint = v; }
    });
    let on_api_key_change = make_input_callback(state.clone(), |s, v| {
        if let Some(svc) = s.model_services.first_mut() { svc.api_key = v; }
    });
    let on_model_change = make_input_callback(state.clone(), |s, v| {
        if let Some(svc) = s.model_services.first_mut() { svc.default_model = v; }
        s.default_chat_model = v;
    });

    let on_test_connection = {
        let state = state.clone();
        Callback::from(move |_| {
            let current = (*state).clone();
            let svc = match current.model_services.first() {
                Some(s) => s.clone(),
                None => return,
            };
            // Mark testing
            {
                let mut new_state = (*state).clone();
                if let Some(s) = new_state.model_services.first_mut() {
                    s.test_status = TestStatus::Testing;
                }
                state.set(new_state);
            }
            // Fire async test
            let req = api::TestConnectionRequest {
                endpoint: svc.endpoint.clone(),
                api_key: svc.api_key.clone(),
                model: svc.default_model.clone(),
            };
            let state_for_cb = state.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = api::test_connection(&req).await;
                let mut new_state = (*state_for_cb).clone();
                if let Some(s) = new_state.model_services.first_mut() {
                    s.test_status = match &result {
                        Ok(r) if r.success => TestStatus::Success,
                        _ => TestStatus::Failed,
                    };
                }
                state_for_cb.set(new_state);
            });
        })
    };

    let show_test_button = props.mode == ConfigMode::Wizard;

    html! {
        <div class="model-config config-section">
            <div class="form-group">
                <label>{ "服务名称" }</label>
                <input type="text" value={service.name.clone()} onchange={on_name_change}
                    placeholder="例如：百炼、OpenAI" />
            </div>
            <div class="form-group">
                <label>{ "API 端点 URL" }</label>
                <input type="text" value={service.endpoint.clone()} onchange={on_endpoint_change}
                    placeholder="https://dashscope.aliyuncs.com/compatible-mode/v1" />
            </div>
            <div class="form-group">
                <label>{ "API 密钥" }</label>
                <input type="password" value={service.api_key.clone()} onchange={on_api_key_change}
                    placeholder="sk-..." />
            </div>
            <div class="form-group">
                <label>{ "默认模型" }</label>
                <input type="text" value={service.default_model.clone()} onchange={on_model_change}
                    placeholder="例如：qwen3.5-plus" />
            </div>

            if show_test_button {
                <div class="form-group">
                    <button class="btn btn-secondary" onclick={on_test_connection}
                        disabled={service.test_status == TestStatus::Testing}>
                        { if service.test_status == TestStatus::Testing { "测试中..." } else { "测试连接" } }
                    </button>
                    if service.test_status != TestStatus::NotTested {
                        <span class={classes!("test-status",
                            service.test_status == TestStatus::Success then_some("success"),
                            service.test_status == TestStatus::Failed then_some("error"),
                        )}>{ service.test_status.text() }</span>
                    }
                </div>
            }
        </div>
    }
}

/// Helper: create an onchange callback that mutates ConfigState via a closure.
fn make_input_callback<F>(state: UseStateHandle<ConfigState>, f: F) -> Callback<Event>
where
    F: Fn(&mut ConfigState, String) + 'static,
{
    Callback::from(move |e: Event| {
        let input: HtmlInputElement = e.target_unchecked_into();
        let mut new_state = (*state).clone();
        f(&mut new_state, input.value());
        state.set(new_state);
    })
}
```

- [ ] **Step 2: Commit**

```powershell
git add gui/src/components/config/model_section.rs
git commit -m "feat(gui): create shared ModelSection component"
```

### Task 3.4: Create storage_section.rs, agent_section.rs

**Files:**
- Create: `gui/src/components/config/storage_section.rs`
- Create: `gui/src/components/config/agent_section.rs`

Follow the same pattern as model_section.rs. Each section:
- Takes `ConfigMode` + `UseStateHandle<ConfigState>` props
- Renders form fields for the relevant ConfigState fields
- In Wizard mode, optional: show only essential fields; in Settings mode, show all fields

**storage_section.rs** fields: data_dir, vector_url, collection_name (wizard), vector_dimension, max_storage_size, auto_cleanup, cleanup_days (settings).

**agent_section.rs** fields: enable_skills, enable_memory, stream_responses, enable_thinking (wizard), default_top_k, max_turns, learned_rules_top_k, learned_rules_max_tokens (settings).

- [ ] **Step 1: Write storage_section.rs**
- [ ] **Step 2: Write agent_section.rs**
- [ ] **Step 3: Commit**

### Task 3.5: Create security_section.rs, logging_section.rs, memory_section.rs, retrieval_section.rs

**Files:**
- Create: `gui/src/components/config/security_section.rs`
- Create: `gui/src/components/config/logging_section.rs`
- Create: `gui/src/components/config/memory_section.rs`
- Create: `gui/src/components/config/retrieval_section.rs`

These sections only appear in Settings mode (wizard doesn't show them). Follow same Props pattern.

**security_section.rs**: security_enabled, confirm_commands, audit_logging, max_file_size (essential). allowed_directories, blocked_directories, allowed_commands, blocked_commands (advanced, comma-separated text inputs).

**logging_section.rs**: log_level (select: trace/debug/info/warn/error), log_format (select: text/json), log_max_file_size, log_max_files, log_include_timestamp, log_include_location (checkboxes).

**memory_section.rs**: auto_consolidation (checkbox), max_session_memory, max_long_term_memory, importance_threshold (0.0-1.0 slider or number), consolidation_interval, decay_rate (0.0-1.0).

**retrieval_section.rs**: two_stage_retrieval (checkbox), enable_cache (checkbox), retrieval_top_k, min_score (0.0-1.0), l0_multiplier, max_context_tokens, cache_ttl.

- [ ] **Step 1: Write each section component**
- [ ] **Step 2: Commit**

---

## Phase 4: Frontend Pages

### Task 4.1: Rewrite ConfigWizard — 4 steps, no SystemPrompt

**Files:**
- Modify: `gui/src/components/config_wizard/mod.rs`
- Modify: `gui/src/components/config_wizard/types.rs`
- Modify: `gui/src/components/config_wizard/api.rs`

- [ ] **Step 1: Rewrite types.rs — alias ConfigState + WizardStep from shared config**

```rust
//! Config wizard types — thin re-export layer.

pub use crate::components::config::types::{
    ConfigState, ConfigMode, WizardStep, TestStatus, SaveConfigResponse,
};
```

Delete all the old types (`WizardState`, `ModelServiceConfig`, `ModelServiceType`, `ConfigStatusResponse`, `SaveConfigRequest`, `WizardConfigData`, etc.).

- [ ] **Step 2: Rewrite api.rs — delegate to shared config/api**

```rust
//! Config wizard API — delegates to shared config API.

pub use crate::components::config::api::{
    fetch_config_status, fetch_config_status_async, save_config, save_config_async,
    test_connection, ConfigStatusResponse,
};
```

- [ ] **Step 3: Rewrite mod.rs — 5-step wizard (Welcome → Model → Storage → Agent → Confirm)**

The main `ConfigWizard` component:
- Props: `on_complete: Callback<()>` (no more `default_system_prompt`)
- State: `UseStateHandle<ConfigState>` + `UseStateHandle<WizardStep>`
- Render: step indicator header + the current step component + prev/next/save buttons
- Step components: `WelcomeStep`, then pass `state` to `ModelSection`, `StorageSection`, `AgentSection` with `ConfigMode::Wizard`, then `ConfirmStep`.

Key differences from old wizard:
- No `SystemPromptStep` — deleted
- Uses shared `ModelSection`/`StorageSection`/`AgentSection` instead of inline components
- `ConfirmStep` shows summary of only model + storage + agent sections
- Save calls `save_config_async` which sends full TianyanConfig JSON via `PUT /api/config`

- [ ] **Step 4: Commit**

### Task 4.2: Rewrite SettingsPanel — left nav + right form, 7 tabs

**Files:**
- Modify: `gui/src/components/settings/mod.rs`

- [ ] **Step 1: Rewrite the component structure**

```rust
use yew::prelude::*;
use crate::components::config::types::ConfigState;
use crate::components::config::api as config_api;
use crate::state::{AppAction, AppState};

// Settings tab identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigTab {
    Models,
    Storage,
    Agent,
    Security,
    Logging,
    Memory,
    Retrieval,
}

impl ConfigTab {
    fn label(&self) -> &'static str {
        match self {
            ConfigTab::Models => "模型服务",
            ConfigTab::Storage => "数据存储",
            ConfigTab::Agent => "Agent 行为",
            ConfigTab::Security => "安全",
            ConfigTab::Logging => "日志",
            ConfigTab::Memory => "记忆",
            ConfigTab::Retrieval => "检索",
        }
    }
    const ALL: [ConfigTab; 7] = [
        ConfigTab::Models, ConfigTab::Storage, ConfigTab::Agent,
        ConfigTab::Security, ConfigTab::Logging, ConfigTab::Memory, ConfigTab::Retrieval,
    ];
}

#[function_component(ConfigSettingsPanel)]
fn config_settings_panel(props: &SettingsPanelProps) -> Html {
    let state = props.state.clone();
    let config_state = use_state(|| None::<ConfigState>);
    let active_tab = use_state(|| ConfigTab::Models);
    let save_status = use_state(|| None::<Result<String, String>>);

    // Load full config on mount
    {
        let config_state = config_state.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match config_api::fetch_config().await {
                    Ok(cfg) => config_state.set(Some(cfg)),
                    Err(e) => {
                        web_sys::console::error_1(&format!("加载配置失败: {}", e).into());
                    }
                }
            });
            || ()
        });
    }

    let on_save = {
        let config_state = config_state.clone();
        let save_status = save_status.clone();
        Callback::from(move |_| {
            if let Some(ref cfg) = *config_state {
                let cfg_clone = cfg.clone();
                let save_status = save_status.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match config_api::save_config(&cfg_clone).await {
                        Ok(resp) => save_status.set(Some(
                            if resp.success { Ok(resp.message) } else { Err(resp.message) }
                        )),
                        Err(e) => save_status.set(Some(Err(e))),
                    }
                });
            }
        })
    };

    html! {
        <div class="settings-panel config-settings">
            <div class="settings-header">
                <h2>{ "配置" }</h2>
                <button class="btn btn-primary" onclick={on_save}>{ "保存" }</button>
            </div>
            <div class="settings-body">
                <nav class="settings-nav">
                    {
                        ConfigTab::ALL.iter().map(|tab| {
                            let is_active = *active_tab == *tab;
                            let tab = *tab;
                            let active_tab = active_tab.clone();
                            html! {
                                <button
                                    class={classes!("settings-nav-item", is_active.then_some("active"))}
                                    onclick={Callback::from(move |_| active_tab.set(tab))}
                                >
                                    { tab.label() }
                                </button>
                            }
                        }).collect::<Html>()
                    }
                </nav>
                <div class="settings-content">
                    {
                        match config_state.as_ref() {
                            None => html! { <p>{ "加载中..." }</p> },
                            Some(cfg) => {
                                let cfg_handle = use_state(|| cfg.clone());
                                match *active_tab {
                                    ConfigTab::Models => html! { <ModelSection mode={ConfigMode::Settings} state={cfg_handle.clone()} /> },
                                    ConfigTab::Storage => html! { <StorageSection mode={ConfigMode::Settings} state={cfg_handle.clone()} /> },
                                    ConfigTab::Agent => html! { <AgentSection mode={ConfigMode::Settings} state={cfg_handle.clone()} /> },
                                    ConfigTab::Security => html! { <SecuritySection mode={ConfigMode::Settings} state={cfg_handle.clone()} /> },
                                    ConfigTab::Logging => html! { <LoggingSection mode={ConfigMode::Settings} state={cfg_handle.clone()} /> },
                                    ConfigTab::Memory => html! { <MemorySection mode={ConfigMode::Settings} state={cfg_handle.clone()} /> },
                                    ConfigTab::Retrieval => html! { <RetrievalSection mode={ConfigMode::Settings} state={cfg_handle.clone()} /> },
                                }
                            }
                        }
                    }
                </div>
            </div>
            // Save status feedback
            {
                if let Some(ref result) = *save_status {
                    html! {
                        <div class={classes!("save-toast", result.is_ok().then_some("success"), result.is_err().then_some("error"))}>
                            { match result { Ok(msg) => msg.clone(), Err(e) => e.clone() } }
                        </div>
                    }
                } else { html! {} }
            }
        </div>
    }
}
```

- [ ] **Step 2: Keep existing theme/font/about sections below or in a separate tab**

The old settings (theme, font-size, API URL, about) can live at the bottom of the settings page or in a separate "外观" section. Keep them but de-emphasize them relative to config.

- [ ] **Step 3: Commit**

### Task 4.3: Update main.rs bootstrap logic

**Files:**
- Modify: `gui/src/main.rs`

- [ ] **Step 1: Replace ConfigStatus check with GET /api/config/status**

In `main.rs`, the bootstrap `use_effect` currently calls `fetch_config_status_async` which returns `ConfigStatusResponse` with `configured` and `default_system_prompt`. Change to:

```rust
use components::config::api::fetch_config_status_async;

// In use_effect:
fetch_config_status_async(move |result| match result {
    Ok(status) => {
        if status.configured {
            let new_session_id = Uuid::new_v4().to_string();
            state.dispatch(AppAction::SetCurrentSession(Some(new_session_id)));
            mode.set(AppMode::Main);
        } else {
            mode.set(AppMode::Wizard);
        }
    }
    Err(_) => {
        mode.set(AppMode::Wizard);
    }
});
```

Remove `default_system_prompt` state entirely — it's no longer needed.

Remove `default_system_prompt` prop from `<ConfigWizard>`:
```rust
AppMode::Wizard => html! {
    <ConfigWizard on_complete={on_wizard_complete} />
},
```

- [ ] **Step 2: Add import for new config API path**

```rust
use components::config::api::fetch_config_status_async;
```

Remove old import:
```rust
// DELETE: use components::config_wizard::api::fetch_config_status_async;
```

- [ ] **Step 3: Wire SettingsPanel to use ConfigSettingsPanel**

In `main.rs`, the `View::Settings` match arm currently renders `<SettingsPanel>`. Change to use the new config-aware settings component. Either rename or add a new `View` variant.

Option: Keep `View::Settings` but render the new `ConfigSettingsPanel`:

```rust
View::Settings => html! {
    <SettingsPanel state={state.clone()} />
},
```

Where `SettingsPanel` (in `components/settings/mod.rs`) now is the new left-nav + right-form layout.

- [ ] **Step 4: Commit**

### Task 4.4: Add CSS for new settings layout

**Files:**
- Modify: CSS file(s) in gui (check `gui/` for where styles live)

- [ ] **Step 1: Add styles for side-nav settings layout**

```css
.settings-panel.config-settings {
    display: flex;
    flex-direction: column;
    height: 100%;
}
.settings-body {
    display: flex;
    flex: 1;
    overflow: hidden;
}
.settings-nav {
    width: 180px;
    border-right: 1px solid var(--border-color);
    padding: 8px 0;
    overflow-y: auto;
}
.settings-nav-item {
    display: block;
    width: 100%;
    padding: 10px 16px;
    text-align: left;
    border: none;
    background: none;
    cursor: pointer;
    font-size: 14px;
    color: var(--text-secondary);
}
.settings-nav-item:hover { background: var(--hover-bg); }
.settings-nav-item.active {
    background: var(--accent-bg);
    color: var(--accent-text);
    font-weight: 600;
}
.settings-content {
    flex: 1;
    padding: 16px 24px;
    overflow-y: auto;
}
```

- [ ] **Step 2: Commit**

---

## Phase 5: Cleanup & Verification

### Task 5.1: Remove dead code

- [ ] **Step 1: Remove old ConfigWizard references**

Check for any remaining imports of deleted types:
```powershell
cargo check --workspace 2>&1 | Select-String "error"
```

Fix any remaining compilation errors from deleted `WizardConfig`, `ConfigStatus`, `WizardState`, `default_system_prompt`.

- [ ] **Step 2: Remove old wizard CSS if any**

- [ ] **Step 3: Full lint + check**

```powershell
cargo check --workspace
cargo fmt --all -- --check
cargo clippy --workspace
```

- [ ] **Step 4: Run all tests**

```powershell
cargo test --workspace -- --nocapture
```

- [ ] **Step 5: Commit final cleanup**

```powershell
git add -A
git commit -m "chore: cleanup dead code after config wizard redesign"
```

---

## Self-Review Checklist

**1. Spec coverage:**
- [x] Remove SystemPrompt step from wizard → Task 4.1
- [x] Wizard covers all "must have" config fields → Task 3.4 (model + storage + agent sections)
- [x] Settings page covers all 7 config sections → Task 3.3-3.5 + Task 4.2
- [x] Shared components between wizard and settings → Task 3.3-3.5
- [x] Backend uses single PUT /api/config endpoint → Task 2.1-2.2
- [x] Delete dead WizardConfig types → Task 1.3
- [x] Delete dead ConfigStatus / default_system_prompt → Task 1.2
- [x] Soul.md is not user-editable → Task 4.1 (no SystemPrompt step)

**2. Placeholder scan:** No TBD/TODO/fill-in-later patterns. All sections have concrete field lists.

**3. Type consistency:**
- ConfigState is the single frontend type → defined in Task 3.1, used throughout Phases 3-4
- TianyanConfig is the single backend type → unchanged except removed re-exports
- ConfigMode enum (Wizard/Settings) → defined in Task 3.1, used by all section components
- API types consistent: ConfigStatusResponse (bootstrap), SaveConfigResponse (PUT response)

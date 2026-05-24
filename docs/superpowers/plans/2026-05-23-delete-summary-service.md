# 删除 SummaryService + SummaryServiceConfig 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 删除 `SummaryService`（死代码，从未启动）和 `SummaryServiceConfig`（无消费者），用已有的 `SummaryTask`（已注册到 scheduler）替代

**Architecture:** `SummaryTask` 已在 `tasks/summary_task.rs` 实现相同功能并通过 scheduler 正常运行。`SummaryService` 创建后从未调用 `start()`，是行为层面死代码。`SummaryEngine` 保留不动（被 `SummaryTask` 通过 `TaskContext.summary_engine` 使用）

**Tech Stack:** Rust, serde

---

## 影响范围

| 文件 | 操作 | 变更量 |
|------|------|--------|
| `core/src/storage/summary/service.rs` | **删除** | -534 行 |
| `core/src/storage/summary/mod.rs` | 编辑 | -3 行 |
| `core/src/storage/mod.rs` | 编辑 | -1 行 |
| `core/src/config/mod.rs` | 编辑 | -3 行 |
| `core/src/config/wizard.rs` | 编辑 | -2 行 |
| `server/src/state.rs` | 编辑 | -48 行 |
| `server/src/api/config/handlers.rs` | 编辑 | -1 行 |
| `server/tests/common/factory.rs` | 编辑 | -2 行 |
| `core/tests/config_integration.rs` | 编辑 | -1 行 |

**总计：9 文件，~595 行纯删除**。零新增代码。

---

### Task 1: 删除 `service.rs` + 清理 `storage/` re-exports

**Files:**
- Delete: `core/src/storage/summary/service.rs`
- Modify: `core/src/storage/summary/mod.rs`
- Modify: `core/src/storage/mod.rs`

- [ ] **Step 1: 删除文件**

```powershell
Remove-Item -LiteralPath "core/src/storage/summary/service.rs"
```

- [ ] **Step 2: 更新 `core/src/storage/summary/mod.rs`**

从 mod.rs 中删除最后两行 `pub mod service;` 和 `pub use service::{SummaryService, SummaryServiceConfig};`。

- [ ] **Step 3: 更新 `core/src/storage/mod.rs` re-export**

从 `pub use summary::{ ... }` 列表中删除 `SummaryService,` 和 `SummaryServiceConfig,`。

- [ ] **Step 4: 验证编译**

```powershell
cargo check --workspace 2>&1 | Select-Object -Last 5
```

Expected: 有编译错误（其他文件仍引用已删除符号），确认是预期错误。

- [ ] **Step 5: Commit**

```bash
git add -u core/src/storage/
git commit -m "refactor(storage): delete SummaryService, SummaryServiceConfig, and service.rs"
```

---

### Task 2: 清理 `config/` 模块

**Files:**
- Modify: `core/src/config/mod.rs` (3 处)
- Modify: `core/src/config/wizard.rs` (2 处)

- [ ] **Step 1: 更新 `core/src/config/mod.rs`**

3 处删除：
1. 删除 line 18: `pub use crate::storage::SummaryServiceConfig;`
2. 删除 lines 59-61: `pub summary_service: SummaryServiceConfig,` 字段（含注释）
3. 删除 line 182: `// summary_service 配置由 SummaryServiceConfig::validate() 单独验证`

- [ ] **Step 2: 更新 `core/src/config/wizard.rs`**

2 处删除：
1. 从 line 10 import 列表中删除 `SummaryServiceConfig,`
2. 删除 line 284: `summary_service: SummaryServiceConfig::default(),`

- [ ] **Step 3: 验证编译**

```powershell
cargo check -p tianyan-core 2>&1 | Select-Object -Last 5
```

Expected: 通过（core 引用已全清）。

- [ ] **Step 4: Commit**

```bash
git add core/src/config/mod.rs core/src/config/wizard.rs
git commit -m "refactor(config): remove SummaryServiceConfig from TianyanConfig"
```

---

### Task 3: 清理 `server/` 和测试

**Files:**
- Modify: `server/src/state.rs` (5 处)
- Modify: `server/src/api/config/handlers.rs` (1 处)
- Modify: `server/tests/common/factory.rs` (2 处)
- Modify: `core/tests/config_integration.rs` (1 处)

- [ ] **Step 1: 更新 `server/src/state.rs`**

5 处变更：
1. **Import** (line 22): `use tianyan::storage::{SummaryEngine, SummaryService, VirtualFileSystemImpl}` → 删除 `SummaryService,`
2. **字段** (lines 54-55): 删除 `summary_service: Option<Arc<SummaryService>>`
3. **函数** (lines 62-95): 删除整个 `create_summary_service()` 函数（34 行）
4. **构造调用** (line 118): 删除 `let summary_service = create_summary_service(&config, vfs.clone())?;`
5. **字段传值** (line 158): 从 `Ok(Self { ... })` 中删除 `summary_service,` 行
6. **访问器** (lines 164-169): 删除 `pub fn summary_service()` 方法

- [ ] **Step 2: 更新 `server/src/api/config/handlers.rs`**

删除 line 71: `"summary_service" => serde_json::to_value(&config.summary_service).unwrap_or(Value::Null),`

- [ ] **Step 3: 更新 `server/tests/common/factory.rs`**

1. 从 import 列表中删除 `SummaryServiceConfig,`
2. 删除 `summary_service: SummaryServiceConfig::default(),` 字段

- [ ] **Step 4: 更新 `core/tests/config_integration.rs`**

删除 line 48: `assert_eq!(config.summary_service.scan_interval_secs, 300);`

- [ ] **Step 5: 验证编译**

```powershell
cargo check --workspace 2>&1 | Select-Object -Last 5
```

Expected: No errors.

- [ ] **Step 6: Commit**

```bash
git add server/src/state.rs server/src/api/config/handlers.rs server/tests/common/factory.rs core/tests/config_integration.rs
git commit -m "refactor: remove SummaryService and SummaryServiceConfig from server and tests"
```

---

### Task 4: Final verification

- [ ] **Step 1: 验证无残留引用**

```powershell
rg "SummaryService|SummaryServiceConfig" --include '*.rs' core/src/ server/src/ gui/src/
```

Expected: Zero results.

- [ ] **Step 2: 运行全量测试**

```powershell
cargo test --workspace --lib -- --nocapture
```

Expected: All tests pass.

- [ ] **Step 3: 运行 lint**

```powershell
cargo clippy --workspace 2>&1 | Select-String -Pattern "SummaryService"
```

Expected: Zero matches.

---

## 保留项确认

| 符号 | 保留原因 |
|------|----------|
| `SummaryEngine` | 被 `SummaryTask` 通过 `TaskContext.summary_engine` 使用 |
| `TokenCounter` | 被 `knowledge/chunker` 和 `SummaryEngine` 使用 |
| `MockSummaryEngine` | 测试用 |
| `SummaryTask` | 已注册 scheduler，正常运行 |
| `SummaryLevel` / `ABSTRACT_TOKEN_LIMIT` / `OVERVIEW_TOKEN_LIMIT` | `SummaryTask` 使用 |

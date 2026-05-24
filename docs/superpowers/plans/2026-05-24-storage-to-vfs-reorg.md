# storage → vfs 模块重组实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 `core/src/storage/` 重命名为 `core/src/vfs/`，删除 `StorageBackend` trait，按 Layer 0/1/2 分层重组子模块

**Architecture:** 重命名后的三层结构：`backend/`（Layer 0 纯文件 I/O，struct 无 trait）→ `uri_mapper.rs`（Layer 1 路径映射）→ `vfs_impl.rs`（Layer 2 VFS 实现组合层）。`VectorStorage` trait 保留（有 Qdrant + Mock 两个 adapter），`VirtualFileSystem` trait 暂时保留。`summary/` 和 `vector/` 作为 VFS 子模块

**Tech Stack:** Rust, git mv, bulk find-replace

---

## 文件映射表

| 原路径 | 新路径 |
|--------|--------|
| `core/src/storage/` | `core/src/vfs/` (git mv) |
| `core/src/storage/vfs/mod.rs` | `core/src/vfs/vfs_impl.rs` |
| `core/src/storage/vfs/builder.rs` | `core/src/vfs/vfs_builder.rs` |
| `core/src/storage/vfs/tests.rs` | `core/src/vfs/vfs_tests.rs` |
| `core/src/storage/traits.rs` | `core/src/vfs/traits.rs` (删除 `StorageBackend`) |
| `core/src/storage/types.rs` | `core/src/vfs/types.rs` |
| `core/src/storage/mod.rs` | `core/src/vfs/mod.rs` |
| `core/src/storage/test_utils.rs` | `core/src/vfs/test_utils.rs` |
| `core/src/storage/uri_mapper.rs` | `core/src/vfs/uri_mapper.rs` |
| `core/src/storage/backend/local.rs` | `core/src/vfs/backend/local.rs` |
| `core/src/storage/vector/qdrant.rs` | `core/src/vfs/vector/qdrant.rs` |
| `core/src/storage/summary/engine.rs` | `core/src/vfs/summary/engine.rs` |

---

### Task 1: 目录重命名 + 内部重构

**Files:**
- Rename: `core/src/storage/` → `core/src/vfs/` (directory)
- Rename: `core/src/vfs/vfs/mod.rs` → `core/src/vfs/vfs_impl.rs`
- Rename: `core/src/vfs/vfs/builder.rs` → `core/src/vfs/vfs_builder.rs`
- Rename: `core/src/vfs/vfs/tests.rs` → `core/src/vfs/vfs_tests.rs`
- Delete: `core/src/vfs/vfs/` (empty directory)

**Context:** `git mv` 保留文件历史。因 `vfs/mod.rs` 是子目录入口，目录重命名后外层叫 `vfs/` 会与内层 `vfs/` 子目录冲突，故将原 `vfs/` 下文件展平。

- [ ] **Step 1: 执行 git mv**

```powershell
git mv core/src/storage core/src/vfs
```

Expected: Working tree reflects new directory name.

- [ ] **Step 2: 展平原 vfs/ 子目录**

```powershell
git mv core/src/vfs/vfs/mod.rs core/src/vfs/vfs_impl.rs
git mv core/src/vfs/vfs/builder.rs core/src/vfs/vfs_builder.rs
git mv core/src/vfs/vfs/tests.rs core/src/vfs/vfs_tests.rs
Remove-Item -LiteralPath "core/src/vfs/vfs"
```

Expected: No more `core/src/vfs/vfs/` directory; `vfs_impl.rs`, `vfs_builder.rs`, `vfs_tests.rs` now at `core/src/vfs/` level.

- [ ] **Step 3: 更新内部 cross-module 引用 (crate::storage:: → crate::vfs::)**

在 `core/src/vfs/` 目录内所有 `.rs` 文件中，将 `crate::storage::` 替换为 `crate::vfs::`：

```powershell
$files = Get-ChildItem -LiteralPath "core/src/vfs" -Recurse -Filter "*.rs"
foreach ($f in $files) {
    $content = Get-Content -LiteralPath $f.FullName -Raw
    $content = $content -replace 'crate::storage::', 'crate::vfs::'
    Set-Content -LiteralPath $f.FullName -Value $content -NoNewline
}
```

**注意：** 这也会替换 `vfs_impl.rs` 等文件中原本导入了 `crate::storage::types` 的引用。手动检查以下特殊文件：

- `vfs_impl.rs:11` — 原来有 `use crate::storage::model::EmbeddingService` → 应为 `use crate::model::EmbeddingService`（不是 vfs 内部）。等等，让我看一下这行原来的内容...

Actually, `vfs_impl.rs` line 11 was: `use crate::model::EmbeddingService;` — that's already correct, not affected by the replace.

But `vfs_impl.rs:12` — `use crate::storage::traits::{...}` → needs to become `use crate::vfs::traits::{...}`. The bulk replace handles this.

However there's a complication: `vfs_impl.rs` imports `ContentMetadata` from `crate::storage::traits` — after the move, it becomes `crate::vfs::traits`. The bulk replace handles this.

But also need to check: `vfs_impl.rs` uses `use crate::config::StorageConfig;` — that's already `crate::config::`, not affected.

- [ ] **Step 4: 更新 `vfs_impl.rs` 内部的 mod 声明**

`vfs_impl.rs` 底部有：
```rust
pub mod builder;
#[cfg(test)]
mod tests;
```

这些需要改为新文件名的引用。改为：
```rust
#[path = "vfs_builder.rs"]
pub mod builder;
#[cfg(test)]
#[path = "vfs_tests.rs"]
mod tests;
```

- [ ] **Step 5: 验证编译**

```powershell
cargo check -p tianyan-core 2>&1 | Select-Object -Last 5
```

Expected: 有大量错误（外部引用尚未更新），但确认无意外错误。

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: rename storage/ -> vfs/, flatten vfs/ subdirectory"
```

---

### Task 2: 更新所有外部 crate::storage:: 引用

**Files:** 约 12 个文件

**Context:** `core/src/vfs/` 之外的代码仍写 `crate::storage::`，现在模块路径已变更。

- [ ] **Step 1: 批量替换 `crate::storage::` → `crate::vfs::`**

```powershell
$externalFiles = Get-ChildItem -LiteralPath "core/src" -Recurse -Filter "*.rs" | Where-Object { $_.FullName -notlike "*\vfs\*" }
foreach ($f in $externalFiles) {
    $content = Get-Content -LiteralPath $f.FullName -Raw
    if ($content -match 'crate::storage::') {
        $content = $content -replace 'crate::storage::', 'crate::vfs::'
        Set-Content -LiteralPath $f.FullName -Value $content -NoNewline
    }
}
```

Expected: 所有 `crate::storage::` 在非 vfs 目录中替换为 `crate::vfs::`。

- [ ] **Step 2: 手动检查重点文件**

**`tasks/summary_task.rs:12`** — 原为 `use crate::storage::{ContextEntry, ABSTRACT_TOKEN_LIMIT};` → 应变为 `use crate::vfs::{ContextEntry, ABSTRACT_TOKEN_LIMIT};`

**`scheduler/task_scheduler.rs:13`** — 原为 `use crate::storage::{SummaryEngine, VirtualFileSystem};` → 应变为 `use crate::vfs::{SummaryEngine, VirtualFileSystem};`

注意：`knowledge/ingestor/builder.rs:3` 引用了 `StorageBackend` — 后续 Task 4 会处理删除此 trait 导入。

- [ ] **Step 3: 验证编译 (core)**

```powershell
cargo check -p tianyan-core 2>&1 | Select-Object -Last 5
```

Expected: No errors inside `core/`.

- [ ] **Step 4: Commit**

```bash
git add -u core/src/
git commit -m "refactor: update crate::storage:: -> crate::vfs:: in core/"
```

---

### Task 3: 更新 server/ 和外部 crate 引用

**Files:**
- Modify: `server/src/state.rs:22`
- Modify: `server/src/lib.rs:106,135`
- Modify: `server/src/agent_builder.rs:23`

- [ ] **Step 1: 批量替换 `tianyan::storage::` → `tianyan::vfs::`**

```powershell
$serverFiles = Get-ChildItem -LiteralPath "server/src" -Recurse -Filter "*.rs"
foreach ($f in $serverFiles) {
    $content = Get-Content -LiteralPath $f.FullName -Raw
    if ($content -match 'tianyan::storage::') {
        $content = $content -replace 'tianyan::storage::', 'tianyan::vfs::'
        Set-Content -LiteralPath $f.FullName -Value $content -NoNewline
    }
}
```

Also check gui/ and tauri/:

```powershell
$guiFiles = Get-ChildItem -LiteralPath "gui/src" -Recurse -Filter "*.rs" -ErrorAction SilentlyContinue
foreach ($f in $guiFiles) {
    $content = Get-Content -LiteralPath $f.FullName -Raw
    if ($content -match 'tianyan::storage::') {
        $content = $content -replace 'tianyan::storage::', 'tianyan::vfs::'
        Set-Content -LiteralPath $f.FullName -Value $content -NoNewline
    }
}
```

- [ ] **Step 2: 验证编译**

```powershell
cargo check --workspace 2>&1 | Select-Object -Last 5
```

Expected: No errors. 如果有 `StorageBackend` 相关错误，属于预期的（Task 4 处理）。

- [ ] **Step 3: Commit**

```bash
git add -u server/src/ gui/src/
git commit -m "refactor: update tianyan::storage:: -> tianyan::vfs:: in server/gui"
```

---

### Task 4: 删除 `StorageBackend` trait，转为 `LocalFileBackend` struct

**Files:**
- Modify: `core/src/vfs/traits.rs` — 删除 `StorageBackend` trait 定义
- Modify: `core/src/vfs/backend/local.rs` — 删除 `#[async_trait] impl StorageBackend`，改为 struct 的直接方法
- Modify: `core/src/vfs/vfs_impl.rs` — `Arc<dyn StorageBackend>` → `Arc<LocalFileBackend>`
- Modify: `core/src/vfs/vfs_builder.rs` — 同上
- Modify: `core/src/vfs/test_utils.rs` — 同上
- Modify: `core/src/vfs/summary/engine.rs` — 删除 `use crate::vfs::traits::StorageBackend`
- Modify: `core/src/knowledge/ingestor/builder.rs` — 删除 `StorageBackend` 引用
- Modify: `core/src/knowledge/ingestor/mod.rs` — 删除 `StorageBackend` 引用

**Context:** `StorageBackend` 只有一个实现 `LocalStorageBackend`，按 "one adapter = hypothetical seam" 原则删除 trait。`LocalStorageBackend` 重命名为 `LocalFileBackend`，方法直接暴露在 struct 上。

- [ ] **Step 1: 删除 `StorageBackend` trait 定义**

在 `core/src/vfs/traits.rs` 中删除 lines 26-94（整个 `StorageBackend` trait + `ContentMetadata` struct 定义）。

Note: `ContentMetadata` struct 定义在 `StorageBackend` trait 之上，但被 `VfsMetadata::get_all_content_metadata` 使用 → 保留 `ContentMetadata`，只删除 `StorageBackend` trait。

Actually, let me re-check: is `ContentMetadata` used by VfsMetadata? Yes — `VfsMetadata::get_all_content_metadata` returns `HashMap<ContentLevel, ContentMetadata>`. So keep `ContentMetadata`.

删除 `StorageBackend` trait 后的 `traits.rs` 从 `ContentMetadata` 后直接接 `VectorStorage` trait：

```rust
/// 内容元数据。
#[derive(Debug, Clone)]
pub struct ContentMetadata {
    pub size: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 向量存储后端 trait。
```

- [ ] **Step 2: 重构 `backend/local.rs` → `LocalFileBackend`**

将 `LocalStorageBackend` 重命名为 `LocalFileBackend`。删除 `#[async_trait] impl StorageBackend for LocalStorageBackend { ... }` 块。将其中**公开方法**提升为 `impl LocalFileBackend` 的直接方法。

`LocalFileBackend` 的公开方法（直接声明）：
```rust
pub struct LocalFileBackend {
    config: StorageConfig,
    mapper: UriMapper,
    write_locks: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
}

impl LocalFileBackend {
    pub fn new(config: StorageConfig) -> Self { ... }
    pub async fn initialize(&self) -> Result<()> { ... }
    pub async fn exists(&self, uri: &TianyanUri) -> Result<bool> { ... }
    pub async fn read_entry(&self, uri: &TianyanUri) -> Result<ContextEntry> { ... }
    pub async fn write_entry(&self, entry: &ContextEntry) -> Result<()> { ... }
    pub async fn delete_entry(&self, uri: &TianyanUri) -> Result<()> { ... }
    pub async fn list_directory(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>> { ... }
    pub async fn read_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String> { ... }
    pub async fn write_content(&self, uri: &TianyanUri, level: ContentLevel, content: &str) -> Result<()> { ... }
    pub async fn append_content(&self, uri: &TianyanUri, level: ContentLevel, content: &str) -> Result<()> { ... }
    pub async fn get_directory_index(&self, uri: &TianyanUri) -> Result<DirectoryIndex> { ... }
    pub async fn get_stats(&self) -> Result<StorageStats> { ... }
}
```

方法体不变，只是从 trait impl 块移到 struct impl 块中。

**注意**：文件内的导入 `use crate::vfs::traits::StorageBackend` 需要删除。`use crate::vfs::uri_mapper::UriMapper` 保留。

- [ ] **Step 3: 更新 `vfs_impl.rs` → `Arc<LocalFileBackend>`**

将 `storage: Arc<dyn StorageBackend>` 改为 `storage: Arc<LocalFileBackend>`（所有出现处）。

构造函数 `new()`：
```rust
pub fn new(
    storage: Arc<LocalFileBackend>,
    vector_storage: Arc<dyn VectorStorage>,
    config: StorageConfig,
) -> Self { ... }
```

同样更新 `vfs_builder.rs` 和 `test_utils.rs` 中的类型引用。

- [ ] **Step 4: 更新 `summary/engine.rs`**

删除 `use crate::vfs::traits::StorageBackend;` 导入。

`update_entry_summaries` 和 `update_parent_summaries` 方法参数 `storage: &dyn StorageBackend` 改为 `storage: &LocalFileBackend`。

- [ ] **Step 5: 更新 knowledge/ 中的 `StorageBackend` 引用**

`knowledge/ingestor/mod.rs:15` — 从 import 中删除 `StorageBackend,`。泛型参数 `<..., S: StorageBackend>` 改为 `<..., S>`（删除 bound）。如果 ingestor 内部不对 storage 做任何 trait-bound 约束的调用，可以直接移除泛型参数。

`knowledge/ingestor/builder.rs:3` — 删除 `StorageBackend` 导入。

- [ ] **Step 6: 验证编译**

```powershell
cargo check --workspace 2>&1 | Select-Object -Last 5
```

Expected: No errors.

- [ ] **Step 7: 运行测试**

```powershell
cargo test --workspace --lib -- --nocapture 2>&1 | Select-String -Pattern "test result:"
```

Expected: All tests pass.

- [ ] **Step 8: Commit**

```bash
git add -u
git commit -m "refactor: remove StorageBackend trait, use LocalFileBackend struct directly"
```

---

### Task 5: VectorStorage trait 移入 vector/traits.rs

**Files:**
- Create: `core/src/vfs/vector/traits.rs`
- Modify: `core/src/vfs/traits.rs` — 删除 `VectorStorage` trait 定义
- Modify: `core/src/vfs/vector/mod.rs` — 添加 `pub use traits::VectorStorage`

**Context:** `VectorStorage` 有两个 adapter（Qdrant + Mock），需要保留 trait。但应放在 `vector/` 子模块中，不在顶级 `traits.rs`。

- [ ] **Step 1: 创建 `core/src/vfs/vector/traits.rs`**

将 `VectorStorage` trait 定义（约 90 行）从 `core/src/vfs/traits.rs` 移到 `core/src/vfs/vector/traits.rs`，方法体保持不变。

**文件内容：**
```rust
//! 向量存储后端 trait。

use async_trait::async_trait;

use crate::common::error::Result;
use crate::common::types::TianyanUri;
use crate::vfs::types::{VectorPoint, VectorSearchQuery, VectorSearchResult, VectorType};

#[async_trait]
pub trait VectorStorage: Send + Sync {
    async fn initialize(&self) -> Result<()>;
    async fn upsert_point(&self, point: &VectorPoint) -> Result<()>;
    async fn upsert_points(&self, points: &[VectorPoint]) -> Result<()> { ... }
    async fn delete_point(&self, id: &str) -> Result<()>;
    async fn search(&self, query: VectorSearchQuery) -> Result<Vec<VectorSearchResult>>;
    async fn get_point(&self, id: &str) -> Result<Option<VectorPoint>>;
    async fn update_vector(&self, uri: &TianyanUri, vector_type: VectorType, vector: &[f32]) -> Result<()>;
    async fn count_points(&self) -> Result<usize>;
    async fn clear(&self) -> Result<()>;
    async fn search_fused(&self, ...) -> Result<Vec<VectorSearchResult>> { Ok(vec![]) }
    async fn search_abstract_and_overview(&self, ...) -> Result<Vec<VectorSearchResult>> { ... }
}
```

- [ ] **Step 2: 更新 `vector/mod.rs`**

```rust
//! 向量存储 —— VectorStorage 适配器实现。

mod qdrant;
mod traits;

pub use qdrant::{QdrantVectorStore, QdrantVectorStoreBuilder};
pub use traits::VectorStorage;
```

- [ ] **Step 3: 删除 `traits.rs` 中的 `VectorStorage`**

从 `core/src/vfs/traits.rs` 中删除 `VectorStorage` trait 定义（保留其上的 `ContentMetadata` 和其下的 `VfsCore`）。

- [ ] **Step 4: 更新引用 VectorStorage 的内部文件**

`vfs_impl.rs`, `vfs_builder.rs`, `test_utils.rs`, `qdrant.rs` 中原来 `use crate::vfs::traits::VectorStorage` → `use crate::vfs::vector::VectorStorage`

批量替换：
```powershell
$files = Get-ChildItem -LiteralPath "core/src/vfs" -Recurse -Filter "*.rs"
foreach ($f in $files) {
    $content = Get-Content -LiteralPath $f.FullName -Raw
    $content = $content -replace 'use crate::vfs::traits::VectorStorage', 'use crate::vfs::vector::VectorStorage'
    Set-Content -LiteralPath $f.FullName -Value $content -NoNewline
}
```

- [ ] **Step 5: 验证编译 + 测试**

```powershell
cargo check --workspace 2>&1 | Select-Object -Last 3
cargo test --workspace --lib -- --nocapture 2>&1 | Select-String -Pattern "test result:"
```

Expected: All pass.

- [ ] **Step 6: Commit**

```bash
git add core/src/vfs/vector/traits.rs core/src/vfs/traits.rs core/src/vfs/vector/mod.rs -u
git commit -m "refactor: move VectorStorage trait to vector/traits.rs"
```

---

### Task 6: 清理 `config/` 和 `server/` 中残留引用

**Files:**
- Modify: `core/src/config/mod.rs` — 删除或更新 `StorageConfig` 相关的 re-export
- Modify: `server/src/state.rs` — 确认无残留的 storage:: 引用

**Context:** `config/mod.rs` 中可能有 `pub use crate::config::StorageConfig;` 是从 `storage/mod.rs` 过去的。现在 `vfs/mod.rs` 应继续 re-export `StorageConfig`。

检查 `config/` 中是否有引用 `crate::storage::`：
```powershell
rg "crate::storage" core/src/config/
```

Expected: Zero results（已在 Task 2 中处理）。

检查 `server/` 中是否有残留：
```powershell
rg "tianyan::storage" server/src/
```

Expected: Zero results.

- [ ] **Step 1: 验证零残留**

```powershell
cargo check --workspace 2>&1 | Select-String -Pattern "^error"
```

Expected: No errors.

- [ ] **Step 2: Commit**

```bash
git commit -m "refactor: final cleanup of storage:: references" --allow-empty
```

---

### Task 7: 最终验证

- [ ] **Step 1: 运行全量测试**

```powershell
cargo test --workspace --lib -- --nocapture 2>&1 | Select-String -Pattern "test result:"
```

Expected: All tests pass with count matching previous run (~298 core + 39 server).

- [ ] **Step 2: 运行 lint**

```powershell
cargo clippy --workspace 2>&1 | Select-String -Pattern "storage" | Select-Object -First 5
```

Expected: Zero matches.

- [ ] **Step 3: 验证模块结构**

```powershell
Get-ChildItem -LiteralPath "core/src/vfs" -Recurse -Directory | ForEach-Object { $_.FullName }
```

Expected:
```
core/src/vfs/backend
core/src/vfs/summary
core/src/vfs/vector
```

---

## 最终结构

```
core/src/vfs/
├── mod.rs            ← 对外入口，re-export 所有 pub 符号
├── traits.rs         ← VFS 对外接口（VfsCore, ContentStore, VfsSearch, VfsMetadata, VirtualFileSystem）
├── types.rs          ← ContextEntry, VectorPoint, DirectoryIndex, StorageStats, ...
├── uri_mapper.rs     ← Layer 1: 路径映射（纯计算）
├── vfs_impl.rs       ← Layer 2: VirtualFileSystemImpl（组合 backend + mapper + vector）
├── vfs_builder.rs    ← VFS 构建器
├── vfs_tests.rs      ← VFS 单元测试
├── test_utils.rs     ← MockVectorStorage + create_test_vfs()
│
├── backend/
│   ├── mod.rs
│   └── local.rs      ← LocalFileBackend struct（无 trait）
│
├── vector/
│   ├── mod.rs
│   ├── traits.rs     ← VectorStorage trait（保留：双 adapter）
│   └── qdrant.rs     ← QdrantVectorStore
│
└── summary/
    ├── mod.rs
    └── engine.rs     ← SummaryEngine, MockSummaryEngine
```

---

## Self-Review

1. **Spec coverage:**
   - [x] `storage/` → `vfs/` 重命名 — Task 1
   - [x] `vfs/` 子目录展平 → `vfs_impl.rs` — Task 1
   - [x] 内部 `crate::storage::` → `crate::vfs::` — Task 1
   - [x] 外部 `crate::storage::` → `crate::vfs::` — Task 2
   - [x] server/ 引用更新 — Task 3
   - [x] 删除 `StorageBackend` trait — Task 4
   - [x] `VectorStorage` 移入 `vector/traits.rs` — Task 5
   - [x] 最终清理 + 验证 — Task 6, 7

2. **Placeholder scan:** 无 TBD/TODO。所有步骤都有具体命令。

3. **Type consistency:** `LocalFileBackend` 类型名在 Task 4 定义，在 vfs_impl/builder/test_utils 中使用，保持一致。

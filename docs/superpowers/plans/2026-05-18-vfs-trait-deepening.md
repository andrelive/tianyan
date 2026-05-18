# VFS Trait 深化重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 VirtualFileSystem trait 从 36 个方法收敛为 13 个方法、4 个子 trait + 1 个组合超 trait，同时将向量搜索能力纳入 VFS，消除 DualLayerRetriever 对 VectorStorage 的直接依赖。

**Architecture:** 在现有 `storage/traits.rs` 中新增 4 个子 trait（VfsCore、ContentStore、VfsSearch、VfsMetadata），由 VirtualFileSystem 超 trait 组合。VirtualFileSystemImpl 实现全部子 trait。search 方法重新设计为 embed → RRF 向量检索。ContentLoader trait 合并到 ContentStore。

**Tech Stack:** Rust, async-trait, tokio, qdrant-client

---

## File Map

| File | Change | Responsibility |
|------|--------|---------------|
| `core/src/storage/traits.rs` | **Heavy modify** | 定义 4 新子 trait + 超 trait, 删除 ContentLoader，修改 search 签名, RRF 融合函数保持不变 |
| `core/src/storage/vfs/mod.rs` | **Heavy modify** | 实现 4 子 trait, search 改为 embed→RRF, 删除旧方法实现, 删除 storage_backend/vector_storage/get_vector_storage |
| `core/src/storage/vfs/builder.rs` | **Modify** | 更新测试 use, 移除 ContentLoader mock |
| `core/src/storage/mod.rs` | **Modify** | 更新 re-exports |
| `core/src/context/retrieval/retriever.rs` | **Heavy modify** | DualLayerRetriever 改用 `Arc<dyn VirtualFileSystem>`, 移除 vector_storage + embedding_service 字段 |
| `core/src/context/retrieval/loader.rs` | **Modify** | ContentLoaderImpl 改用 VFS trait |
| `core/src/context/retrieval/mod.rs` | **Modify** | 更新 re-exports |
| `core/src/context/retrieval/intent.rs` | **No change** | 保持独立 |
| `core/src/context/mod.rs` | **Modify** | 更新 re-exports |
| `core/src/context/pipeline.rs` | **Modify** | 更新 use |
| `core/src/context/rule_recorder.rs` | **Modify** | 更新 use |
| `core/src/context/rule_suggester.rs` | **Modify** | 更新 use |
| `core/src/agent/coordinator.rs` | **Modify** | 更新 use |
| `core/src/agent/builder.rs` | **Modify** | 更新 use |
| `core/src/session/manager.rs` | **Modify** | 更新 use |
| `core/src/skills/manager.rs` | **Modify** | 更新 use |
| `core/src/skills/learning/mod.rs` | **Modify** | 更新 MockVfs |
| `core/src/storage/extractor.rs` | **Modify** | 更新 use |
| `core/src/storage/summary_service.rs` | **Modify** | 更新 use |
| `core/src/storage/summary.rs` | **Modify** | 更新 use |
| `core/src/scheduler/task_scheduler.rs` | **Modify** | 更新 use |
| `core/src/tasks/summary_task.rs` | **Modify** | 更新 use |
| `core/src/tasks/memory_task.rs` | **Modify** | 更新 use |
| `core/src/tasks/gc_task.rs` | **Modify** | 更新 use |
| `server/src/agent_builder.rs` | **Modify** | 改传 VFS 给 DualLayerRetriever |
| `server/src/state.rs` | **Modify** | 更新 use |
| `server/src/lib.rs` | **Modify** | 更新 use |
| `core/src/common/types/uri.rs` | **No change** | TianyanUri 已有 `join` 方法 |
| `core/src/common/types/content.rs` | **No change** | ContentLevel 保持不变 |

---

### Phase 0: 定义新 trait 结构

### Task 0: Define the 4 sub-traits and supertrait

**Files:**
- Modify: `core/src/storage/traits.rs` (add new traits before old VirtualFileSystem, keep old temporarily)
- Modify: `core/src/storage/mod.rs` (update re-exports)

- [ ] **Step 1: Add new trait definitions to storage/traits.rs**

在 `core/src/storage/traits.rs` 的第 293 行（`VirtualFileSystem` trait 之前）插入以下内容：

```rust
/// 虚拟文件系统核心操作 —— 条目生命周期管理。
#[async_trait]
pub trait VfsCore: Send + Sync {
    /// 初始化文件系统和向量存储。
    async fn initialize(&self) -> Result<()>;

    /// 检查条目是否存在。
    async fn exists(&self, uri: &TianyanUri) -> Result<bool>;

    /// 递归删除条目及其子条目，同时清理向量库。
    async fn delete(&self, uri: &TianyanUri) -> Result<()>;

    /// 列出指定 URI 前缀下的所有条目。
    async fn list(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>>;

    /// 移动条目及其子条目到新位置。
    async fn move_entry(&self, source: &TianyanUri, destination: &TianyanUri) -> Result<()>;
}

/// 分层内容读写 —— L0 Abstract / L1 Overview / L2 Detail。
#[async_trait]
pub trait ContentStore: Send + Sync {
    /// 写入指定层级的内容。条目不存在则自动创建。
    async fn write(&self, uri: &TianyanUri, level: ContentLevel, content: &str) -> Result<()>;

    /// 读取指定层级的内容。
    async fn read(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String>;

    /// 追加内容到 Detail 层级末尾。用于会话消息持续写入。
    async fn append(&self, uri: &TianyanUri, content: &str) -> Result<()>;

    /// 检查条目在指定层级是否已有内容。
    async fn has_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<bool>;
}

/// 向量检索 —— 查询文本先 embed 再通过 RRF 融合 Abstract+Overview 向量搜索。
#[async_trait]
pub trait VfsSearch: Send + Sync {
    /// 用 EmbeddingService 将查询文本转为向量，进行 Abstract + Overview 双向量 RRF 融合检索。
    /// `namespace` 为 `None` 时搜索全部命名空间。
    async fn search(
        &self,
        query: &str,
        limit: usize,
        namespace: Option<ContextNamespace>,
    ) -> Result<Vec<SearchResult>>;

    /// 将 Abstract 和 Overview 文本 embed 为向量，存入向量库。
    async fn update_summary_vectors(
        &self,
        uri: &TianyanUri,
        abstract_content: &str,
        overview_content: &str,
    ) -> Result<()>;
}

/// 元数据管理 —— 重要性评分与自定义标签。
#[async_trait]
pub trait VfsMetadata: Send + Sync {
    /// 更新条目的重要性评分和自定义键值标签。
    async fn update_metadata(
        &self,
        uri: &TianyanUri,
        importance: f32,
        custom: HashMap<String, serde_json::Value>,
    ) -> Result<()>;

    /// 批量获取所有层级的内容元数据。
    async fn get_all_content_metadata(
        &self,
        uri: &TianyanUri,
    ) -> Result<HashMap<ContentLevel, ContentMetadata>>;
}

/// 组合超 trait —— 提供统一的 VirtualFileSystem 接口。
///
/// 任何同时实现了上述四个子 trait 的类型自动实现本 trait。
#[async_trait]
pub trait VirtualFileSystem: VfsCore + ContentStore + VfsSearch + VfsMetadata {}
```

- [ ] **Step 2: Add blanket impl for supertrait**

在超 trait 定义后立即添加：

```rust
// 当 VfsImpl 实现时自动获得 VirtualFileSystem
// （这里只声明 trait，不加 blanket impl —— 等 VirtualFileSystemImpl 实现了子 trait 后手动添加）
```

- [ ] **Step 3: Update storage/mod.rs re-exports**

修改 `core/src/storage/mod.rs`，新增子 trait 的 re-export：

```rust
pub use traits::{VfsCore, ContentStore, VfsSearch, VfsMetadata, VirtualFileSystem};
```

- [ ] **Step 4: Verify compilation**

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

Expected: No errors (old VirtualFileSystem trait still exists, new traits are additive).

- [ ] **Step 5: Commit**

```bash
git add core/src/storage/traits.rs core/src/storage/mod.rs
git commit -m "refactor(storage): define VfsCore/ContentStore/VfsSearch/VfsMetadata sub-traits"
```

---

### Phase 1: 实现新 trait 到 VirtualFileSystemImpl

### Task 1: Implement VfsCore on VirtualFileSystemImpl

**Files:**
- Modify: `core/src/storage/vfs/mod.rs`

- [ ] **Step 1: Add VfsCore implementation**

在 `core/src/storage/vfs/mod.rs` 中，在现有 impl 块之后添加。这些方法复用现有实现：

```rust
#[async_trait]
impl VfsCore for VirtualFileSystemImpl {
    async fn initialize(&self) -> Result<()> {
        self.storage.initialize().await?;
        self.vector_storage.initialize().await?;
        tracing::info!("虚拟文件系统已初始化");
        Ok(())
    }

    async fn exists(&self, uri: &TianyanUri) -> Result<bool> {
        self.storage.exists(uri).await
    }

    async fn delete(&self, uri: &TianyanUri) -> Result<()> {
        // 复用现有 delete 实现逻辑（递归删除 + 清理向量库）
        // 此处暂时委托给旧方法，Phase 6 时内联
        <Self as VirtualFileSystemOld>::delete(self, uri).await
    }

    async fn list(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>> {
        Self::validate_uri(uri)?;
        self.storage.list_directory(uri).await
    }

    async fn move_entry(&self, source: &TianyanUri, destination: &TianyanUri) -> Result<()> {
        <Self as VirtualFileSystemOld>::move_entry(self, source, destination).await
    }
}
```

> 注意：临时用 `VirtualFileSystemOld` 引用旧 trait。在最终清理阶段内联逻辑。

- [ ] **Step 2: Verify compilation**

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

Expected: No errors.

- [ ] **Step 3: Commit**

```bash
git add core/src/storage/vfs/mod.rs
git commit -m "feat(vfs): implement VfsCore trait on VirtualFileSystemImpl"
```

### Task 2: Implement ContentStore on VirtualFileSystemImpl

**Files:**
- Modify: `core/src/storage/vfs/mod.rs`

- [ ] **Step 1: Add ContentStore implementation**

```rust
#[async_trait]
impl ContentStore for VirtualFileSystemImpl {
    async fn write(&self, uri: &TianyanUri, level: ContentLevel, content: &str) -> Result<()> {
        Self::validate_uri(uri)?;

        // 条目不存在则自动创建
        if !self.storage.exists(uri).await? {
            let entry = ContextEntry::new_file(uri.clone());
            // 自动创建父目录
            if let Some(parent) = uri.parent() {
                if !self.storage.exists(&parent).await? {
                    self.storage
                        .write_entry(&ContextEntry::new_directory(parent.clone()))
                        .await?;
                }
            }
            self.storage.write_entry(&entry).await?;
        }

        self.storage.write_content(uri, level, content).await?;

        let mut entry = self.storage.read_entry(uri).await?;
        entry.metadata.touch();
        entry.set_content(level, content.to_string());
        let token_count = estimate_token_count(content);
        entry.token_counts.set(level, token_count);
        self.storage.write_entry(&entry).await?;

        tracing::trace!("已写入 {:?} 内容： {}", level, uri);
        Ok(())
    }

    async fn read(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String> {
        Self::validate_uri(uri)?;
        self.storage.read_content(uri, level).await
    }

    async fn append(&self, uri: &TianyanUri, content: &str) -> Result<()> {
        Self::validate_uri(uri)?;

        if !self.storage.exists(uri).await? {
            let entry = ContextEntry::new_file(uri.clone());
            if let Some(parent) = uri.parent() {
                if !self.storage.exists(&parent).await? {
                    self.storage
                        .write_entry(&ContextEntry::new_directory(parent.clone()))
                        .await?;
                }
            }
            self.storage.write_entry(&entry).await?;
        }

        let level = ContentLevel::Detail;
        self.storage.append_content(uri, level, content).await?;

        let mut entry = self.storage.read_entry(uri).await?;
        entry.metadata.touch();
        let existing = entry
            .content
            .get(&level)
            .cloned()
            .unwrap_or_default();
        let combined = format!("{}{}", existing, content);
        entry.set_content(level, combined);
        let token_count = estimate_token_count(&combined);
        entry.token_counts.set(level, token_count);
        self.storage.write_entry(&entry).await?;

        tracing::trace!("已追加内容： {}", uri);
        Ok(())
    }

    async fn has_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<bool> {
        let entry = self.storage.read_entry(uri).await?;
        Ok(entry.has_content(level))
    }
}
```

- [ ] **Step 2: Add ContentStore re-export to storage/mod.rs**

Already done in Task 0 Step 3.

- [ ] **Step 3: Verify compilation**

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add core/src/storage/vfs/mod.rs
git commit -m "feat(vfs): implement ContentStore trait on VirtualFileSystemImpl"
```

### Task 3: Implement VfsSearch on VirtualFileSystemImpl

**Files:**
- Modify: `core/src/storage/vfs/mod.rs`

> **重大语义变更**：`search` 方法从纯文本递归检索改为 embed → RRF 向量检索。

- [ ] **Step 1: Add VfsSearch implementation**

```rust
#[async_trait]
impl VfsSearch for VirtualFileSystemImpl {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        namespace: Option<ContextNamespace>,
    ) -> Result<Vec<SearchResult>> {
        let embedding_service = self
            .embedding_service
            .as_ref()
            .ok_or_else(|| TianyanError::Retrieval(
                "VFS 未配置嵌入服务，无法进行向量搜索".to_string(),
            ))?;

        let model = self
            .embedding_model
            .as_deref()
            .unwrap_or("text-embedding-3-small");

        // Step 1: Embed query text
        let embedding = embedding_service.embed_single(model, query).await?;
        let query_vector = embedding.vector;

        // Step 2: RRF fused search across Abstract + Overview vectors
        let category_filter = namespace.map(|ns| ns.to_string());
        let results = self
            .vector_storage
            .search_abstract_and_overview(
                query_vector,
                limit * 2, // 过采样以应对 namespace 过滤
                category_filter.as_deref(),
            )
            .await?;

        // Step 3: 转换为 SearchResult
        let results: Vec<SearchResult> = results
            .into_iter()
            .filter_map(|vsr| {
                let namespace = vsr.payload.uri.namespace();
                SearchResult::new(
                    vsr.payload.uri.clone(),
                    vsr.score,
                    namespace.to_string(),
                    vsr.payload.tags.clone(),
                )
                .into()
            })
            .collect();

        Ok(results)
    }

    async fn update_summary_vectors(
        &self,
        uri: &TianyanUri,
        abstract_content: &str,
        overview_content: &str,
    ) -> Result<()> {
        let embedding_service = self
            .embedding_service
            .as_ref()
            .ok_or_else(|| TianyanError::Retrieval(
                "VFS 未配置嵌入服务".to_string(),
            ))?;

        let model = self
            .embedding_model
            .as_deref()
            .unwrap_or("text-embedding-3-small");

        // Embed abstract
        let abstract_embedding = embedding_service.embed_single(model, abstract_content).await?;

        // Embed overview
        let overview_embedding = embedding_service.embed_single(model, overview_content).await?;

        // Update vectors
        let point_id = uri.to_string().replace("://", "_").replace('/', "_");
        let payload = EntryMetadata::new(uri.clone(), uri.namespace().to_string());
        let point = VectorPoint {
            schema_version: crate::storage::CURRENT_SCHEMA_VERSION,
            id: point_id,
            abstract_vector: Some(abstract_embedding.vector),
            overview_vector: Some(overview_embedding.vector),
            visual_vector: None,
            payload,
        };
        self.vector_storage.upsert_point(&point).await?;

        tracing::debug!("已更新向量：{}", uri);
        Ok(())
    }
}
```

- [ ] **Step 2: Add `use crate::common::types::EntryMetadata;` import**

在文件头部添加：

```rust
use crate::common::types::EntryMetadata;
```

- [ ] **Step 3: Verify compilation**

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

Expected: No errors. SearchResult 的构造需要确认签名——可能需要查看 `core/src/common/types/search.rs`。

- [ ] **Step 4: Commit**

```bash
git add core/src/storage/vfs/mod.rs
git commit -m "feat(vfs): implement VfsSearch trait with embed→RRF search"
```

### Task 4: Implement VfsMetadata on VirtualFileSystemImpl

**Files:**
- Modify: `core/src/storage/vfs/mod.rs`

- [ ] **Step 1: Add VfsMetadata implementation**

```rust
#[async_trait]
impl VfsMetadata for VirtualFileSystemImpl {
    async fn update_metadata(
        &self,
        uri: &TianyanUri,
        importance: f32,
        custom: HashMap<String, serde_json::Value>,
    ) -> Result<()> {
        let point_id = uri.to_string().replace("://", "_").replace('/', "_");

        // Get or create vector point
        let point = match self.vector_storage.get_point(&point_id).await? {
            Some(mut p) => {
                p.payload.importance = importance;
                for (k, v) in custom {
                    p.payload.custom.insert(k, v);
                }
                p
            }
            None => {
                let payload = EntryMetadata::new(uri.clone(), uri.namespace().to_string())
                    .with_importance(importance)
                    .with_custom(custom);
                VectorPoint {
                    schema_version: crate::storage::CURRENT_SCHEMA_VERSION,
                    id: point_id,
                    abstract_vector: None,
                    overview_vector: None,
                    visual_vector: None,
                    payload,
                }
            }
        };

        self.vector_storage.upsert_point(&point).await
    }

    async fn get_all_content_metadata(
        &self,
        uri: &TianyanUri,
    ) -> Result<HashMap<ContentLevel, ContentMetadata>> {
        let entry = self.storage.read_entry(uri).await?;
        let mut result = HashMap::new();

        for level in [ContentLevel::Abstract, ContentLevel::Overview, ContentLevel::Detail] {
            if entry.has_content(level) {
                // 读内容计算大小
                let content = self.storage.read_content(uri, level).await?;
                result.insert(
                    level,
                    ContentMetadata {
                        size: content.len() as u64,
                        created_at: entry.metadata.created_at,
                        updated_at: entry.metadata.updated_at,
                    },
                );
            }
        }

        Ok(result)
    }
}
```

- [ ] **Step 2: Verify compilation**

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

Expected: No errors.

- [ ] **Step 3: Commit**

```bash
git add core/src/storage/vfs/mod.rs
git commit -m "feat(vfs): implement VfsMetadata trait on VirtualFileSystemImpl"
```

### Task 5: Implement supertrait VirtualFileSystem

**Files:**
- Modify: `core/src/storage/vfs/mod.rs`

- [ ] **Step 1: Add VirtualFileSystem blanket impl**

在 VirtualFileSystemImpl 的 4 个子 trait impl 之后添加：

```rust
impl VirtualFileSystem for VirtualFileSystemImpl {}
```

- [ ] **Step 2: Verify compilation**

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

Expected: No errors.

- [ ] **Step 3: Commit**

```bash
git add core/src/storage/vfs/mod.rs
git commit -m "feat(vfs): implement VirtualFileSystem supertrait"
```

---

### Phase 2: 迁移调用方（按模块逐个迁移）

迁移原则：每个调用方改为只依赖其实际使用的子 trait。Agent 等需要全功能的继续使用 `Arc<dyn VirtualFileSystem>`。

### Task 6: Migrate Agent (coordinator + builder)

**Files:**
- Modify: `core/src/agent/coordinator.rs`
- Modify: `core/src/agent/builder.rs`

Agent 使用全部功能，保持 `vfs: Arc<dyn VirtualFileSystem>`。只需确保新的 trait 路径正确。

- [ ] **Step 1: Verify coordinator.rs uses are compatible**

检查 `core/src/agent/coordinator.rs` 中的 VFS 方法调用是否都能映射到新 trait：
- `vfs.initialize()` → `VfsCore`
- `vfs.write_content()` → `ContentStore::write` (需改签名)
- `vfs.read_content()` → `ContentStore::read`
- `vfs.search()` → `VfsSearch` (新语义)

由于 `VirtualFileSystem` 超 trait 组合了全部子 trait，`Arc<dyn VirtualFileSystem>` 可调用所有方法。coordinator.rs 无需修改 use 语句（它已导入 `VirtualFileSystem`）。

- [ ] **Step 2: Verify builder.rs uses are compatible**

同理，`core/src/agent/builder.rs` 只需确认 `Arc<dyn VirtualFileSystem>` 类型正常流通。

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

Expected: No errors specific to agent module.

- [ ] **Step 3: Commit**

```bash
git add core/src/agent/
git commit -m "refactor(agent): verify compatibility with new VFS sub-traits"
```

### Task 7: Migrate ContextPipeline

**Files:**
- Modify: `core/src/context/pipeline.rs`

ContextPipeline 仅使用 `read` 和 `list`，可以缩小依赖到 `ContentStore + VfsCore`。但为简单起见，保持 `Arc<dyn VirtualFileSystem>`。

- [ ] **Step 1: Confirm compilation**

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

No changes needed — `VirtualFileSystem` 超 trait 包含所有方法。

- [ ] **Step 2: Commit**

```bash
git commit --allow-empty -m "refactor(context): pipeline confirmed compatible with new Vfs traits"
```

### Task 8: Migrate SessionManager

**Files:**
- Modify: `core/src/session/manager.rs`

SessionManager 使用 `initialize`, `exists`, `create_directory`, `write_content`, `append_content`, `read_content`, `update_metadata`, `delete`, `list`。映射：
- `create_directory` → 移除（条目在 `write` 时自动创建）
- `write_content` → `write` (ContentStore)
- `read_content` → `read` (ContentStore)
- `append_content` → `append` (ContentStore)
- `update_metadata` → `update_metadata` (VfsMetadata)

- [ ] **Step 1: Update session/manager.rs method calls**

找到所有 VFS 方法调用并重命名：
- `vfs.write_content(uri, content)` → `vfs.write(uri, ContentLevel::Detail, content)`
- `vfs.read_content(uri, ContentLevel::Detail)` → `vfs.read(uri, ContentLevel::Detail)`
- `vfs.append_content(uri, content)` → `vfs.append(uri, content)`
- `vfs.create_directory(uri)` → 删除此行（write 自动创建）

具体改动位置（需要读取文件确认行号）：
- 第 163 行 `create_directory` → 删除
- 第 73 行 `read_content` → `read`
- 第 118 行 `append_content` → `append`
- 第 204 行 `read_content` → `read`
- 第 126 行 `update_metadata` → 不变
- 第 250 行 `delete` → 不变

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

- [ ] **Step 2: Commit**

```bash
git add core/src/session/manager.rs
git commit -m "refactor(session): migrate to new Vfs trait method names"
```

### Task 9: Migrate RuleRecorder and RuleSuggester

**Files:**
- Modify: `core/src/context/rule_recorder.rs`
- Modify: `core/src/context/rule_suggester.rs`

- [ ] **Step 1: Update rule_recorder.rs method calls**

映射：
- `vfs.create_file(uri)` → 删除（write 自动创建）
- `vfs.write_content(uri, content)` → `vfs.write(uri, ContentLevel::Detail, content)`
- `vfs.write_abstract(uri, content)` → `vfs.write(uri, ContentLevel::Abstract, content)`
- `vfs.read_content(uri, level)` → `vfs.read(uri, level)`

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

- [ ] **Step 2: Update rule_suggester.rs method calls**

映射：
- `vfs.exists(uri)` → 不变
- `vfs.list(uri)` → 不变
- `vfs.read_content(uri, level)` → `vfs.read(uri, level)`

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

- [ ] **Step 3: Commit**

```bash
git add core/src/context/rule_recorder.rs core/src/context/rule_suggester.rs
git commit -m "refactor(context): migrate rule recorder/suggester to new Vfs methods"
```

### Task 10: Migrate SkillsManager

**Files:**
- Modify: `core/src/skills/manager.rs`

SkillsManager 使用 `list`, `read_file`, `file_exists`, `list_files`, `read_abstract`。
- `read_file(dir, filename)` → 替换为 URI 拼接：`read(uri.join(filename), ContentLevel::Detail)`
- `file_exists(dir, filename)` → 替换为 `exists(uri.join(filename))`
- `list_files(dir)` → 替换为 `list(dir)` + 过滤
- `read_abstract(uri)` → 替换为 `read(uri, ContentLevel::Abstract)`

- [ ] **Step 1: Update skills/manager.rs**

读取当前文件，执行上述替换。需要检查 `TianyanUri` 是否有 `join` 方法：

```rust
// read_file(dir, filename) → 
let child_uri = uri.clone().join(&[filename]);
vfs.read(&child_uri, ContentLevel::Detail).await

// file_exists(dir, filename) →
let child_uri = uri.clone().join(&[filename]);
vfs.exists(&child_uri).await

// list_files(dir) →
let entries = vfs.list(uri).await?;
let files: Vec<String> = entries
    .iter()
    .filter(|e| e.is_file())
    .map(|e| e.uri.path_segments().last().unwrap_or_default().to_string())
    .collect();

// read_abstract(uri) →
vfs.read(uri, ContentLevel::Abstract).await
```

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

- [ ] **Step 2: Commit**

```bash
git add core/src/skills/manager.rs
git commit -m "refactor(skills): migrate SkillsManager to new Vfs traits"
```

### Task 11: Migrate storage sub-modules (extractor, summary_service, summary)

**Files:**
- Modify: `core/src/storage/extractor.rs`
- Modify: `core/src/storage/summary_service.rs`
- Modify: `core/src/storage/summary.rs`
- Modify: `core/src/tasks/summary_task.rs`
- Modify: `core/src/tasks/memory_task.rs`
- Modify: `core/src/tasks/gc_task.rs`

- [ ] **Step 1: Update extractor.rs**

方法映射：
- `vfs.exists(uri)` → 不变
- `vfs.create_directory(uri)` → 删除（write 自动创建）
- `vfs.write_content(uri, content)` → `vfs.write(uri, ContentLevel::Detail, content)`
- `vfs.write_abstract(uri, content)` → `vfs.write(uri, ContentLevel::Abstract, content)`
- `vfs.read_content(uri, level)` → `vfs.read(uri, level)`

- [ ] **Step 2: Update summary_service.rs**

方法映射：
- `vfs.list(uri)` → 不变
- `vfs.get_all_content_metadata(uri)` → 不变
- `vfs.write_abstract(uri, content)` → `vfs.write(uri, ContentLevel::Abstract, content)`
- `vfs.write_overview(uri, content)` → `vfs.write(uri, ContentLevel::Overview, content)`
- `vfs.read_content(uri, level)` → `vfs.read(uri, level)`
- `vfs.update_summary_vectors(uri, abs, ov)` → 不变

- [ ] **Step 3: Update tasks (summary_task, memory_task, gc_task)**

summary_task.rs 映射：
- `vfs.list(uri)` → 不变
- `vfs.read_content(uri, level)` → `vfs.read(uri, level)`
- `vfs.get_all_content_metadata(uri)` → 不变
- `vfs.write_abstract(uri, content)` → `vfs.write(uri, ContentLevel::Abstract, content)`
- `vfs.write_overview(uri, content)` → `vfs.write(uri, ContentLevel::Overview, content)`
- `vfs.update_summary_vectors(uri, abs, ov)` → 不变

memory_task.rs 映射：
- `vfs.list(uri)` → 不变

gc_task.rs 映射：
- `vfs.list(uri)` → 不变
- `vfs.delete(uri)` → 不变
- `vfs.move_entry(src, dst)` → 不变

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

- [ ] **Step 4: Commit**

```bash
git add core/src/storage/extractor.rs core/src/storage/summary_service.rs core/src/storage/summary.rs core/src/tasks/
git commit -m "refactor(storage): migrate extractor/summary/tasks to new Vfs methods"
```

### Task 12: Migrate scheduler/task_scheduler

**Files:**
- Modify: `core/src/scheduler/task_scheduler.rs`

TaskContext 持有 `vfs: Arc<dyn VirtualFileSystem>`，不需要改动。

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

- [ ] **Step 1: Commit**

```bash
git commit --allow-empty -m "refactor(scheduler): confirmed compatible with new Vfs traits"
```

### Task 13: Migrate server/src

**Files:**
- Modify: `server/src/agent_builder.rs`
- Modify: `server/src/state.rs`
- Modify: `server/src/lib.rs`

- [ ] **Step 1: Update server/src/agent_builder.rs**

关键变更：`DualLayerRetriever` 不再需要 `Arc<dyn VectorStorage>`。改为传入 VFS：

```rust
// 旧代码（约第 74 行）：
let retriever = DualLayerRetriever::new(vfs.get_vector_storage());

// 新代码：
let retriever = DualLayerRetriever::new(Arc::clone(&vfs));
```

`DualLayerRetriever::new()` 签名将在 Task 14 中修改。

- [ ] **Step 2: Update server/src/state.rs**

确认 `AppState` 中的 `vfs: Arc<dyn VirtualFileSystem>` 字段不受影响。

- [ ] **Step 3: Update server/src/lib.rs**

确认 `initialize_vfs_for_app()` 返回类型不变。

```powershell
cargo check -p tianyan-server 2>&1 | Select-String "error"
```

Expected: Error — `DualLayerRetriever::new(vfs)` 类型不匹配（retriever 尚未修改）。这是预期的，将在 Task 14 修复。

- [ ] **Step 4: Commit**

```bash
git add server/src/agent_builder.rs
git commit -m "refactor(server): prepare agent_builder for new DualLayerRetriever API"
```

---

### Phase 3: 迁移 DualLayerRetriever

### Task 14: Migrate DualLayerRetriever to use VFS

**Files:**
- Modify: `core/src/context/retrieval/retriever.rs`

**重大变更**：DualLayerRetriever 不再持有 `Arc<dyn VectorStorage>` 和 `Arc<dyn EmbeddingService>`。改为持有 `Arc<dyn VirtualFileSystem>`。搜索通过 `vfs.search()` 完成，内容加载通过 `vfs.read()`。

- [ ] **Step 1: Change DualLayerRetriever fields**

```rust
pub struct DualLayerRetriever {
    /// VFS 用于向量搜索和内容加载。
    vfs: Arc<dyn crate::storage::VirtualFileSystem>,
    /// 内容加载器（可选——VFS 也可直接 read）。
    content_loader: Option<Arc<dyn ContentLoader>>,
    /// 意图分析器。
    intent_analyzer: IntentAnalyzer,
    /// 使用的嵌入模型。
    embedding_model: String,
    /// 默认 Token 预算。
    default_token_budget: usize,
    /// Memory 命名空间的分数偏置倍数。
    memory_bias: f32,
    /// 记忆衰减率（每日，0.0-1.0）。
    memory_decay_rate: f32,
}
```

移除了 `vector_storage` 和 `embedding_service` 字段。

- [ ] **Step 2: Update DualLayerRetriever::new()**

```rust
pub fn new(vfs: Arc<dyn crate::storage::VirtualFileSystem>) -> Self {
    Self {
        vfs,
        content_loader: None,
        intent_analyzer: IntentAnalyzer::new(),
        embedding_model: "text-embedding-3-small".to_string(),
        default_token_budget: 4096,
        memory_bias: 1.15,
        memory_decay_rate: 0.01,
    }
}
```

- [ ] **Step 3: Remove with_embedding_service() method**

该 builder 方法不再需要——VFS 内部已持有嵌入服务。

- [ ] **Step 4: Update get_query_vector() to delegate to VFS**

VFS 的 `search()` 自己处理 embed，所以 retriever 不再需要 `get_query_vector()`。直接调用 `vfs.search()` 即可。删除 `get_query_vector()` 方法。

- [ ] **Step 5: Update fused_search()**

```rust
async fn fused_search(
    &self,
    query: &str,
    top_k: usize,
    intent: &Intent,
) -> Result<Vec<RetrievalResult>> {
    let namespace = intent.namespace();
    let results = self
        .vfs
        .search(query, top_k, namespace)
        .await?;

    let mut results: Vec<RetrievalResult> = results
        .into_iter()
        .map(|sr| {
            let uri = sr.uri.clone();
            let mut rr = RetrievalResult::new(uri, sr.score);
            rr.category = sr.namespace.clone();
            rr
        })
        .collect();

    // Memory bias
    for result in &mut results {
        if result.category == "memory" {
            result.score *= self.memory_bias;
        }
    }

    // Freshness scoring
    for result in &mut results {
        if let Some(freshness) = result.freshness_score {
            result.score = compute_combined_score(result.score, freshness);
        }
    }

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(results)
}
```

- [ ] **Step 6: Update load_content methods to use VFS**

```rust
async fn load_content_for_results(
    &self,
    results: Vec<RetrievalResult>,
) -> Result<Vec<RetrievalResult>> {
    let mut loaded_results = Vec::with_capacity(results.len());

    for mut result in results {
        let level = ContentLoadStrategy::from_score(result.score).to_content_level();
        match self.vfs.read(&result.uri, level).await {
            Ok(content) => {
                result.content = Some(content.clone());
                result.content_level = level;
                result.token_count = estimate_tokens(&content);
            }
            Err(e) => {
                tracing::warn!(uri = %result.uri, error = %e, "加载内容失败");
                if level != ContentLevel::Abstract {
                    if let Ok(content) = self.vfs.read(&result.uri, ContentLevel::Abstract).await {
                        result.content = Some(content.clone());
                        result.content_level = ContentLevel::Abstract;
                        result.token_count = estimate_tokens(&content);
                    }
                }
            }
        }
        loaded_results.push(result);
    }

    Ok(loaded_results)
}
```

- [ ] **Step 7: Update search_by_visual()**

视觉搜索需要 `VectorStorage::search(VectorType::Visual)`。这是 VFS 未暴露的能力。暂时保留直接访问 `vector_storage` 的能力——改为通过 VFS 内部（VFS 新增一个 `search_by_visual` 方法，或暂时在 retriever 中保留 `vector_storage` 字段仅用于视觉搜索）。

**决定**：在 VfsSearch trait 上新增方法：

```rust
async fn search_by_visual(
    &self,
    visual_vector: &[f32],
    top_k: usize,
) -> Result<Vec<SearchResult>>;
```

并在 VirtualFileSystemImpl 中实现。

更新 retriever 中的 `search_by_visual()`：
```rust
pub async fn search_by_visual(
    &self,
    visual_vector: &[f32],
    top_k: usize,
) -> Result<Vec<RetrievalResult>> {
    let results = self.vfs.search_by_visual(visual_vector, top_k).await?;
    // ... 转换为 RetrievalResult
}
```

- [ ] **Step 8: Update DualLayerRetrieverBuilder**

移除 `with_vector_storage` 和 `with_embedding_service`。添加 `with_vfs`。

```rust
pub struct DualLayerRetrieverBuilder {
    vfs: Option<Arc<dyn crate::storage::VirtualFileSystem>>,
    content_loader: Option<Arc<dyn ContentLoader>>,
    // ...
}

impl DualLayerRetrieverBuilder {
    pub fn with_vfs(mut self, vfs: Arc<dyn crate::storage::VirtualFileSystem>) -> Self {
        self.vfs = Some(vfs);
        self
    }

    pub fn build(self) -> Result<DualLayerRetriever> {
        let vfs = self.vfs
            .ok_or_else(|| TianyanError::Internal("VFS 不可用".to_string()))?;
        let mut retriever = DualLayerRetriever::new(vfs);
        // ... content_loader, model, budget ...
        Ok(retriever)
    }
}
```

- [ ] **Step 9: Update server/src/agent_builder.rs to use the new builder API**

```rust
// 旧：
let retriever = DualLayerRetriever::new(vfs.get_vector_storage());

// 新：
let retriever = DualLayerRetrieverBuilder::new()
    .with_vfs(Arc::clone(&vfs))
    .build()?;
```

- [ ] **Step 10: Update tests in retriever.rs**

测试需要从 MockVectorStorage 改为 MockVfs。利用现有的 `MockVfs`（在 `core/src/skills/learning/mod.rs` 或 storage/mod.rs）。

```rust
// 创建带 VFS 的 retriever：
let mock_vfs = Arc::new(MockVfs::new());
let retriever = DualLayerRetrieverBuilder::new()
    .with_vfs(mock_vfs)
    .build()
    .unwrap();
```

- [ ] **Step 11: Add search_by_visual to VfsSearch trait and VirtualFileSystemImpl**

在 `core/src/storage/traits.rs` 的 `VfsSearch` trait 中添加：

```rust
async fn search_by_visual(
    &self,
    visual_vector: &[f32],
    top_k: usize,
) -> Result<Vec<SearchResult>>;
```

在 `core/src/storage/vfs/mod.rs` 中实现：

```rust
async fn search_by_visual(
    &self,
    visual_vector: &[f32],
    top_k: usize,
) -> Result<Vec<SearchResult>> {
    let query = VectorSearchQuery {
        vector: visual_vector.to_vec(),
        vector_type: VectorType::Visual,
        limit: top_k,
        category_filter: None,
        min_score: None,
    };
    let results = self.vector_storage.search(query).await?;
    // 转换为 SearchResult...
    Ok(results.into_iter().map(|r| SearchResult { ... }).collect())
}
```

- [ ] **Step 12: Verify compilation**

```powershell
cargo check -p tianyan-core 2>&1 | Select-String "error"
```

Expected: test errors due to mock changes, no production code errors.

- [ ] **Step 13: Commit**

```bash
git add core/src/context/retrieval/retriever.rs core/src/storage/traits.rs core/src/storage/vfs/mod.rs server/src/agent_builder.rs
git commit -m "refactor(retriever): migrate DualLayerRetriever to use VFS instead of VectorStorage+EmbeddingService"
```

---

### Phase 4: 迁移 ContentLoader

### Task 15: Remove ContentLoader trait, migrate callers

**Files:**
- Modify: `core/src/storage/traits.rs` (remove ContentLoader trait)
- Modify: `core/src/storage/vfs/mod.rs` (remove ContentLoader impl)
- Modify: `core/src/context/retrieval/loader.rs`
- Modify: `core/src/context/retrieval/retriever.rs`
- Modify: `core/src/storage/mod.rs` (remove ContentLoader re-export)

ContentLoader 的 `load_content`, `load_entry`, `has_content` 已分别被 `ContentStore::read`, `VfsCore::list`(过滤), `ContentStore::has_content` 覆盖。

- [ ] **Step 1: Remove ContentLoader trait from storage/traits.rs**

删除第 441-454 行的 `ContentLoader` trait 定义。

- [ ] **Step 2: Remove ContentLoader impl from vfs/mod.rs**

删除第 919 行开始的 `impl ContentLoader for VirtualFileSystemImpl` 块。

- [ ] **Step 3: Update loader.rs**

ContentLoaderImpl 的 `inner: Arc<dyn ContentLoader>` 改为 `vfs: Arc<dyn VirtualFileSystem>`（或更精确的 `Arc<dyn ContentStore>`）。

```rust
pub struct ContentLoaderImpl {
    vfs: Arc<dyn crate::storage::ContentStore>,
    budget: TokenBudget,
    token_counter: Option<TokenCounter>,
}

impl ContentLoaderImpl {
    pub fn new(vfs: Arc<dyn crate::storage::ContentStore>) -> Self { ... }
    pub fn with_budget(vfs: Arc<dyn crate::storage::ContentStore>, budget: TokenBudget) -> Self { ... }

    // load_with_strategy:
    let content = self.vfs.read(uri, level).await?;

    // has_content_level:
    self.vfs.has_content(uri, level).await

    // get_best_available_level:
    if self.vfs.has_content(uri, preferred_level).await? { ... }
}
```

- [ ] **Step 4: Update loader.rs tests**

Mock 改为实现 `ContentStore` 而非 `ContentLoader`：

```rust
#[async_trait]
impl crate::storage::ContentStore for MockContentStore {
    async fn write(&self, _uri: &TianyanUri, _level: ContentLevel, _content: &str) -> Result<()> { Ok(()) }
    async fn read(&self, _uri: &TianyanUri, level: ContentLevel) -> Result<String> { ... }
    async fn append(&self, _uri: &TianyanUri, _content: &str) -> Result<()> { Ok(()) }
    async fn has_content(&self, _uri: &TianyanUri, _level: ContentLevel) -> Result<bool> { Ok(true) }
}
```

- [ ] **Step 5: Update retriever.rs tests**

Mock 测试中的 `MockContentLoader` → `MockContentStore`。

- [ ] **Step 6: Update storage/mod.rs**

移除 `ContentLoader` 的 re-export，添加 `ContentStore` 的 re-export（已在 Task 0 完成）。

- [ ] **Step 7: Verify compilation**

```powershell
cargo check --workspace 2>&1 | Select-String "error"
```

Expected: No errors.

- [ ] **Step 8: Commit**

```bash
git add core/src/storage/traits.rs core/src/storage/vfs/mod.rs core/src/context/retrieval/loader.rs core/src/context/retrieval/retriever.rs core/src/storage/mod.rs
git commit -m "refactor(storage): remove ContentLoader trait, migrate to ContentStore"
```

---

### Phase 5: 清理 —— 移除旧 VirtualFileSystem trait

### Task 16: Remove old VirtualFileSystem trait methods

**Files:**
- Modify: `core/src/storage/traits.rs` (remove old VirtualFileSystem trait entirely)
- Modify: `core/src/storage/vfs/mod.rs` (remove old impl block)
- Modify: `core/src/skills/learning/mod.rs` (update MockVfs)
- Modify: `core/src/storage/mod.rs` (update re-exports, update MockVectorStorage, remove old test helpers)

- [ ] **Step 1: Remove old VirtualFileSystem trait from traits.rs**

删除第 294-439 行的整个旧 `VirtualFileSystem` trait。现在只有 4 个子 trait + 超 trait。

- [ ] **Step 2: Remove old impl block from vfs/mod.rs**

删除旧 `impl VirtualFileSystem for VirtualFileSystemImpl` 块（第 247 行开始）。此 impl 已在子 trait impl 中覆盖。

- [ ] **Step 3: Update MockVfs in skills/learning/mod.rs**

MockVfs 需要实现全部 4 个子 trait：

```rust
#[async_trait]
impl VfsCore for MockVfs { ... }
#[async_trait]
impl ContentStore for MockVfs { ... }
#[async_trait]
impl VfsSearch for MockVfs { ... }
#[async_trait]
impl VfsMetadata for MockVfs { ... }
impl VirtualFileSystem for MockVfs {}
```

移除 `storage_backend()`, `vector_storage()`, `get_vector_storage()` 方法。

- [ ] **Step 4: Update storage/mod.rs**

移除旧的 VirtualFileSystem re-export（已由超 trait 覆盖）。更新 `MockVectorStorage` 以匹配新 trait 结构。

更新 `SharedVfs` 类型别名：
```rust
pub type SharedVfs = Arc<dyn VirtualFileSystem>;
```

- [ ] **Step 5: Remove backend accessor impls from vfs/mod.rs**

删除 `storage_backend()`, `vector_storage()`, `get_vector_storage()` 方法（保留在 VirtualFileSystemImpl 但不再从 trait 暴露）。

保留 `VirtualFileSystemImpl` 内部的 `self.storage` 和 `self.vector_storage` 字段访问（不通过 trait）。

- [ ] **Step 6: Verify compilation**

```powershell
cargo check --workspace 2>&1 | Select-String "error"
```

Expected: No errors. All old trait references must be resolved.

- [ ] **Step 7: Commit**

```bash
git add core/src/storage/ core/src/skills/learning/mod.rs
git commit -m "refactor(vfs): remove old VirtualFileSystem trait, fully migrate to sub-traits"
```

---

### Phase 6: 测试修复与最终验证

### Task 17: Fix all tests and run full test suite

**Files:**
- Modify: `core/src/storage/mod.rs` (tests)
- Modify: `core/src/storage/vfs/builder.rs` (tests)
- Modify: `core/src/context/retrieval/retriever.rs` (tests)
- Modify: `core/src/context/retrieval/loader.rs` (tests)
- Modify: `core/src/skills/learning/mod.rs` (tests)
- Modify: `core/tests/structural.rs` (update trait references if any)

- [ ] **Step 1: Run core unit tests to see failures**

```powershell
cargo test -p tianyan-core --lib 2>&1 | Select-String "FAILED|error"
```

- [ ] **Step 2: Fix storage/mod.rs tests**

更新 `create_test_vfs` 辅助函数和所有测试用例，使用新 trait 方法名。

- [ ] **Step 3: Fix storage/vfs/builder.rs tests**

更新测试中的 VFS 方法调用。

- [ ] **Step 4: Fix retriever.rs tests**

Mock 类型更新。测试中的 `MockVectorStorage` → `MockVfs`。

- [ ] **Step 5: Fix loader.rs tests**

MockContentStore 更新。

- [ ] **Step 6: Fix skills/learning/mod.rs tests**

MockVfs 更新为子 trait 实现。

- [ ] **Step 7: Run full test suite**

```powershell
cargo test -p tianyan-core --lib -- --nocapture
```

Expected: All tests pass.

- [ ] **Step 8: Run integration tests**

```powershell
cargo test -p tianyan-core --test '*' -- --nocapture
cargo test -p tianyan-server -- --nocapture
```

Expected: All tests pass.

- [ ] **Step 9: Run full lint**

```powershell
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

Expected: No warnings.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "test: fix all tests for Vfs sub-trait migration"
```

---

### Task 18: Final verification checklist

- [ ] **Step 1: cargo check --workspace**

```powershell
cargo check --workspace
```

- [ ] **Step 2: cargo test --workspace**

```powershell
cargo test --workspace -- --nocapture
```

- [ ] **Step 3: cargo clippy --workspace**

```powershell
cargo clippy --workspace -- -D warnings
```

- [ ] **Step 4: cargo fmt --all**

```powershell
cargo fmt --all -- --check
```

- [ ] **Step 5: Run structural tests**

```powershell
cargo test -p tianyan-core --test structural -- --nocapture
```

- [ ] **Step 6: Commit**

```bash
git commit --allow-empty -m "chore: final verification of Vfs trait deepening"
```

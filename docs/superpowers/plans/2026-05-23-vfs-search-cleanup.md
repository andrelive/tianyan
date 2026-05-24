# VFS 搜索路径统一：删除暴力检索 + 修复向量库 namespace 过滤

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 删除 VFS 中的全文暴力递归搜索路径，将 `VfsSearch::search()` 保留为唯一搜索入口，同时修复 Qdrant 向量库中 namespace 过滤字段名 bug

**Architecture:** `VfsSearch::search()` 已经接受 `Option<ContextNamespace>` 参数，内部通过 Qdrant 的 payload 过滤实现按 namespace 检索。但过滤字段名写错了（`"category"` 应为 `"namespace"`），导致从未生效。修复后，无需额外组件即可实现按 namespace 的语义搜索

**Tech Stack:** Rust, Qdrant (qdrant-client crate), async-trait

---

## File Structure

| 文件 | 操作 | 职责 |
|------|------|------|
| `core/src/storage/vector/qdrant.rs` | Modify | 修复 Qdrant payload 过滤字段名 |
| `core/src/storage/traits.rs` | Modify | 删除 `search_by_namespace` 等 5 个死方法 |
| `core/src/storage/vfs/mod.rs` | Modify | 删除 `search_recursive` 和 `search_by_namespace` override |
| `core/src/storage/vfs/tests.rs` | Modify | 删除 `test_search_by_namespace` 全文搜索测试 |

---

### Task 1: Fix Qdrant namespace filter field name

**Files:**
- Modify: `core/src/storage/vector/qdrant.rs:310-313`
- Modify: `core/src/storage/vector/qdrant.rs:395-398`

**Context:** Qdrant payload 中 namespace 存储在 `"namespace"` 字段（见 `EntryMetadata::to_qdrant_payload()` in `core/src/common/types/metadata.rs:124-127`），但搜索过滤器却错用了 `"category"` 字段名，导致按 namespace 过滤从未生效。

- [ ] **Step 1: Fix `search_fused` method filter**

Change `core/src/storage/vector/qdrant.rs` line 311 from `"category"` to `"namespace"`:

```rust
        // 添加类别过滤
        if let Some(cat) = category_filter {
            query_builder = query_builder.filter(Filter::must([Condition::matches(
                "namespace",
                cat.to_string(),
            )]));
        }
```

- [ ] **Step 2: Fix private `search` method filter**

Change `core/src/storage/vector/qdrant.rs` line 396 from `"category"` to `"namespace"`:

```rust
        // 如果指定了类别则添加过滤
        if let Some(cat) = category_filter {
            search_builder = search_builder.filter(Filter::must([Condition::matches(
                "namespace",
                cat.to_string(),
            )]));
        }
```

- [ ] **Step 3: Verify compilation**

```powershell
cargo check --workspace
```

Expected: No errors (only pre-existing `missing_docs` warnings).

- [ ] **Step 4: Commit**

```bash
git add core/src/storage/vector/qdrant.rs
git commit -m "fix(storage): use correct Qdrant payload field 'namespace' for category filter"
```

---

### Task 2: Delete dead full-text search path

**Files:**
- Modify: `core/src/storage/traits.rs:454-486`
- Modify: `core/src/storage/vfs/mod.rs:252-342`
- Modify: `core/src/storage/vfs/mod.rs:687-698`
- Modify: `core/src/storage/vfs/tests.rs:157-222`

**Context:** 全文递归搜索路径（`search_by_namespace` → `search_recursive`）没有任何外部调用方。所有外部搜索都走 `VfsSearch::search()` 向量搜索路径。删除后可消除两套搜索的歧义。

- [ ] **Step 1: Delete 5 methods from `VirtualFileSystem` trait**

In `core/src/storage/traits.rs`, delete lines 454-486 (the block containing `search_by_namespace`, `search_session`, `search_memory`, `search_knowledge`, `search_skill`):

```rust
    /// 按命名空间搜索。
    async fn search_by_namespace(
        &self,
        _namespace: ContextNamespace,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<SearchResult>> {
        Ok(vec![])
    }

    /// 搜索会话。
    async fn search_session(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        self.search_by_namespace(ContextNamespace::Session, query, limit)
            .await
    }

    /// 搜索记忆。
    async fn search_memory(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        self.search_by_namespace(ContextNamespace::Memory, query, limit)
            .await
    }

    /// 搜索知识。
    async fn search_knowledge(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        self.search_by_namespace(ContextNamespace::Knowledge, query, limit)
            .await
    }

    /// 搜索技能。
    async fn search_skill(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        self.search_by_namespace(ContextNamespace::Skill, query, limit)
            .await
    }
```

Result: the `VirtualFileSystem` trait now ends with `copy_entry`, then `regenerate_metadata`, then `list_all_uris`. The preceding method (`read_file` at ~line 453) is unchanged.

- [ ] **Step 2: Delete `search_recursive` private method from `VirtualFileSystemImpl`**

In `core/src/storage/vfs/mod.rs`, delete lines 252-342 (the entire `impl VirtualFileSystemImpl { ... search_recursive ... }` block). This is the `impl VirtualFileSystemImpl` block that contains only `search_recursive`:

```rust
impl VirtualFileSystemImpl {
    /// 递归搜索匹配查询的条目。
    async fn search_recursive(
        &self,
        uri: &TianyanUri,
        query: &str,
        results: &mut Vec<SearchResult>,
        limit: usize,
    ) -> Result<()> {
        if results.len() >= limit {
            return Ok(());
        }

        if !self.storage.exists(uri).await? {
            return Ok(());
        }

        let entry = self.storage.read_entry(uri).await?;

        if entry.is_directory() {
            let query_lower = query.to_lowercase();
            let mut score = 0.0f32;

            if let Some(ref content) = entry.detail_content {
                if content.to_lowercase().contains(&query_lower) {
                    score += 0.5;
                }
            }

            if let Some(ref content) = entry.overview_content {
                if content.to_lowercase().contains(&query_lower) {
                    score += 0.3;
                }
            }

            if let Some(ref content) = entry.abstract_content {
                if content.to_lowercase().contains(&query_lower) {
                    score += 0.2;
                }
            }

            if score > 0.0 {
                results.push(SearchResult {
                    uri: entry.metadata.uri.clone(),
                    score,
                    matched_level: ContentLevel::Detail,
                    content: entry.detail_content.clone(),
                });
            }

            let children = self.storage.list_directory(uri).await?;
            for child in children {
                Box::pin(self.search_recursive(child.uri(), query, results, limit)).await?;
                if results.len() >= limit {
                    return Ok(());
                }
            }
        } else {
            let query_lower = query.to_lowercase();
            let mut score = 0.0f32;

            if let Some(ref content) = entry.detail_content {
                if content.to_lowercase().contains(&query_lower) {
                    score += 0.5;
                }
            }

            if let Some(ref content) = entry.overview_content {
                if content.to_lowercase().contains(&query_lower) {
                    score += 0.3;
                }
            }

            if let Some(ref content) = entry.abstract_content {
                if content.to_lowercase().contains(&query_lower) {
                    score += 0.2;
                }
            }

            if score > 0.0 {
                results.push(SearchResult {
                    uri: entry.metadata.uri.clone(),
                    score,
                    matched_level: ContentLevel::Detail,
                    content: entry.detail_content.clone(),
                });
            }
        }

        Ok(())
    }
}
```

Note: The main `impl VirtualFileSystemImpl { ... }` block (which contains `new`, `with_defaults`, etc.) starts before this and is NOT deleted. Only this secondary `impl VirtualFileSystemImpl` block containing `search_recursive` is removed.

- [ ] **Step 3: Delete `search_by_namespace` override from `impl VirtualFileSystem for VirtualFileSystemImpl`**

In `core/src/storage/vfs/mod.rs`, delete lines 687-698 (the `search_by_namespace` override method):

```rust
        namespace: ContextNamespace,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let root_uri = TianyanUri::new(namespace, vec![]);
        let mut results = Vec::new();
        self.search_recursive(&root_uri, query, &mut results, limit)
            .await?;
        results.truncate(limit);
        Ok(results)
    }
```

Result: The `impl VirtualFileSystem for VirtualFileSystemImpl` block now contains only `copy_entry`.

- [ ] **Step 4: Delete `test_search_by_namespace` test**

In `core/src/storage/vfs/tests.rs`, delete lines 157-222 (the entire `test_search_by_namespace` function):

```rust
#[tokio::test]
async fn test_search_by_namespace() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let session_uri = TianyanUri::new(ContextNamespace::Session, vec!["session1".to_string()]);
    vfs.create_file(&session_uri).await.unwrap();
    vfs.write(&session_uri, ContentLevel::Detail, "会话测试内容")
        .await
        .unwrap();

    let memory_uri = TianyanUri::new(ContextNamespace::Memory, vec!["memory1".to_string()]);
    vfs.create_file(&memory_uri).await.unwrap();
    vfs.write(&memory_uri, ContentLevel::Detail, "记忆测试内容")
        .await
        .unwrap();

    let knowledge_uri =
        TianyanUri::new(ContextNamespace::Knowledge, vec!["knowledge1".to_string()]);
    vfs.create_file(&knowledge_uri).await.unwrap();
    vfs.write(&knowledge_uri, ContentLevel::Detail, "知识测试内容")
        .await
        .unwrap();

    let skill_uri = TianyanUri::new(ContextNamespace::Skill, vec!["skill1".to_string()]);
    vfs.create_file(&skill_uri).await.unwrap();
    vfs.write(&skill_uri, ContentLevel::Detail, "技能测试内容")
        .await
        .unwrap();

    let session_results = vfs
        .search_by_namespace(ContextNamespace::Session, "测试", 10)
        .await
        .unwrap();
    assert!(!session_results.is_empty());
    assert!(session_results
        .iter()
        .any(|r| r.uri.namespace() == ContextNamespace::Session));

    let memory_results = vfs
        .search_by_namespace(ContextNamespace::Memory, "测试", 10)
        .await
        .unwrap();
    assert!(!memory_results.is_empty());
    assert!(memory_results
        .iter()
        .any(|r| r.uri.namespace() == ContextNamespace::Memory));

    let knowledge_results = vfs
        .search_by_namespace(ContextNamespace::Knowledge, "测试", 10)
        .await
        .unwrap();
    assert!(!knowledge_results.is_empty());
    assert!(knowledge_results
        .iter()
        .any(|r| r.uri.namespace() == ContextNamespace::Knowledge));

    let skill_results = vfs
        .search_by_namespace(ContextNamespace::Skill, "测试", 10)
        .await
        .unwrap();
    assert!(!skill_results.is_empty());
    assert!(skill_results
        .iter()
        .any(|r| r.uri.namespace() == ContextNamespace::Skill));
}
```

Note: The tests `test_search_session`, `test_search_memory`, `test_search_knowledge`, `test_search_skill` (lines 224-292) are KEPT — they test `VfsSearch::search()`, which is the vector search path we are retaining.

- [ ] **Step 5: Verify compilation**

```powershell
cargo check --workspace
```

Expected: No errors. Ensure that `search_by_namespace` has no remaining references anywhere.

- [ ] **Step 6: Run tests**

```powershell
cargo test --workspace --lib -- --nocapture
```

Expected: All storage tests pass. `test_search_by_namespace` should no longer exist; the remaining namespace-scoped tests (`test_search_session`, etc.) should still pass (they assert `Err` since MockVectorStorage has no embedding service).

- [ ] **Step 7: Commit**

```bash
git add core/src/storage/traits.rs core/src/storage/vfs/mod.rs core/src/storage/vfs/tests.rs
git commit -m "refactor(storage): remove dead full-text search path, keep VfsSearch::search() as sole entry"
```

---

### Task 3: Final verification

**Files:** None (read-only)

- [ ] **Step 1: Run full lint pass**

```powershell
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
```

Expected: No new warnings or errors.

- [ ] **Step 2: Run all tests**

```powershell
cargo test --workspace --lib -- --nocapture
```

Expected: All tests pass; zero failures.

- [ ] **Step 3: Verify no remaining references**

```powershell
rg "search_by_namespace|search_session|search_memory|search_knowledge|search_skill|search_recursive" --include '*.rs' core/src/
```

Expected: Zero results (all dead code has been removed).

---

## Self-Review

1. **Spec coverage:**
   - [x] Fix Qdrant namespace filter field name — Task 1
   - [x] Delete `search_by_namespace` and convenience methods from traits.rs — Task 2 Step 1
   - [x] Delete `search_recursive` from vfs/mod.rs — Task 2 Step 2
   - [x] Delete `search_by_namespace` override from vfs/mod.rs — Task 2 Step 3
   - [x] Delete related tests — Task 2 Step 4
   - [x] Keep `VfsSearch::search()` as sole entry — implicitly verified by Task 3

2. **Placeholder scan:** No TBD, TODO, or vague instructions found.

3. **Type consistency:** All method names and file paths match the actual codebase. `VfsSearch::search()` signature verified against `traits.rs` lines 357-362.

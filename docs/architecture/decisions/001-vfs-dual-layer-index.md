# ADR-001: VFS 双层摘要索引 —— 基础机制

**日期**: 2026-06  
**状态**: ✅ 已采纳  
**影响范围**: 全局基础 — 所有其他设计必须妥协于此

---

## 背景

天演需要统一管理多种上下文（知识库、记忆、技能、规则、用户偏好）。传统方案使用 chunk-based RAG 或独立存储，导致检索碎片化、Token 浪费、系统复杂性高。

## 决策

采用 **三层内容 + 双向量 RRF 融合** 的统一存储与检索层（VFS）：

| 层级 | 名称 | Token | 向量 | 用途 |
|------|------|-------|------|------|
| L0 | Abstract | ~100 | `abstract_vector` | 向量搜索、快速过滤 |
| L1 | Overview | ~2K | `overview_vector` | 内容导航、重排序 |
| L2 | Detail | 无限制 | — | 完整内容，按需加载 |

检索流程：查询文本 → embed → LanceDB RRF 融合搜索 `abstract_vector` + `overview_vector` → `ContentLoadStrategy::from_score()` 按分数分层加载（>0.85→L2, >0.6→L1, 其他→L0）。

## 后果

### 正面
- 统一抽象：所有命名空间共享同一套检索机制
- Token 高效：渐进式披露，按需加载
- 无需 chunk：`SummaryEngine` 生成摘要替代传统 RAG 分块

### 对子系统的约束
- **知识库不做 chunk**：完整文档直接写入 L2，双层检索替代 chunk-based RAG。`Chunker` 已移除。
- **技能渐进式披露**：L0 Abstract 用于快速发现（`SkillManager::list_available_skills()`），L2 Detail 按需加载。
- **图像双通道**：VLM 生成文本描述 → 文本 embedding（L0/L1），同时 `embed_image()` → `visual_vector` 用于视觉相似度搜索。

## 禁止模式清单

以下是与 VFS 设计**冲突**的做法，需在设计评审中主动拦截：

| 禁止模式 | 具体表现 | 违反原因 |
|---------|---------|---------|
| **独立检索管道** | 为知识库单独建立向量索引 + 检索 API | VFS RRF 融合检索是唯一入口 |
| **独立存储后端** | 引入 Redis、独立文件存储绕过 VFS trait | 所有持久化通过 `VfsCore` / `ContentStore` trait，底层由 `SqliteBackend` + `LanceDbVectorStore` 实现 |
| **全量技能加载** | 启动时读取所有 `skill.md` 文件到内存 | 技能通过 L0 Abstract 发现，L2 Detail 按需加载 |
| **手动 chunk** | 对文档分块后独立索引 | `Chunker` 已删除，`SummaryEngine` 替代 |
| **绕过 SummaryEngine** | 手动生成摘要或向量 | `SummaryEngine` 是 L0/L1 的唯一生成入口 |
| **独立命名空间** | 为某个模块创建独立 URI scheme | 所有内容在 `tianyan://` 下统一管理 |
| **在 VFS 外做检索** | 模块内部实现独立的搜索/过滤逻辑 | `VfsSearch::search()` 是唯一检索入口 |
| **绕过 ContentLoadStrategy** | 硬编码加载策略（总是加载 L2） | `ContentLoadStrategy::from_score()` 是唯一加载决策 |

## 关键文件

- `core/src/vfs/backend/sqlite.rs` — `SqliteBackend` 内容存储
- `core/src/vfs/vector/lancedb.rs` — RRF 融合检索
- `core/src/context/retrieval/retriever.rs` — `DualLayerRetriever`
- `core/src/observability/sqlite_db.rs` — 共享 SQLite 连接
- `core/src/observability/usage_stats.rs` — 使用统计追踪

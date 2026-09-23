# ADR-001: VFS 双层摘要索引 —— 基础机制

**日期**: 2026-06  
**状态**: ✅ 已采纳（2026-09-22 修订：L1 语义重定义 —— 概览 → 目录）
**影响范围**: 全局基础 — 所有其他设计必须妥协于此

---

## 背景

天演需要统一管理多种上下文（知识库、记忆、技能、规则、用户偏好）。传统方案使用 chunk-based RAG 或独立存储，导致检索碎片化、Token 浪费、系统复杂性高。

## 决策

采用 **三层内容 + 双向量 RRF 融合** 的统一存储与检索层（VFS）：

| 层级 | 名称 | Token | 向量 | 用途 |
|------|------|-------|------|------|
| L0 | 简介（Abstract） | ~100 | `abstract_vector` | 向量搜索、快速过滤（**这是什么**） |
| L1 | 目录（Overview） | 按章节数（结构化） | `overview_vector` | 内容导航（**下面有什么**）：叶节点 = 章节目录（标题 + 摘要 + 行号 range）；目录节点 = **不生成**（消费时 `vfs_list` 现遍历） |
| L2 | 正文（Detail） | 无限制 | — | 完整内容，**按章节 range 按需取段**（不自动加载） |

检索流程：查询文本 → embed → LanceDB RRF 融合搜索 `abstract_vector` + `overview_vector` → 命中**叶节点**（目录节点无内容、无向量，天然不参与命中）→ `ContentLoadStrategy` 返回 **简介 + 目录**（>0.6）/ **仅简介**（≤0.6）→ 模型读目录完成章节定位 → 按需取段（L2 range 读）。**L2 不再自动加载**（短内容下 L1"直用"= 全文，行为等价于旧制）。

## 后果

### 正面
- 统一抽象：所有命名空间共享同一套检索机制
- Token 高效：渐进式披露，按需加载
- 无需 chunk：`SummaryEngine` 生成摘要替代传统 RAG 分块

### 对子系统的约束
- **知识库不做 chunk**：完整文档直接写入 L2，双层检索替代 chunk-based RAG。`Chunker` 已移除。
- **技能渐进式披露**：L0 简介用于快速发现（`SkillManager::list_available_skills()`），L1 目录用于章节/部件定位，L2 正文按需取段；**多文件技能 = VFS 目录节点**（成员目录消费时现遍历，零生成、零向量）。
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
| **章节作为存储/检索单位** | 把文档按章节切块存储、各自索引 | 章节是**读取视图**（目录条目 + 行号 range），L2 保持单一完整文本——分块存储即 chunk 回归（见 REJECTED #4 边界澄清） |

## 关键文件

- `core/src/vfs/backend/sqlite.rs` — `SqliteBackend` 内容存储
- `core/src/vfs/vector/lancedb/` — RRF 融合检索（`batch.rs` / `mod.rs` / `tests.rs`）
- `core/src/context/retrieval/retriever.rs` — `DualLayerRetriever`
- `core/src/vfs/backend/sqlite_db.rs` — 共享 SQLite 连接（自 `observability/` 下沉，ADR-007）
- `core/src/observability/usage_stats.rs` — 使用统计追踪

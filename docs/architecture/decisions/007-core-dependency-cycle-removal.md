# ADR-007: Core 依赖环消除与共享基础设施归属

**日期**: 2026-08
**状态**: ✅ 已采纳
**影响范围**: core 全模块（vfs / common / config / context / observability / executor / agent / scheduler）

---

## 背景

Wave 6 系统性重构前，core 内部存在 3 个模块依赖环与多处"设计声明与实际不符"的残留：

1. **vfs→observability 环**：`vfs/backend/sqlite.rs` 依赖 `observability/sqlite_db.rs` 的 `SqliteDb`（共享 SQLite 连接），造成存储层反向依赖可观测层。
2. **context↔observability 环**：`RetrievalTrace` 家族定义于 `context/retrieval/types.rs`（生产方），`observability/usage_stats.rs` 持久化检索轨迹时反向依赖 context。
3. **common↔config 环**：`common` 的 `init_logging` 依赖 `config::LoggingConfig`，而 config 依赖 common 的错误类型。
4. **token 估算口径分裂**：存在三套口径——`context/compression/estimator.rs` 的 `TokenEstimator`、`knowledge/ingestor` 本地 `count_tokens`（字节数/4）、`summary_task` 的字节长度判断（`ABSTRACT_TOKEN_LIMIT * 4`），同一概念多处实现且互相不一致。
5. **死参数**：`SummaryEngine::new` 携带 2 个从未使用的 embedding 参数（与 traits.rs 声称的 EmbeddingProvider 反转不符）；`Agent` 结构体携带 5 个"为未来协调器 API 预留"的从未被读取字段。

## 决策

### 1. 共享基础设施归属被依赖方/叶模块

跨模块共享的基础设施一律下沉到被依赖方（vfs）或叶模块（common），消费方保留单向 re-export 保留下游兼容：

- **`SqliteDb` → `vfs/backend/sqlite_db.rs`**：整体移动（内容不变），`vfs/backend/mod.rs` 注册 `pub mod sqlite_db;`；`observability/mod.rs` 删除模块声明与 re-export（不保留隐藏依赖）。`usage_stats.rs` 经 `crate::vfs::backend::sqlite_db::SqliteDb` 引入——observability→vfs 成为残留单向依赖（共享连接，非环）。
- **`RetrievalStep` / `RetrievalStepType` / `RetrievalTrace` → `common/types/retrieval_trace.rs`**：纯数据契约放 common（对生产方 context 与持久化方 observability 同为下游）；`context/retrieval/types.rs` 保留 `pub use` 兼容生产方路径；`RetrievalTraceBuilder`（构建器）保留在 `context/retrieval/trace.rs`。`trace_json` 序列化契约冻结（含 gui-vite `types.ts` 镜像）。
- **`LoggingConfig` → `common/logging.rs`**：与 `init_logging` 同驻（类型与行为不拆散）；`config/mod.rs` 改为 `pub use crate::common::logging::LoggingConfig;`，`tianyan::config::LoggingConfig` 路径仍可用；删除空壳 `config/logging.rs`。TOML 配置兼容性零变化。
- **`TokenEstimator` / `estimate_tokens` → `common/token_estimator.rs`**：成为全系统唯一估算入口（字符类启发式：中文 1.5 字符/token、英文 0.75 词/token）；`context::compression` 保留 re-export；ingestor 删除本地 `count_tokens`，summary_task 字节判断改为 `estimate_tokens`。

理由：共享基础设施放在被依赖方而非依赖方，消除反向依赖；不保留隐藏 re-export（observability 的 SqliteDb 除外——observability 直接引用新路径），避免旧路径残留掩盖真实依赖。

### 2. 死参数清理（公开 API 零变化）

- `SummaryEngine::new` 4 参数 → 2 参数（删除 `_embedding_service` / `_embedding_model_name`），vfs→model 依赖边收窄为 `ChatService`（摘要生成所需，非环）。
- `Agent::new` 12 参数 → 7 参数：删除 `config` / `model_service` / `vfs` / `skill_executor` / `skill_registry` 5 个无读取点字段（编译器验证），`AgentBuilder` 同步删除 `with_skill_registry`（唯一变死的 with_* 方法）；保留 7 字段（`default_model` / `context_pipeline` / `metrics` / `skill_learning_engine` / `agent_loop` / `session_manager` / `snapshot_manager`）。

### 3. SKIP：P0-5b 显式索引标记

**不引入"写入方显式标记已索引"机制。** 理由：

1. 摘要任务的字节/token 误判 bug（P0-5a）已修复，L0 摘要污染问题消除，无失败行为支撑新机制。
2. 显式索引标记 = 新功能面（VFS metadata 契约 + SummaryTask 扫描逻辑 + 前端/API 可能受影响），收益不构成当前架构缺陷。
3. 现有 SummaryTask 兜底扫描（5 分钟间隔）在数据量增长前是可接受的延迟；若未来成为瓶颈，再引入写入触发队列（届时作为独立 ADR 决策）。

### 4. SKIP：P2-4 VFS trait 进一步拆分

**不引入更细粒度的 VFS trait 拆分。** VFS 接口已由 `VfsCore` / `ContentStore` / `VfsSearch` 三个 trait 组织（`vfs/traits.rs`，各由 `VfsImpl` 实现，均有生产实现 + 测试 mock → 真实 seam），职责边界清晰；进一步拆分（如按命名空间或按存储层级细分）无失败行为支撑，只会增加 trait 对象组合与派生实现的复杂度。

## 后果

### 正面
- 3 个依赖环（vfs→observability、context↔observability、common↔config）全部消除；common 为叶模块（无任何 core 内部模块引用）
- 残留依赖全部单向：`observability→vfs`（SqliteDb）、`observability→common`（RetrievalTrace）、`vfs→model`（ChatService），均为非环
- token 估算唯一入口，模块间统计口径一致

### 负面 / 代价
- server 与 tauri 对 `SqliteDb` / `LoggingConfig` 的引用路径更新为 `tianyan::vfs::backend::sqlite_db::SqliteDb` / `tianyan::common::logging::LoggingConfig`（re-export 兼容，显式路径更清晰）
- `TaskResult` 丢失 `Clone` 派生（`TianyanError` 含非 Clone 的 `Io` 变体；无克隆消费点，零影响）

### 行为变化记录（无 API 契约破坏）
- `ingest` API 的 `tokens_processed` 数值口径：字节数/4 → 字符类启发式估算（纯中文文本数值略降；server 不消费该字段）
- `run_tests` / `verify_build` 返回 JSON 的 `errors`：匹配更全（大小写不敏感 `error:` / `error[`）+ 每行 trim + 单行 200 字符截断 + 严格 50 条上限（原两套实现合并，修复字节切片 panic 与 51 条 off-by-one）
- 澄清路径 parent_id 回复链语义统一为"挂接在用户回答之下"（此前两路径不一致；仅 JSONL 元数据，无 API 消费者）
- `TaskStatus` JSON 契约零变化（`TaskResult.error` 类型 `Option<String>` → `Option<TianyanError>` 不影响序列化）
- 会话截断常量单点化：`session/mod.rs` 定义 `MAX_SESSION_MESSAGES=100` / `KEEP_RECENT_MESSAGES=50`，数值零变化

### 边界条件（违反即重新评估）
- 若 observability 需要再次反向依赖 context（而非 common），应重新评估 RetrievalTrace 归属
- 若新增跨模块共享基础设施，必须评估依赖方向：归属被依赖方，而非在被依赖方复制

## 关键文件

- `core/src/vfs/backend/sqlite_db.rs` — SqliteDb（`vfs/backend/mod.rs:61` 注册，VFS 与 UsageStats 共享连接）
- `core/src/observability/usage_stats.rs` — 经 `vfs::backend::sqlite_db::SqliteDb` + `common::types::retrieval_trace::RetrievalTrace` 引入
- `core/src/common/types/retrieval_trace.rs` — RetrievalTrace 家族（`context/retrieval/types.rs` re-export）
- `core/src/common/logging.rs` — LoggingConfig（`config/mod.rs` re-export）
- `core/src/common/token_estimator.rs` — TokenEstimator / estimate_tokens（`context/compression/mod.rs` re-export）
- `core/src/vfs/summary/engine.rs` — `SummaryEngine::new(model_service, model_name)` 2 参数
- `core/src/agent/agent_core.rs` + `core/src/agent/builder.rs` — `Agent::new` 7 参数、`AgentBuilder` 无 `with_skill_registry`
- `core/src/scheduler/tasks/summary_task.rs` — 短内容判断用 `estimate_tokens`；`core/src/knowledge/ingestor/mod.rs` — 删除本地 `count_tokens`
- `core/src/executor/output_parse.rs` — 统一 `extract_build_errors` / `count_test_passed` / `extract_test_failures`

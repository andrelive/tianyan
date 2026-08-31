# ADR-025: 移除检索轨迹功能

日期：2026-08-31
状态：已采纳（对应否决记录：[REJECTED #20](REJECTED.md)）

## 背景

检索轨迹（RetrievalTrace）记录单次 `DualLayerRetriever::retrieve()` 的完整过程（意图分析 → L0/L1 搜索 → 内容加载 → 聚合），持久化到 `retrieval_traces` 表（FIFO 500 条），GUI 提供「检索轨迹」面板。设计意图是检索调试黑匣子：回溯"为什么这次检索成这样"。

## 问题

实际运行暴露了三重问题（2026-08-31 用户报告与代码核查）：

1. **覆盖面错位**：只记录上下文组装路径（`ContextPipeline::load_injectable` 内的规则检索 + 记忆检索各一条），**不记录** agent 主动调用的 `search_vfs` 工具（走 `vfs.search()`，不落轨迹）。最值得观测的检索路径反而不可见。
2. **展示误导**：规则检索与记忆检索使用同一 query 字符串，两条轨迹的 query、steps、total_tokens 完全相同（仅执行时间不同），面板上呈现为"重复记录"——用户无法区分，误以为 bug。
3. **零有效使用**：功能存在的整个生命周期内，用户从未通过它排查过任何检索问题；空结果时轨迹仅剩 intent_analysis 一步，信息量近零。

## 决策

完整移除：

- `core/src/common/types/retrieval_trace.rs`（类型契约）
- `core/src/context/retrieval/trace.rs`（RetrievalTraceBuilder）
- `record_retrieval_trace` / `query_recent_traces`（observability + db Repository）
- `retrieval_traces` 表创建（sqlite_db.rs；已存在的旧表保留为孤儿，无害）
- `GET /api/v1/retrieval/traces` 端点与 GUI 面板、侧边栏入口、类型与 mock

**保留**：retriever 的 `record_doc_hit` / `record_search_query` / `record_doc_load` 统计记录与 `Retrieval completed` 结构化日志（观测能力主体不受损）。

## 后果

- 减少 2 张 GUI 页面、1 个 API 端点、1 张表、6 个核心类型/构建器文件的维护面。
- 检索观测退回 UsageStats（搜索热度/文档命中）+ tracing 日志（耗时/结果数）。
- 若未来需要检索调试，按 [REJECTED #20](REJECTED.md) 的重建条件执行：一次覆盖两条检索路径并标注用途。
- `record_retrieval_trace` 写入时的 `db.try_lock()` 竞争点随之消失（热路径更干净）。

//! 执行记录日志（GEPA 数据层，ADR-017）。
//!
//! 工具执行轨迹的持久化存储与统计查询：
//! - 记录：ToolObservabilityListener 每次工具执行后写入（零 LLM）；
//! - 查询：供演化智能体经 execution_stats / execution_detail /
//!   delegation_stats 工具调用（让智能体反思自己的执行历史）。
//!
//! 本模块是派生统计数据：内容权威在 VFS，数据可重建；共享 SqliteDb 连接
//! （ADR-005 / AGENTS.md 派生索引模式）。

use std::sync::Arc;

use crate::common::error::TianyanError;
use crate::db::execution::{DelegationStat, ExecutionDetail, ExecutionRepo, ExecutionStat};
use crate::db::Database;
use crate::observability::execution_history::ExecutionHistory;

/// 任务描述截断上限（防参数膨胀入库）。
const MAX_TASK_DESCRIPTION: usize = 500;
/// 执行结果截断上限（防工具输出膨胀入库）。
const MAX_RESULT_CHARS: usize = 2000;

/// `executions` 表的保留窗口（天）。
///
/// 与 `trace_spans` 的 `SPAN_RETENTION_DAYS`（30 天）同属派生数据治理：本表可重建、
/// 内容权威在 VFS，无限保留只会单调膨胀——实测 2026-11-13：73,427 行 / 79 MB
/// （`result` 62.2 MB + `task_description` 17.7 MB），横跨 85 天。
///
/// 取 **30 天**（与 trace 一致）。**为什么不取更宽**：实测三档收益——90 天删 0 行
/// （现有数据只跨 85 天，等于不清理）、60 天删 9,850 行 / 10.9 MB、30 天删
/// 34,786 行 / **39.3 MB**。而消费侧（演化任务）只看近 7 天，`execution_stats`
/// 的窗口语义也以「最近」为主——留 30 天足够，留 85 天只是占空间。
const EXECUTION_RETENTION_DAYS: i64 = 30;

/// 清理节流间隔（毫秒）：`record` 是每次工具执行的热路径，清理不必每次都跑。
const PRUNE_THROTTLE_MS: i64 = 60 * 60 * 1000;

/// 按关键词分类执行（零 LLM 纯代码；GEPA 数据层分类）。
///
/// 与技能学习引擎的关键词兜底同一套类别；演化智能体按类别查询统计。
pub fn categorize_by_keyword(description: &str) -> &'static str {
    let lower = description.to_lowercase();

    if lower.contains("文件")
        || lower.contains("file")
        || lower.contains("目录")
        || lower.contains("folder")
    {
        "file_operation"
    } else if lower.contains("代码")
        || lower.contains("code")
        || lower.contains("编程")
        || lower.contains("programming")
    {
        "code_operation"
    } else if lower.contains("搜索")
        || lower.contains("search")
        || lower.contains("查找")
        || lower.contains("find")
        || lower.contains("grep")
        || lower.contains("glob")
    {
        "search_operation"
    } else if lower.contains("测试")
        || lower.contains("test")
        || lower.contains("验证")
        || lower.contains("verify")
    {
        "test_operation"
    } else if lower.contains("部署")
        || lower.contains("deploy")
        || lower.contains("发布")
        || lower.contains("release")
    {
        "deploy_operation"
    } else if lower.contains("分析")
        || lower.contains("analyze")
        || lower.contains("检查")
        || lower.contains("check")
    {
        "analysis_operation"
    } else {
        "general_operation"
    }
}

/// 按类别统计的执行数据。
/// 执行记录日志（GEPA 数据层）。
#[derive(Clone)]
pub struct ExecutionLog {
    /// 执行记录域仓储（SQL 收敛：写入/统计/明细经此）。
    repo: ExecutionRepo,
    /// 上次清理时间戳（毫秒；节流用）。`Arc` 让 `Clone` 出的副本共享同一个节流窗口
    /// （`ExecutionLog` 会按需 clone 到多个监听器）。
    last_prune_ms: Arc<std::sync::atomic::AtomicI64>,
}

impl ExecutionLog {
    /// 创建执行记录日志（共享 SqliteDb 连接）。
    ///
    /// # Errors
    /// * 返回 TianyanError（本方法当前不失败，签名保持与全库统一错误类型）。
    pub fn new(db: Arc<Database>) -> Result<Arc<Self>, TianyanError> {
        Ok(Arc::new(Self {
            repo: ExecutionRepo::new(db),
            last_prune_ms: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        }))
    }

    /// 记录一次工具执行（热路径：try_lock，锁不可用时跳过）。
    ///
    /// # Errors
    /// * SQLite 写入失败或锁不可用时返回 TianyanError::Custom（带 observability 前缀）。
    pub async fn record(
        &self,
        session_id: &str,
        history: &ExecutionHistory,
    ) -> Result<(), TianyanError> {
        let tool_name = tool_name_from(&history.task_description);
        let category = categorize_by_keyword(&history.task_description).to_string();
        let task_description = crate::common::truncate::truncate_utf8_boundary(
            &history.task_description,
            MAX_TASK_DESCRIPTION,
        );
        let result =
            crate::common::truncate::truncate_utf8_boundary(&history.result, MAX_RESULT_CHARS);
        let skills_used =
            serde_json::to_string(&history.skills_used).unwrap_or_else(|_| "[]".to_string());
        let steps_json = serde_json::to_string(&history.steps).unwrap_or_else(|_| "[]".to_string());
        let ts = chrono::Utc::now().timestamp();
        // 落盘经 ExecutionRepo（SQL 收敛于 db 层）
        self.repo
            .record(
                session_id,
                tool_name,
                &category,
                &task_description,
                history.success,
                history.execution_time_ms as i64,
                &skills_used,
                &steps_json,
                &result,
                ts,
            )
            .await?;
        // 落盘成功后按节流窗口清理超期记录（维护动作，失败仅告警）
        self.prune_if_due().await;
        Ok(())
    }

    /// 按节流窗口清理超出保留期的执行记录（`EXECUTION_RETENTION_DAYS`）。
    ///
    /// 失败仅告警：这是维护动作，不影响「记录已成功落盘」的语义。
    async fn prune_if_due(&self) {
        use std::sync::atomic::Ordering;
        let now = chrono::Utc::now().timestamp_millis();
        let last = self.last_prune_ms.load(Ordering::Relaxed);
        if now.saturating_sub(last) < PRUNE_THROTTLE_MS {
            return;
        }
        self.last_prune_ms.store(now, Ordering::Relaxed);
        match self.repo.prune(EXECUTION_RETENTION_DAYS).await {
            Ok(0) => {}
            Ok(removed) => tracing::info!(
                removed,
                days = EXECUTION_RETENTION_DAYS,
                "executions: 已清理超出保留窗口的执行记录"
            ),
            Err(e) => tracing::warn!(error = %e, "executions: 清理失败（下次重试）"),
        }
    }

    /// 按类别统计执行情况。
    ///
    /// since_ts 为 epoch 秒（只统计该时间之后的记录）；category 为类别过滤。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError::Custom（带 observability 前缀）。
    pub async fn stats(
        &self,
        since_ts: Option<i64>,
        category: Option<&str>,
    ) -> Result<Vec<ExecutionStat>, TianyanError> {
        self.repo.stats(since_ts, category).await
    }

    /// 查询原始执行记录（按时间倒序）。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError::Custom（带 observability 前缀）。
    pub async fn detail(
        &self,
        since_ts: Option<i64>,
        category: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ExecutionDetail>, TianyanError> {
        self.repo.detail(since_ts, category, limit).await
    }

    /// 按角色统计委托使用情况（解析 delegate_to_agent 记录）。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError::Custom（带 observability 前缀）。
    pub async fn delegation_stats(
        &self,
        since_ts: Option<i64>,
    ) -> Result<Vec<DelegationStat>, TianyanError> {
        self.repo.delegation_stats(since_ts).await
    }
}

/// 从任务描述（"工具名: 参数JSON"）提取工具名。
fn tool_name_from(task_description: &str) -> &str {
    task_description
        .split(": ")
        .next()
        .unwrap_or(task_description)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::execution::parse_delegated_role;
    use crate::observability::execution_history::{ExecutionHistory, ExecutionStep};

    async fn make_log() -> Arc<ExecutionLog> {
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        ExecutionLog::new(db).unwrap()
    }

    fn history(task_description: &str, success: bool) -> ExecutionHistory {
        ExecutionHistory {
            task_description: task_description.to_string(),
            steps: vec![ExecutionStep {
                description: "step".to_string(),
                action: "action".to_string(),
                parameters: Default::default(),
                result: "ok".to_string(),
            }],
            result: if success {
                "ok".to_string()
            } else {
                "error".to_string()
            },
            success,
            execution_time_ms: 100,
            skills_used: vec!["skill-1".to_string()],
        }
    }

    #[test]
    fn test_categorize_keywords() {
        assert_eq!(
            categorize_by_keyword("read_file: {\"path\": \"a.rs\"}"),
            "file_operation"
        );
        assert_eq!(categorize_by_keyword("grep: pattern"), "search_operation");
        assert_eq!(categorize_by_keyword("run_tests: {}"), "test_operation");
        assert_eq!(categorize_by_keyword("web_search: x"), "search_operation");
        assert_eq!(categorize_by_keyword("分析项目结构"), "analysis_operation");
        assert_eq!(categorize_by_keyword("unknown thing"), "general_operation");
    }

    #[test]
    fn test_tool_name_from() {
        assert_eq!(
            tool_name_from("read_file: {\"path\": \"a.rs\"}"),
            "read_file"
        );
        assert_eq!(
            tool_name_from("delegate_to_agent: {\"task\": \"x\"}"),
            "delegate_to_agent"
        );
        assert_eq!(tool_name_from("plain"), "plain");
    }

    #[test]
    fn test_parse_delegated_role() {
        assert_eq!(
            parse_delegated_role("delegate_to_agent: {\"role\": \"researcher\", \"task\": \"t\"}"),
            Some("researcher".to_string())
        );
        assert_eq!(parse_delegated_role("read_file: {}"), None);
        assert_eq!(parse_delegated_role("delegate_to_agent: not-json"), None);
    }

    #[test]
    fn test_truncate_utf8_boundary() {
        // 中文多字节：截断点落在字符中间时回退到边界
        let s = "中文内容测试";
        let t = crate::common::truncate::truncate_utf8_boundary(s, 5);
        assert!(t.len() <= 5 + 1);
        assert!(t.ends_with('…'));
        let t2 = crate::common::truncate::truncate_utf8_boundary("short", 10);
        assert_eq!(t2, "short");
    }

    #[tokio::test]
    async fn test_record_and_stats() {
        let log = make_log().await;
        log.record("s1", &history("read_file: {\"path\": \"a.rs\"}", true))
            .await
            .unwrap();
        log.record("s1", &history("run_tests: {}", false))
            .await
            .unwrap();
        log.record("s2", &history("run_tests: {}", true))
            .await
            .unwrap();

        let stats = log.stats(None, None).await.unwrap();
        assert_eq!(stats.len(), 2);
        let test_stat = stats
            .iter()
            .find(|s| s.category == "test_operation")
            .unwrap();
        assert_eq!(test_stat.total, 2);
        assert_eq!(test_stat.successes, 1);
        assert!((test_stat.success_rate - 0.5).abs() < 1e-9);

        // 类别过滤
        let file_stats = log.stats(None, Some("file_operation")).await.unwrap();
        assert_eq!(file_stats.len(), 1);
        assert_eq!(file_stats[0].total, 1);
        assert_eq!(file_stats[0].successes, 1);
    }

    /// `executions` 保留窗口（90 天）：超期行被删、窗口内保留；重复清理幂等返回 0。
    ///
    /// 这张表此前**没有任何保留策略**（实测 2026-11-13：73,427 行 / 79 MB，横跨
    /// 85 天），与 `trace_spans` 的 30 天窗口不一致。
    #[tokio::test]
    async fn test_prune_removes_only_rows_outside_retention_window() {
        let log = make_log().await;
        // make_log 的 record 用「现在」作 ts：造一条 100 天前的记录走底层 repo
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        let repo = ExecutionRepo::new(db.clone());
        let now = chrono::Utc::now().timestamp();
        let old = now - 100 * 86_400;
        repo.record(
            "s1",
            "read_file",
            "file_operation",
            "旧记录",
            true,
            10,
            "[]",
            "[]",
            "r",
            old,
        )
        .await
        .unwrap();
        repo.record(
            "s1",
            "read_file",
            "file_operation",
            "新记录",
            true,
            10,
            "[]",
            "[]",
            "r",
            now,
        )
        .await
        .unwrap();

        let removed = repo.prune(90).await.unwrap();
        assert_eq!(removed, 1, "只应删除 100 天前那条");
        assert_eq!(repo.prune(90).await.unwrap(), 0, "重复清理应幂等");
        // `log` 仅用于确认既有构造路径仍可用（节流清理走 record 后自动触发）
        drop(log);
    }

    #[tokio::test]
    async fn test_record_and_detail() {
        let log = make_log().await;
        log.record("s1", &history("grep: {\"pattern\": \"foo\"}", true))
            .await
            .unwrap();
        let detail = log.detail(None, None, 10).await.unwrap();
        assert_eq!(detail.len(), 1);
        assert_eq!(detail[0].tool_name, "grep");
        assert_eq!(detail[0].category, "search_operation");
        assert!(detail[0].success);
        assert_eq!(detail[0].skills_used, vec!["skill-1".to_string()]);
        assert_eq!(detail[0].session_id, "s1");

        // 类别过滤
        let empty = log.detail(None, Some("code_operation"), 10).await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_since_filter() {
        let log = make_log().await;
        log.record("s1", &history("read_file: {}", true))
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp();
        // since 未来时间 → 无结果
        let stats = log.stats(Some(now + 3600), None).await.unwrap();
        assert!(stats.is_empty());
        // since 过去时间 → 有结果
        let stats = log.stats(Some(now - 3600), None).await.unwrap();
        assert!(!stats.is_empty());
    }

    #[tokio::test]
    async fn test_delegation_stats() {
        let log = make_log().await;
        log.record(
            "s1",
            &history(
                "delegate_to_agent: {\"role\": \"researcher\", \"task\": \"r1\"}",
                true,
            ),
        )
        .await
        .unwrap();
        log.record(
            "s1",
            &history(
                "delegate_to_agent: {\"role\": \"researcher\", \"task\": \"r2\"}",
                false,
            ),
        )
        .await
        .unwrap();
        log.record(
            "s1",
            &history(
                "delegate_to_agent: {\"role\": \"editor\", \"task\": \"e1\"}",
                true,
            ),
        )
        .await
        .unwrap();
        log.record("s1", &history("read_file: {}", true))
            .await
            .unwrap();

        let stats = log.delegation_stats(None).await.unwrap();
        assert_eq!(stats.len(), 2);
        let researcher = stats.iter().find(|s| s.role == "researcher").unwrap();
        assert_eq!(researcher.total, 2);
        assert_eq!(researcher.successes, 1);
        let editor = stats.iter().find(|s| s.role == "editor").unwrap();
        assert_eq!(editor.total, 1);
        assert_eq!(editor.successes, 1);
    }
}

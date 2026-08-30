//! 执行记录日志（GEPA 数据层，ADR-017）。
//!
//! 工具执行轨迹的持久化存储与统计查询：
//! - 记录：ToolObservabilityListener 每次工具执行后写入（零 LLM）；
//! - 查询：供演化智能体经 execution_stats / execution_detail /
//!   delegation_stats 工具调用（让智能体反思自己的执行历史）。
//!
//! 本模块是派生统计数据：内容权威在 VFS，数据可重建；共享 SqliteDb 连接
//! （ADR-005 / AGENTS.md 派生索引模式）。

use std::collections::HashMap;
use std::sync::Arc;

use crate::common::error::TianyanError;
use crate::observability::execution_history::ExecutionHistory;
use crate::db::Database;

/// 任务描述截断上限（防参数膨胀入库）。
const MAX_TASK_DESCRIPTION: usize = 500;
/// 执行结果截断上限（防工具输出膨胀入库）。
const MAX_RESULT_CHARS: usize = 2000;

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
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExecutionStat {
    /// 类别（file_operation / code_operation / ...）。
    pub category: String,
    /// 执行总次数。
    pub total: u64,
    /// 成功次数。
    pub successes: u64,
    /// 成功率（0.0 ~ 1.0）。
    pub success_rate: f64,
    /// 平均耗时（毫秒）。
    pub avg_time_ms: f64,
    /// 最近一次执行时间（epoch 秒）。
    pub last_ts: i64,
}

/// 单条执行明细。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExecutionDetail {
    /// 记录 ID。
    pub id: i64,
    /// 来源会话。
    pub session_id: String,
    /// 工具名。
    pub tool_name: String,
    /// 类别。
    pub category: String,
    /// 任务描述（工具名 + 参数摘要）。
    pub task_description: String,
    /// 是否成功。
    pub success: bool,
    /// 耗时（毫秒）。
    pub execution_time_ms: u64,
    /// 使用的技能列表。
    pub skills_used: Vec<String>,
    /// 执行结果（截断）。
    pub result: String,
    /// 执行时间（epoch 秒）。
    pub ts: i64,
}

/// 角色委托统计。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DelegationStat {
    /// 角色名。
    pub role: String,
    /// 委托总次数。
    pub total: u64,
    /// 成功次数。
    pub successes: u64,
    /// 成功率（0.0 ~ 1.0）。
    pub success_rate: f64,
}

/// 执行记录日志（GEPA 数据层）。
#[derive(Clone)]
pub struct ExecutionLog {
    db: Arc<Database>,
}

impl ExecutionLog {
    /// 创建执行记录日志（共享 SqliteDb 连接）。
    ///
    /// # Errors
    /// * 返回 TianyanError（本方法当前不失败，签名保持与全库统一错误类型）。
    pub fn new(db: Arc<Database>) -> Result<Arc<Self>, TianyanError> {
        Ok(Arc::new(Self { db }))
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

        let conn = self.db.try_lock().map_err(|e| {
            TianyanError::Custom(format!("observability: execution_log: 数据库锁不可用：{e}"))
        })?;
        conn.execute(
            "INSERT INTO executions (session_id, tool_name, category, task_description, success, execution_time_ms, skills_used, steps_json, result, ts) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            rusqlite::params![
                session_id,
                tool_name,
                category,
                task_description,
                if history.success { 1 } else { 0 },
                history.execution_time_ms as i64,
                skills_used,
                steps_json,
                result,
                ts,
            ],
        )
        .map_err(|e| TianyanError::Custom(format!("observability: execution_log: 写入失败：{e}")))?;
        Ok(())
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
        let conn = self.db.lock().await;
        let (where_sql, params) = build_where(since_ts, category);
        let sql = format!(
            "SELECT category, COUNT(*) AS total,              SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END) AS successes,              AVG(execution_time_ms) AS avg_time_ms, MAX(ts) AS last_ts              FROM executions{where_sql} GROUP BY category ORDER BY total DESC"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| sqlite_query_error(&sql, e))?;
        let rows = stmt
            .query_map(
                rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                |row| {
                    let total = row.get::<_, i64>(1)? as u64;
                    let successes = row.get::<_, i64>(2)? as u64;
                    Ok(ExecutionStat {
                        category: row.get(0)?,
                        total,
                        successes,
                        success_rate: if total > 0 {
                            successes as f64 / total as f64
                        } else {
                            0.0
                        },
                        avg_time_ms: row.get(3)?,
                        last_ts: row.get(4)?,
                    })
                },
            )
            .map_err(|e| sqlite_query_error(&sql, e))?;
        collect_rows(rows, &sql)
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
        let conn = self.db.lock().await;
        let (where_sql, mut params) = build_where(since_ts, category);
        let sql = format!(
            "SELECT id, session_id, tool_name, category, task_description, success,              execution_time_ms, skills_used, result, ts FROM executions{where_sql}              ORDER BY ts DESC, id DESC LIMIT ?"
        );
        params.push(Box::new(limit as i64));
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| sqlite_query_error(&sql, e))?;
        let rows = stmt
            .query_map(
                rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                |row| {
                    let skills_used: String = row.get(7)?;
                    Ok(ExecutionDetail {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        tool_name: row.get(2)?,
                        category: row.get(3)?,
                        task_description: row.get(4)?,
                        success: row.get::<_, i64>(5)? != 0,
                        execution_time_ms: row.get::<_, i64>(6)? as u64,
                        skills_used: serde_json::from_str(&skills_used).unwrap_or_default(),
                        result: row.get(8)?,
                        ts: row.get(9)?,
                    })
                },
            )
            .map_err(|e| sqlite_query_error(&sql, e))?;
        collect_rows(rows, &sql)
    }

    /// 按角色统计委托使用情况（解析 delegate_to_agent 记录）。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError::Custom（带 observability 前缀）。
    pub async fn delegation_stats(
        &self,
        since_ts: Option<i64>,
    ) -> Result<Vec<DelegationStat>, TianyanError> {
        let conn = self.db.lock().await;
        let mut where_clauses = vec!["tool_name = 'delegate_to_agent'".to_string()];
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(ts) = since_ts {
            where_clauses.push("ts >= ?".to_string());
            params.push(Box::new(ts));
        }
        let where_sql = format!(" WHERE {}", where_clauses.join(" AND "));
        let sql = format!("SELECT task_description, success FROM executions{where_sql}");
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| sqlite_query_error(&sql, e))?;
        let rows = stmt
            .query_map(
                rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0)),
            )
            .map_err(|e| sqlite_query_error(&sql, e))?;

        let mut agg: HashMap<String, (u64, u64)> = HashMap::new();
        let mut failed_rows: Option<TianyanError> = None;
        for row in rows {
            match row {
                Ok((task_description, success)) => {
                    if let Some(role) = parse_delegated_role(&task_description) {
                        let entry = agg.entry(role).or_insert((0, 0));
                        entry.0 += 1;
                        if success {
                            entry.1 += 1;
                        }
                    }
                }
                Err(e) => {
                    failed_rows = Some(sqlite_query_error(&sql, e));
                    break;
                }
            }
        }
        if let Some(e) = failed_rows {
            return Err(e);
        }

        let mut stats: Vec<DelegationStat> = agg
            .into_iter()
            .map(|(role, (total, successes))| DelegationStat {
                role,
                total,
                successes,
                success_rate: if total > 0 {
                    successes as f64 / total as f64
                } else {
                    0.0
                },
            })
            .collect();
        stats.sort_by_key(|s| std::cmp::Reverse(s.total));
        Ok(stats)
    }
}

/// 从任务描述（"工具名: 参数JSON"）提取工具名。
fn tool_name_from(task_description: &str) -> &str {
    task_description
        .split(": ")
        .next()
        .unwrap_or(task_description)
}

/// 解析 delegate_to_agent 记录中的 role 参数。
fn parse_delegated_role(task_description: &str) -> Option<String> {
    let (_, json_part) = task_description.split_once(": ")?;
    serde_json::from_str::<serde_json::Value>(json_part)
        .ok()
        .and_then(|v| v.get("role").and_then(|r| r.as_str()).map(str::to_string))
}

/// 构造 WHERE 子句与参数（since_ts / category 可选）。
fn build_where(
    since_ts: Option<i64>,
    category: Option<&str>,
) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(ts) = since_ts {
        clauses.push("ts >= ?".to_string());
        params.push(Box::new(ts));
    }
    if let Some(cat) = category {
        clauses.push("category = ?".to_string());
        params.push(Box::new(cat.to_string()));
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    (where_sql, params)
}

/// 收集查询行（统一错误包装）。
fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
    sql: &str,
) -> Result<Vec<T>, TianyanError> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| sqlite_query_error(sql, e))?);
    }
    Ok(out)
}

/// SQLite 查询错误包装（带 SQL 摘要，便于排查）。
fn sqlite_query_error(sql: &str, e: rusqlite::Error) -> TianyanError {
    let sql_head = sql.split_whitespace().take(6).collect::<Vec<_>>().join(" ");
    TianyanError::Custom(format!(
        "observability: execution_log: 查询失败（{sql_head}...）：{e}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
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

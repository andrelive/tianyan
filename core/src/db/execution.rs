//! 执行记录域仓储（Repository 层）。
//!
//! **SQL 收敛**：执行记录（GEPA 数据层：executions 表）的写入/统计/明细/委托统计
//! 从 `observability::execution_log::ExecutionLog` 收口于此。

use std::collections::HashMap;
use std::sync::Arc;

use crate::common::error::TianyanError;
use crate::db::Database;

/// 执行统计（按类别 / 工具 / 会话维度聚合）。
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

/// 执行记录域仓储。
#[derive(Clone)]
pub struct ExecutionRepo {
    db: Arc<Database>,
}

impl ExecutionRepo {
    /// 创建仓储（共享 Database 单连接）。
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// 插入一条执行记录（组件已组装派生字段；SQL 收敛于本层）。
    ///
    /// 注：字段较多（11 参数）；后续可收敛为 `ExecutionRecord` 参数对象（记入债务）。
    #[allow(clippy::too_many_arguments)]
    pub async fn record(
        &self,
        session_id: &str,
        tool_name: &str,
        category: &str,
        task_description: &str,
        success: bool,
        execution_time_ms: i64,
        skills_used_json: &str,
        steps_json: &str,
        result: &str,
        ts: i64,
    ) -> Result<(), TianyanError> {
        // 写路径异步等待锁（此前 try_lock 失败直接上抛 → 记录丢失）。
        let conn = self.db.lock().await;
        conn.execute(
            "INSERT INTO executions (session_id, tool_name, category, task_description, success, execution_time_ms, skills_used, steps_json, result, ts) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            rusqlite::params![
                session_id,
                tool_name,
                category,
                task_description,
                if success { 1 } else { 0 },
                execution_time_ms,
                skills_used_json,
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

/// 解析 delegate_to_agent 记录中的 role 参数。
pub(crate) fn parse_delegated_role(task_description: &str) -> Option<String> {
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

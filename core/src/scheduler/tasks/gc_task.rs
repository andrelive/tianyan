//! Garbage Collection 任务。
//!
//! 定期扫描 agent/learned/ 规则和 Memory 中的过期记忆，
//! 标记或清理不再有效的内容。

use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{AgentPath, ContextNamespace, TianyanUri};
use crate::config::StorageConfig;
use crate::scheduler::{TaskContext, TaskHandler, TaskResult};

/// 规则过时的默认天数阈值。
const DEFAULT_RULE_STALE_DAYS: u32 = 30;
/// 记忆过时的默认天数阈值。
const DEFAULT_MEMORY_TTL_DAYS: u32 = 90;
/// 演化报告（运维日志类）的 TTL 天数。
const EVOLUTION_REPORT_TTL_DAYS: u32 = 30;

/// GC 任务：定期扫描并清理过期内容。
pub struct GcTask {
    /// 是否启用自动清理（来自 [`StorageConfig::auto_cleanup`]）。
    auto_cleanup: bool,
    /// 记忆条目 TTL 天数。
    memory_ttl_days: u32,
    /// 规则过时天数阈值。
    rule_stale_days: u32,
}

impl GcTask {
    /// 创建新的 GC 任务。
    ///
    /// `auto_cleanup` 取自存储配置（`[storage] auto_cleanup`）；
    /// TTL 阈值使用模块默认值（规则 30 天 / 记忆 90 天）。
    pub fn new(config: &StorageConfig) -> Self {
        Self {
            auto_cleanup: config.auto_cleanup,
            memory_ttl_days: DEFAULT_MEMORY_TTL_DAYS,
            rule_stale_days: DEFAULT_RULE_STALE_DAYS,
        }
    }

    /// 扫描 learned 规则，检查并清理过时规则。
    async fn scan_learned_rules(&self, ctx: &TaskContext) -> Result<usize> {
        let learned_uri = AgentPath::Learned.uri();
        let entries = match ctx.vfs.list(&learned_uri).await {
            Ok(e) => e,
            Err(_) => return Ok(0),
        };

        let mut stale_count = 0;
        let stale_threshold = Utc::now() - Duration::from_secs(self.rule_stale_days as u64 * 86400);

        for entry in &entries {
            let metadata_updated = entry.metadata.updated_at;
            if metadata_updated >= stale_threshold {
                continue;
            }

            stale_count += 1;
            let days_since = (Utc::now() - metadata_updated).num_days();
            tracing::warn!(
                rule_uri = %entry.metadata.uri,
                days_since_update = days_since,
                "规则过时"
            );

            // 如果启用自动清理，将过时规则归档
            if self.auto_cleanup {
                let rule_name = entry
                    .metadata
                    .uri
                    .path()
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());

                let archive_uri = AgentPath::learned_archive().append(&rule_name);

                match ctx.vfs.move_entry(&entry.metadata.uri, &archive_uri).await {
                    Ok(_) => {
                        tracing::info!(
                            rule_uri = %entry.metadata.uri,
                            days_since_update = days_since,
                            "已归档过时规则"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            rule_uri = %entry.metadata.uri,
                            error = %e,
                            "归档过时规则失败"
                        );
                    }
                }
            }
        }

        if stale_count > 0 {
            tracing::info!(
                total_rules = entries.len(),
                stale_count = stale_count,
                auto_cleanup = self.auto_cleanup,
                "GC: 发现过时规则"
            );
        } else if !entries.is_empty() {
            tracing::debug!(total_rules = entries.len(), "GC: 所有规则均未过时");
        }

        Ok(stale_count)
    }

    /// 扫描 Memory 命名空间，清理过期记忆条目。
    ///
    /// 递归遍历全树（历史缺陷：此前只 `list` 一层，嵌套在 `cases/`、`facts/`
    /// 等子目录中的条目从未被扫描到，"90 天 TTL"实际从未生效）；
    /// TTL 按路径白名单分层（[`Self::ttl_days_for`]）——未列入的域默认豁免。
    async fn scan_memory(&self, ctx: &TaskContext) -> Result<usize> {
        if !self.auto_cleanup {
            return Ok(0);
        }

        let memory_uri = TianyanUri::new(ContextNamespace::Memory, vec![]);
        let mut cleaned = 0;
        self.clean_expired_in_dir(ctx, &memory_uri, &mut cleaned)
            .await;

        if cleaned > 0 {
            tracing::info!(cleaned = cleaned, "GC: 已清理过期记忆条目");
        }

        Ok(cleaned)
    }

    /// 记忆 TTL 分层策略：按路径分段确定 TTL 天数（`None` = 豁免，不自动清理）。
    ///
    /// 白名单化设计（防误删）：只有明确列出的域受 TTL 治理——
    /// - `cases/`（情景案例）与 `clipboard/`（剪贴板沉淀）：默认记忆 TTL（90 天）；
    /// - `events/evolution_reports/`（运维日志）：30 天；
    /// - 其余（`facts/`、`events/decisions/`、`events/task_states/` 等语义记忆与
    ///   系统状态）：豁免——删除权归演化治理或任务自身。
    fn ttl_days_for(&self, uri: &TianyanUri) -> Option<u32> {
        let path: Vec<&str> = uri.path().iter().map(|s| s.as_str()).collect();
        match path.first().copied() {
            Some("cases") | Some("clipboard") => Some(self.memory_ttl_days),
            Some("events") => match path.get(1).copied() {
                Some("evolution_reports") => Some(EVOLUTION_REPORT_TTL_DAYS),
                _ => None,
            },
            _ => None,
        }
    }

    /// 递归清理指定目录下按 TTL 过期的叶子条目（目录下钻，不删目录）。
    async fn clean_expired_in_dir(&self, ctx: &TaskContext, dir: &TianyanUri, cleaned: &mut usize) {
        let entries = match ctx.vfs.list(dir).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(uri = %dir, error = %e, "GC: 列出记忆目录失败");
                return;
            }
        };

        for entry in entries {
            if entry.is_directory() {
                Box::pin(self.clean_expired_in_dir(ctx, entry.uri(), cleaned)).await;
                continue;
            }

            let Some(ttl_days) = self.ttl_days_for(entry.uri()) else {
                continue;
            };
            let threshold = Utc::now() - Duration::from_secs(ttl_days as u64 * 86400);
            if entry.metadata.updated_at < threshold {
                match ctx.vfs.delete(entry.uri()).await {
                    Ok(_) => {
                        *cleaned += 1;
                        tracing::debug!(
                            memory_uri = %entry.uri(),
                            "已清理过期记忆"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            memory_uri = %entry.uri(),
                            error = %e,
                            "清理过期记忆失败"
                        );
                    }
                }
            }
        }
    }
}

impl Default for GcTask {
    fn default() -> Self {
        Self::new(&StorageConfig::default())
    }
}

#[async_trait]
impl TaskHandler for GcTask {
    async fn execute(&self, ctx: &TaskContext) -> TaskResult {
        tracing::info!("开始执行 GC 扫描...");

        let rules_result = self.scan_learned_rules(ctx).await;
        let memory_result = self.scan_memory(ctx).await;

        match (rules_result, memory_result) {
            (Ok(rules), Ok(mem)) => {
                tracing::info!(stale_rules = rules, cleaned_memory = mem, "GC 扫描完成");
                TaskResult::success(rules + mem)
            }
            (Err(e), _) | (_, Err(e)) => {
                tracing::warn!(error = %e, "GC 扫描失败");
                TaskResult::failed(TianyanError::Custom(format!("GC 任务：扫描失败：{}", e)))
            }
        }
    }

    fn name(&self) -> &str {
        "garbage_collection"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, Utc};
    use std::sync::Arc;

    use crate::common::types::ContextNamespace;
    use crate::memory::{ExtractionConfig, MemoryExtractor};
    use crate::model::ChatService;
    use crate::test_utils::{MockChatService, MockVfs};
    use crate::vfs::{SummaryEngine, VfsCore};

    /// 构造 GC 测试上下文（MockVfs + mock 服务，GC 仅使用 vfs）。
    fn make_context(vfs: Arc<MockVfs>) -> TaskContext {
        let chat: Arc<dyn ChatService> = Arc::new(MockChatService::new());
        let summary_engine = Arc::new(SummaryEngine::new(chat.clone(), "test-model"));
        let memory_extractor = Arc::new(MemoryExtractor::new(
            chat.clone(),
            ExtractionConfig::default(),
        ));
        let skill_reviewer = Arc::new(crate::skills::SkillReviewer::new(
            chat,
            vfs.clone(),
            "test-model".to_string(),
        ));
        let config = Arc::new(crate::config::TianyanConfig::default());
        TaskContext::new(
            vfs,
            summary_engine,
            memory_extractor,
            skill_reviewer,
            config,
        )
    }

    fn learned_uri(name: &str) -> TianyanUri {
        AgentPath::Learned.uri().append(name)
    }

    fn mem_path(segments: &[&str]) -> TianyanUri {
        TianyanUri::new(
            ContextNamespace::Memory,
            segments.iter().map(|s| s.to_string()).collect(),
        )
    }

    // ── scan_learned_rules ───────────────────────────────────────────

    #[tokio::test]
    async fn test_scan_learned_rules_archives_stale_rule() {
        let vfs = Arc::new(MockVfs::new());
        let learned_dir = AgentPath::Learned.uri();
        vfs.add_entry_with_updated_at(
            &learned_dir,
            &learned_uri("stale-rule"),
            Utc::now() - ChronoDuration::days(40), // 超过 30 天阈值
        );
        vfs.add_entry_with_updated_at(
            &learned_dir,
            &learned_uri("fresh-rule"),
            Utc::now() - ChronoDuration::days(1),
        );

        let ctx = make_context(vfs.clone());
        let stale = GcTask::default().scan_learned_rules(&ctx).await.unwrap();

        assert_eq!(stale, 1, "仅过时规则应计数");
        let moves = vfs.move_calls();
        assert_eq!(moves.len(), 1, "仅过时规则应被归档");
        assert_eq!(
            moves[0],
            (
                learned_uri("stale-rule").to_string(),
                AgentPath::learned_archive()
                    .append("stale-rule")
                    .to_string()
            )
        );
    }

    #[tokio::test]
    async fn test_scan_learned_rules_keeps_fresh_rule() {
        let vfs = Arc::new(MockVfs::new());
        vfs.add_entry_with_updated_at(
            &AgentPath::Learned.uri(),
            &learned_uri("fresh-rule"),
            Utc::now() - ChronoDuration::days(1),
        );

        let ctx = make_context(vfs.clone());
        let stale = GcTask::default().scan_learned_rules(&ctx).await.unwrap();

        assert_eq!(stale, 0, "新规则不应计数");
        assert!(vfs.move_calls().is_empty(), "新规则不应被归档");
    }

    // ── scan_memory ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_scan_memory_deletes_expired_cases_recursively() {
        // 递归覆盖：三层嵌套目录下的过期案例必须被清理；
        // facts 域不在白名单——超过 TTL 也豁免（语义记忆删除权归演化治理）
        let vfs = Arc::new(MockVfs::new());
        let root = mem_path(&[]);
        vfs.add_directory(&root, &mem_path(&["cases"]));
        vfs.add_directory(&mem_path(&["cases"]), &mem_path(&["cases", "failed_tasks"]));
        vfs.add_entry_with_updated_at(
            &mem_path(&["cases", "failed_tasks"]),
            &mem_path(&["cases", "failed_tasks", "old-case"]),
            Utc::now() - ChronoDuration::days(120), // 超过 90 天 TTL
        );
        vfs.add_entry_with_updated_at(
            &mem_path(&["cases", "failed_tasks"]),
            &mem_path(&["cases", "failed_tasks", "fresh-case"]),
            Utc::now() - ChronoDuration::days(1),
        );
        vfs.add_directory(&root, &mem_path(&["facts"]));
        vfs.add_entry_with_updated_at(
            &mem_path(&["facts"]),
            &mem_path(&["facts", "old-fact"]),
            Utc::now() - ChronoDuration::days(200), // 豁免：不被 TTL 清理
        );

        let ctx = make_context(vfs.clone());
        let cleaned = GcTask::default().scan_memory(&ctx).await.unwrap();

        assert_eq!(cleaned, 1, "仅过期案例应被清理");
        let remaining = vfs
            .list(&mem_path(&["cases", "failed_tasks"]))
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1, "过期案例应从 VFS 移除");
        assert_eq!(
            remaining[0].uri().path().last().map(|s| s.as_str()),
            Some("fresh-case"),
            "新案例应保留"
        );
        assert_eq!(
            vfs.list(&mem_path(&["facts"])).await.unwrap().len(),
            1,
            "facts 域豁免 TTL，不应被清理"
        );
    }

    #[tokio::test]
    async fn test_scan_memory_keeps_fresh_memory() {
        let vfs = Arc::new(MockVfs::new());
        let root = mem_path(&[]);
        vfs.add_directory(&root, &mem_path(&["cases"]));
        vfs.add_entry_with_updated_at(
            &mem_path(&["cases"]),
            &mem_path(&["cases", "recent-case"]),
            Utc::now() - ChronoDuration::days(1),
        );

        let ctx = make_context(vfs.clone());
        let cleaned = GcTask::default().scan_memory(&ctx).await.unwrap();

        assert_eq!(cleaned, 0, "新记忆不应被清理");
        assert_eq!(vfs.list(&mem_path(&["cases"])).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_scan_memory_skips_when_auto_cleanup_disabled() {
        let vfs = Arc::new(MockVfs::new());
        let root = mem_path(&[]);
        vfs.add_directory(&root, &mem_path(&["cases"]));
        vfs.add_entry_with_updated_at(
            &mem_path(&["cases"]),
            &mem_path(&["cases", "expired-case"]),
            Utc::now() - ChronoDuration::days(120),
        );

        // 同模块可直接构造私有字段（模拟 auto_cleanup=false 配置）
        let task = GcTask {
            auto_cleanup: false,
            memory_ttl_days: DEFAULT_MEMORY_TTL_DAYS,
            rule_stale_days: DEFAULT_RULE_STALE_DAYS,
        };
        let ctx = make_context(vfs.clone());
        let cleaned = task.scan_memory(&ctx).await.unwrap();

        assert_eq!(cleaned, 0, "关闭自动清理时不应删除任何记忆");
        assert_eq!(
            vfs.list(&mem_path(&["cases"])).await.unwrap().len(),
            1,
            "过期记忆应保留"
        );
    }

    #[tokio::test]
    async fn test_scan_memory_evolution_reports_ttl_30_days() {
        // 演化报告（运维日志）按 30 天 TTL 清理，比记忆默认 TTL 更短
        let vfs = Arc::new(MockVfs::new());
        let root = mem_path(&[]);
        vfs.add_directory(&root, &mem_path(&["events"]));
        vfs.add_directory(
            &mem_path(&["events"]),
            &mem_path(&["events", "evolution_reports"]),
        );
        vfs.add_entry_with_updated_at(
            &mem_path(&["events", "evolution_reports"]),
            &mem_path(&["events", "evolution_reports", "old-report.md"]),
            Utc::now() - ChronoDuration::days(40), // 超过 30 天
        );
        vfs.add_entry_with_updated_at(
            &mem_path(&["events", "evolution_reports"]),
            &mem_path(&["events", "evolution_reports", "recent-report.md"]),
            Utc::now() - ChronoDuration::days(3),
        );

        let ctx = make_context(vfs.clone());
        let cleaned = GcTask::default().scan_memory(&ctx).await.unwrap();

        assert_eq!(cleaned, 1, "仅超过 30 天的演化报告应被清理");
        assert_eq!(
            vfs.list(&mem_path(&["events", "evolution_reports"]))
                .await
                .unwrap()
                .len(),
            1,
            "近期报告应保留"
        );
    }

    #[tokio::test]
    async fn test_scan_memory_decisions_and_task_states_exempt() {
        // decisions（语义记忆）与 task_states（系统状态）均豁免 TTL：
        // 删除权归演化治理或任务自身，GC 不得自动清理
        let vfs = Arc::new(MockVfs::new());
        let root = mem_path(&[]);
        vfs.add_directory(&root, &mem_path(&["events"]));
        vfs.add_directory(&mem_path(&["events"]), &mem_path(&["events", "decisions"]));
        vfs.add_directory(
            &mem_path(&["events"]),
            &mem_path(&["events", "task_states"]),
        );
        vfs.add_entry_with_updated_at(
            &mem_path(&["events", "decisions"]),
            &mem_path(&["events", "decisions", "old-decision"]),
            Utc::now() - ChronoDuration::days(300),
        );
        vfs.add_entry_with_updated_at(
            &mem_path(&["events", "task_states"]),
            &mem_path(&["events", "task_states", "evolution.md"]),
            Utc::now() - ChronoDuration::days(300),
        );

        let ctx = make_context(vfs.clone());
        let cleaned = GcTask::default().scan_memory(&ctx).await.unwrap();

        assert_eq!(cleaned, 0, "decisions 与 task_states 域豁免，不得自动清理");
    }
}

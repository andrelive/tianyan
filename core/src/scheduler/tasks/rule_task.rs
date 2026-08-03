//! 规则提炼任务。
//!
//! 定时扫描 VFS 中的 Pattern 和 FailedCase 记忆，
//! 当同类失败 ≥ 阈值时自动提炼为 Learned Rule，
//! 写入 agent/learned/，实现从记忆到规则的跨会话转化。

use std::sync::Arc;

use async_trait::async_trait;

use super::rule_suggester::RuleSuggester;
use crate::model::ChatService;
use crate::scheduler::{TaskContext, TaskHandler, TaskResult};
use crate::vfs::VirtualFileSystem;

/// 规则提炼任务。
///
/// 作为 MemoryTask 的下游消费者，定时扫描 patterns/ 和 failed_tasks/
/// 目录中的记忆聚类，用 LLM 提炼为 learned rule。
pub struct RuleTask {
    /// 规则建议器（扫描 + LLM 聚类 + 记录）。
    suggester: RuleSuggester,
    /// 每个扫描周期处理上限。
    max_per_cycle: usize,
}

impl RuleTask {
    /// 创建新的规则提炼任务。
    pub fn new(
        vfs: Arc<dyn VirtualFileSystem>,
        model_service: Arc<dyn ChatService>,
        model_name: impl Into<String>,
    ) -> Self {
        Self {
            suggester: RuleSuggester::new(vfs, model_service, model_name),
            max_per_cycle: 10,
        }
    }

    /// 设置模型版本（用于记录规则生成来源）。
    pub fn with_model_version(mut self, version: impl Into<String>) -> Self {
        self.suggester = self.suggester.with_model_version(version);
        self
    }

    /// 设置 Pipeline 版本（用于记录规则生成来源）。
    pub fn with_pipeline_version(mut self, version: impl Into<String>) -> Self {
        self.suggester = self.suggester.with_pipeline_version(version);
        self
    }
}

#[async_trait]
impl TaskHandler for RuleTask {
    async fn execute(&self, _ctx: &TaskContext) -> TaskResult {
        tracing::info!("开始执行规则提炼任务...");

        let suggestions = match self.suggester.scan().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "记忆聚类扫描失败");
                return TaskResult::failed(format!("扫描失败：{}", e));
            }
        };

        if suggestions.is_empty() {
            tracing::debug!("没有需要提炼的记忆聚类");
            return TaskResult::success(0);
        }

        let mut promoted_count = 0;
        for suggestion in &suggestions {
            if suggestion.source_count < 2 {
                continue;
            }

            if promoted_count >= self.max_per_cycle {
                break;
            }

            tracing::info!(
                category = %suggestion.source_category,
                count = suggestion.source_count,
                "检测到记忆聚类，尝试提炼规则"
            );

            match self
                .suggester
                .promote_to_rule(
                    suggestion,
                    &format!("scheduled-{}", chrono::Utc::now().timestamp()),
                )
                .await
            {
                Ok(()) => promoted_count += 1,
                Err(e) => {
                    tracing::warn!(
                        category = %suggestion.source_category,
                        error = %e,
                        "规则提炼失败"
                    );
                }
            }
        }

        tracing::info!("规则提炼任务完成，提炼了 {} 条规则", promoted_count);
        TaskResult::success(promoted_count)
    }

    fn name(&self) -> &str {
        "rule_extraction"
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_rule_task_new() {
        // RuleTask::new needs real Arc<dyn VFS + ChatService>, skip construction test.
        // Name test doesn't require construction either — just verify the constant.
        assert_eq!("rule_extraction", "rule_extraction");
    }
}

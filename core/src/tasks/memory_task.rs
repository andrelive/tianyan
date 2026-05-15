//! 记忆提取任务。
//!
//! 本模块提供扫描会话并提取记忆的后台任务。

use std::collections::HashSet;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::common::types::{ContextNamespace, TianyanUri};
use crate::scheduler::{TaskContext, TaskHandler, TaskResult};

/// 记忆提取任务。
///
/// 扫描新会话并从对话中提取记忆。
pub struct MemoryTask {
    /// 已处理的会话 URI 集合。
    processed_sessions: RwLock<HashSet<String>>,
}

impl MemoryTask {
    /// 创建新的记忆任务。
    pub fn new() -> Self {
        Self {
            processed_sessions: RwLock::new(HashSet::new()),
        }
    }

    /// 扫描新会话。
    async fn scan_new_sessions(
        &self,
        ctx: &TaskContext,
    ) -> crate::common::error::Result<Vec<TianyanUri>> {
        let mut new_sessions = Vec::new();

        // 扫描 Session 类别下的所有会话
        let root_uri = TianyanUri::new(ContextNamespace::Session, vec![]);
        self.scan_sessions_recursive(ctx, &root_uri, &mut new_sessions)
            .await?;

        Ok(new_sessions)
    }

    /// 递归扫描会话。
    async fn scan_sessions_recursive(
        &self,
        ctx: &TaskContext,
        uri: &TianyanUri,
        new_sessions: &mut Vec<TianyanUri>,
    ) -> crate::common::error::Result<()> {
        let entries = ctx.vfs.list(uri).await?;

        for entry in entries {
            if entry.is_directory() {
                // 递归扫描子目。
                Box::pin(self.scan_sessions_recursive(ctx, entry.uri(), new_sessions)).await?;
            } else {
                // 检查是否是会话文件且未处理。
                let uri_str = entry.metadata.uri.to_string();
                let is_processed = {
                    let processed = self.processed_sessions.read().await;
                    processed.contains(&uri_str)
                };

                if !is_processed {
                    new_sessions.push(entry.metadata.uri.clone());
                }
            }
        }

        Ok(())
    }

    /// 从会话中提取记忆。
    async fn extract_from_session(
        &self,
        ctx: &TaskContext,
        session_uri: &TianyanUri,
    ) -> crate::common::error::Result<usize> {
        match ctx.memory_extractor.extract_from_session(session_uri).await {
            Ok(memories) => {
                let count = memories.len();
                tracing::info!("从会话提取 {} 条记忆：{}", count, session_uri);
                Ok(count)
            }
            Err(e) => {
                tracing::warn!("从会话提取记忆失败：{} - {}", session_uri, e);
                Err(e)
            }
        }
    }

    /// 标记会话为已处理。
    async fn mark_processed(&self, uri: &TianyanUri) {
        let mut processed = self.processed_sessions.write().await;
        processed.insert(uri.to_string());
    }
}

impl Default for MemoryTask {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TaskHandler for MemoryTask {
    async fn execute(&self, ctx: &TaskContext) -> TaskResult {
        tracing::info!("开始执行记忆提取任务...");

        // 1. 扫描新会。
        let sessions = match self.scan_new_sessions(ctx).await {
            Ok(uris) => uris,
            Err(e) => {
                return TaskResult::failed(format!("扫描会话失败：{}", e));
            }
        };

        if sessions.is_empty() {
            tracing::debug!("没有新会话需要处理");
            return TaskResult::success(0);
        }

        tracing::info!("发现 {} 个新会话", sessions.len());

        // 2. 处理每个会话
        let mut total_memories = 0;
        let mut processed_count = 0;

        for session_uri in sessions {
            match self.extract_from_session(ctx, &session_uri).await {
                Ok(count) => {
                    total_memories += count;
                    processed_count += 1;
                    self.mark_processed(&session_uri).await;
                }
                Err(_) => {
                    // 错误已在 extract_from_session 中记。
                    continue;
                }
            }
        }

        tracing::info!(
            "记忆提取任务完成，处理了 {} 个会话，提取 {} 条记忆",
            processed_count,
            total_memories
        );

        TaskResult::success(total_memories)
    }

    fn name(&self) -> &str {
        "memory_extraction"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_task_new() {
        let task = MemoryTask::new();
        assert_eq!(task.name(), "memory_extraction");
    }

    #[test]
    fn test_memory_task_default() {
        let task: MemoryTask = Default::default();
        assert_eq!(task.name(), "memory_extraction");
    }
}

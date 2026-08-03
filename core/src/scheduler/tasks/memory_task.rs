//! 记忆提取任务。
//!
//! 扫描 VFS 中的会话，对含有新消息的会话调用 MemoryExtractor 提取长期记忆。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::common::error::Result;
use crate::common::types::{ContentLevel, ContextNamespace, MemoryEntry, TianyanUri};
use crate::memory::format_memory_as_markdown;
use crate::scheduler::{TaskContext, TaskHandler, TaskResult};

/// 会话的记忆提取状态，存储在会话目录下的 `_metadata` 文件中。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionExtractionState {
    /// 上次提取时处理到的消息行数。
    last_extracted_message_count: usize,
    /// 上次提取时间（ISO 8601）。
    last_extracted_at: String,
}

impl SessionExtractionState {
    fn new(message_count: usize) -> Self {
        Self {
            last_extracted_message_count: message_count,
            last_extracted_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// 记忆提取任务。
///
/// 定时扫描 Session 命名空间下的所有会话，对比 VFS 中持久化的
/// `_metadata` 文件判断是否有新消息需要提取。
pub struct MemoryTask {
    /// 每个扫描周期处理上限，避免单次扫描耗时过长。
    max_process_per_cycle: usize,
}

impl MemoryTask {
    /// 创建新的记忆任务。
    pub fn new() -> Self {
        Self {
            max_process_per_cycle: 50,
        }
    }

    /// 从 `_metadata` 子文件中读取上次提取状态。
    async fn read_state(
        &self,
        ctx: &TaskContext,
        session_uri: &TianyanUri,
    ) -> Option<SessionExtractionState> {
        let state_uri = session_uri.append("_metadata");
        match ctx.vfs.read_content(&state_uri, ContentLevel::Detail).await {
            Ok(json) => match serde_json::from_str::<SessionExtractionState>(&json) {
                Ok(state) => Some(state),
                Err(e) => {
                    tracing::warn!(
                        session_uri = %session_uri,
                        error = %e,
                        "解析 _metadata 文件失败，将全量提取"
                    );
                    None
                }
            },
            Err(_) => None,
        }
    }

    /// 写入 `_metadata` 状态文件。
    async fn write_state(
        &self,
        ctx: &TaskContext,
        session_uri: &TianyanUri,
        state: &SessionExtractionState,
    ) -> Result<()> {
        let state_uri = session_uri.append("_metadata");
        let json = serde_json::to_string(state).map_err(|e| {
            crate::common::error::TianyanError::Custom(format!("序列化错误：{}", e))
        })?;
        if !ctx.vfs.exists(&state_uri).await? {
            ctx.vfs.create_file(&state_uri).await?;
        }
        ctx.vfs.write_content(&state_uri, &json).await?;
        Ok(())
    }

    /// 统计 session JSONL 文件中的消息行数。
    async fn count_session_messages(
        &self,
        ctx: &TaskContext,
        session_uri: &TianyanUri,
    ) -> Result<usize> {
        let content = ctx
            .vfs
            .read_content(session_uri, ContentLevel::Detail)
            .await?;
        Ok(content.lines().filter(|l| !l.trim().is_empty()).count())
    }

    /// 扫描需要提取记忆的会话。
    async fn scan_sessions(&self, ctx: &TaskContext) -> Result<Vec<TianyanUri>> {
        let mut candidates = Vec::new();
        let root_uri = TianyanUri::new(ContextNamespace::Session, vec![]);

        let entries = match ctx.vfs.list(&root_uri).await {
            Ok(e) => e,
            Err(_) => return Ok(Vec::new()),
        };

        for entry in entries {
            if !entry.is_directory() {
                continue;
            }
            let session_uri = entry.uri().clone();

            let current_count = match self.count_session_messages(ctx, &session_uri).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(session_uri = %session_uri, error = %e, "统计消息数失败");
                    continue;
                }
            };

            if current_count == 0 {
                continue;
            }

            let needs_extraction = match self.read_state(ctx, &session_uri).await {
                Some(state) => current_count > state.last_extracted_message_count,
                None => true,
            };

            if needs_extraction {
                candidates.push(session_uri);
                if candidates.len() >= self.max_process_per_cycle {
                    break;
                }
            }
        }

        Ok(candidates)
    }

    /// 存储单条记忆到 VFS。
    async fn store_memory(&self, ctx: &TaskContext, memory: &MemoryEntry) -> Result<()> {
        let uri = &memory.uri;

        if let Some(parent) = uri.parent() {
            if !ctx.vfs.exists(&parent).await? {
                ctx.vfs.create_directory(&parent).await?;
            }
        }

        let content = format_memory_as_markdown(memory);
        ctx.vfs.write_content(uri, &content).await?;

        let abstract_content = format!(
            "{} | 重要性: {:.2} | 类别: {}",
            memory.content, memory.importance, memory.category
        );
        ctx.vfs.write_abstract(uri, &abstract_content).await?;

        tracing::debug!(memory_id = %memory.id, uri = %uri, "记忆已持久化");
        Ok(())
    }

    /// 处理单个会话：提取并持久化记忆。
    async fn process_session(&self, ctx: &TaskContext, session_uri: &TianyanUri) -> Result<usize> {
        let conversation = ctx
            .vfs
            .read_content(session_uri, ContentLevel::Detail)
            .await?;

        let memories = ctx.memory_extractor.extract(&conversation).await?;

        let mut stored_count = 0;
        for mut memory in memories {
            memory.source_session = Some(session_uri.to_string());
            if let Err(e) = self.store_memory(ctx, &memory).await {
                tracing::warn!(memory_id = %memory.id, error = %e, "存储记忆失败");
                continue;
            }
            stored_count += 1;
        }

        let message_count = conversation
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count();
        let state = SessionExtractionState::new(message_count);
        self.write_state(ctx, session_uri, &state).await?;

        tracing::info!(
            session_uri = %session_uri,
            stored_count = stored_count,
            message_count = message_count,
            "记忆提取完成"
        );

        Ok(stored_count)
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

        let sessions = match self.scan_sessions(ctx).await {
            Ok(uris) => uris,
            Err(e) => {
                return TaskResult::failed(format!("扫描会话失败：{}", e));
            }
        };

        if sessions.is_empty() {
            tracing::debug!("没有需要提取记忆的会话");
            return TaskResult::success(0);
        }

        tracing::info!("发现 {} 个会话需要提取记忆", sessions.len());

        let mut total_memories = 0;
        let mut processed_count = 0;

        for session_uri in sessions {
            match self.process_session(ctx, &session_uri).await {
                Ok(count) => {
                    total_memories += count;
                    processed_count += 1;
                }
                Err(e) => {
                    tracing::warn!(session_uri = %session_uri, error = %e, "处理会话失败");
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

    #[test]
    fn test_extraction_state_roundtrip() {
        let state = SessionExtractionState::new(42);
        let json = serde_json::to_string(&state).unwrap();
        let parsed: SessionExtractionState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.last_extracted_message_count, 42);
        assert!(!parsed.last_extracted_at.is_empty());
    }
}

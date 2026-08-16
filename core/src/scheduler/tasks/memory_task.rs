//! 记忆提取任务。
//!
//! 扫描 VFS 中的会话，对含有新消息的会话调用 MemoryExtractor 提取长期记忆。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::common::error::Result;
use crate::common::types::{
    ContentLevel, ContextNamespace, MemoryCategory, MemoryEntry, TianyanUri,
};
use crate::memory::format_memory_as_markdown;
use crate::scheduler::{TaskContext, TaskHandler, TaskResult};
use crate::session::parse_message_lines;

/// 会话的记忆提取状态，存储在 `tianyan://memory/events/extraction_state/{session_id}`
/// （迁出 Session 命名空间——旧版存放于会话目录 `_metadata` 子文件，会被
/// `list_sessions` 当作空会话列出）。
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

/// 提取状态的存储 URI（`tianyan://memory/events/extraction_state/{session_id}`）。
fn state_uri(session_uri: &TianyanUri) -> TianyanUri {
    let session_id = session_uri.path().last().cloned().unwrap_or_default();
    TianyanUri::new(
        ContextNamespace::Memory,
        vec![
            "events".to_string(),
            "extraction_state".to_string(),
            session_id,
        ],
    )
}

/// 旧版水位线 URI（会话目录下 `_metadata` 子文件；一次性迁移用）。
fn legacy_state_uri(session_uri: &TianyanUri) -> TianyanUri {
    session_uri.append("_metadata")
}

/// 记忆提取任务。
///
/// 定时扫描 Session 命名空间下的所有会话，对比持久化的提取状态
/// 判断是否有新消息需要提取。
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

    /// 读取上次提取状态（新位置优先；旧版 `_metadata` 回退读取，不删除——
    /// 待写入成功后再清理，避免提取失败丢水位线导致重复提取）。
    async fn read_state(
        &self,
        ctx: &TaskContext,
        session_uri: &TianyanUri,
    ) -> Option<SessionExtractionState> {
        let content = match ctx
            .vfs
            .read_content(&state_uri(session_uri), ContentLevel::Detail)
            .await
        {
            Ok(json) => json,
            Err(_) => match ctx
                .vfs
                .read_content(&legacy_state_uri(session_uri), ContentLevel::Detail)
                .await
            {
                Ok(json) => json,
                Err(_) => return None,
            },
        };
        match serde_json::from_str::<SessionExtractionState>(&content) {
            Ok(state) => Some(state),
            Err(e) => {
                tracing::warn!(
                    session_uri = %session_uri,
                    error = %e,
                    "解析提取状态文件失败，将全量提取"
                );
                None
            }
        }
    }

    /// 写入提取状态到新位置；成功后清理旧版 `_metadata` 文件（一次性迁移）。
    async fn write_state(
        &self,
        ctx: &TaskContext,
        session_uri: &TianyanUri,
        state: &SessionExtractionState,
    ) -> Result<()> {
        let target_uri = state_uri(session_uri);
        let json = serde_json::to_string(state).map_err(|e| {
            crate::common::error::TianyanError::Custom(format!("序列化错误：{}", e))
        })?;
        if !ctx.vfs.exists(&target_uri).await? {
            ctx.vfs.create_file(&target_uri).await?;
        }
        ctx.vfs.write_content(&target_uri, &json).await?;
        // 迁移完成：删除旧版水位线（失败仅告警——list_sessions 已按目录过滤）
        let legacy_uri = legacy_state_uri(session_uri);
        if ctx.vfs.exists(&legacy_uri).await.unwrap_or(false) {
            if let Err(e) = ctx.vfs.delete(&legacy_uri).await {
                tracing::warn!(error = %e, "旧版提取状态文件删除失败（无害）");
            }
        }
        Ok(())
    }

    /// 统计 session JSONL 中的消息数。
    ///
    /// 经共享解析 [`parse_message_lines`] 计数——**不含首行 SessionHeader**，
    /// 修复旧版按行数统计把头部行计入的差一问题（头部缺失/补齐会触发
    /// 意外全量重提；水位线从此与真实消息数精确对齐）。
    async fn count_session_messages(
        &self,
        ctx: &TaskContext,
        session_uri: &TianyanUri,
    ) -> Result<usize> {
        let content = ctx
            .vfs
            .read_content(session_uri, ContentLevel::Detail)
            .await?;
        Ok(parse_message_lines(&content).len())
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

        // L1 摘要携带来源会话（G3：注入上下文时出处对 LLM 可见）。
        let mut abstract_content = format!(
            "{} | 重要性: {:.2} | 类别: {}",
            memory.content, memory.importance, memory.category
        );
        if let Some(ref session_id) = memory.source_session {
            abstract_content.push_str(&format!(" | 来源会话: {}", session_id));
        }
        ctx.vfs.write_abstract(uri, &abstract_content).await?;

        tracing::debug!(memory_id = %memory.id, uri = %uri, "记忆已持久化");
        Ok(())
    }

    /// 从会话 JSONL 文本中提取消息 ID 列表（消息级溯源，G3）。
    ///
    /// 经共享解析 [`parse_message_lines`]（跳过首行 SessionHeader 与解析失败行），
    /// 不再自行解析格式内部。全失败时返回空列表（退化为仅会话级溯源）。
    fn extract_message_ids(conversation: &str) -> Vec<String> {
        parse_message_lines(conversation)
            .into_iter()
            .map(|sm| sm.id)
            .collect()
    }

    /// 处理单个会话：提取并持久化记忆。
    async fn process_session(&self, ctx: &TaskContext, session_uri: &TianyanUri) -> Result<usize> {
        let conversation = ctx
            .vfs
            .read_content(session_uri, ContentLevel::Detail)
            .await?;

        let source_message_ids = Self::extract_message_ids(&conversation);
        let memories = ctx
            .memory_extractor
            .extract(&conversation, &source_message_ids)
            .await?;

        // 技能使用复审（顺路：会话内容已在内存；失败不影响记忆提取）
        if let Err(e) = ctx
            .skill_reviewer
            .review_conversation(session_uri.as_ref(), &conversation)
            .await
        {
            tracing::warn!(session_uri = %session_uri, error = %e, "技能使用复审失败（不影响记忆提取）");
        }

        let mut stored_count = 0;
        for mut memory in memories {
            memory.source_session = Some(session_uri.to_string());

            // 偏好类写前校验（G3，opt-in）：验证为稳定长期偏好才持久化。
            if memory.category == MemoryCategory::Preference && ctx.config.memory.verify_preferences
            {
                match ctx.memory_extractor.verify_preference(&memory).await {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::info!(
                            memory_id = %memory.id,
                            "偏好记忆未通过写前校验，跳过持久化"
                        );
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(
                            memory_id = %memory.id,
                            error = %e,
                            "偏好记忆校验失败，跳过持久化"
                        );
                        continue;
                    }
                }
            }

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

        // G5 跨运行状态：读取上次周期摘要（任务上下文延续）
        match ctx.task_state.read("memory_extraction").await {
            Ok(Some(prev)) => {
                let first_line = prev.lines().next().unwrap_or("").to_string();
                tracing::info!(prev_summary = %first_line, "记忆提取任务上次运行摘要");
            }
            Ok(None) => tracing::debug!("记忆提取任务首次运行（无历史状态）"),
            Err(e) => tracing::warn!(error = %e, "读取任务状态失败"),
        }

        let sessions = match self.scan_sessions(ctx).await {
            Ok(uris) => uris,
            Err(e) => {
                return TaskResult::failed(crate::common::error::TianyanError::Custom(format!(
                    "记忆提取任务：扫描会话失败：{}",
                    e
                )));
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

        // G5 跨运行状态：写入本次周期摘要（下次运行可读）
        let summary = format!(
            "# 记忆提取周期摘要\n\n- 时间: {}\n- 处理会话: {}\n- 提取记忆: {}\n",
            chrono::Utc::now().to_rfc3339(),
            processed_count,
            total_memories
        );
        if let Err(e) = ctx.task_state.write("memory_extraction", &summary).await {
            tracing::warn!(error = %e, "任务状态写入失败");
        }

        TaskResult::success(total_memories)
    }

    fn name(&self) -> &str {
        "memory_extraction"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use crate::common::types::{DetailedTokenUsage, MessageRole, MessageTime, Part, PartTime};
    use crate::memory::{ExtractionConfig, MemoryExtractor};
    use crate::model::ChatService;
    use crate::test_utils::{MockChatService, MockVfs};
    use crate::vfs::SummaryEngine;

    /// 构造任务执行上下文（MockVfs + mock 服务）。
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

    /// 构造单条消息（测试辅助）。
    fn make_msg(id: &str) -> crate::common::types::StructuredMessage {
        crate::common::types::StructuredMessage {
            id: id.to_string(),
            parent_id: None,
            role: MessageRole::User,
            parts: vec![Part::Text {
                text: "hi".to_string(),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: "s1".to_string(),
            finish: None,
            compression_marker: false,
        }
    }

    /// 头部行 + N 条消息的 JSONL 文本。
    fn jsonl_with_header(message_ids: &[&str]) -> String {
        let header = serde_json::to_string(&crate::session::SessionHeader::default()).unwrap();
        let mut content = format!("{header}\n");
        for id in message_ids {
            content.push_str(&serde_json::to_string(&make_msg(id)).unwrap());
            content.push('\n');
        }
        content
    }

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

    #[tokio::test]
    async fn test_count_session_messages_excludes_header() {
        // 回归保护：消息计数不得包含首行 SessionHeader（旧版按行数统计差 1，
        // 头部补齐会触发意外全量重提）。
        let vfs = Arc::new(MockVfs::new());
        let ctx = make_context(vfs.clone());
        let session_uri = TianyanUri::parse("tianyan://session/s1").unwrap();
        vfs.set_content(
            &session_uri,
            ContentLevel::Detail,
            &jsonl_with_header(&["m1", "m2"]),
        );

        let task = MemoryTask::new();
        let count = task
            .count_session_messages(&ctx, &session_uri)
            .await
            .unwrap();
        assert_eq!(count, 2, "头部行不得计入消息数");
    }

    #[test]
    fn test_extract_message_ids_excludes_header() {
        // 回归保护：消息溯源 ID 列表不含头部行
        let ids = MemoryTask::extract_message_ids(&jsonl_with_header(&["m1", "m2"]));
        assert_eq!(ids, vec!["m1".to_string(), "m2".to_string()]);
    }
}

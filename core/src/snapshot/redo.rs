//! 重做（redo）子系统：回退后恢复工作区树与被截断消息。
//!
//! 与 capture/restore/diff/gc 完全正交（deletion test 正）：重做数据存于
//! `{root}/{session_id}/redo/`，按被回退消息的 ID 组织（消息 ID 是两端共享的
//! 稳定定位键——前端索引与服务端消息列表可能错位）。

use std::path::PathBuf;

use tokio::fs;

use crate::common::error::{Result, TianyanError};
use crate::common::types::StructuredMessage;

use super::{SnapshotManager, SnapshotTree};

impl SnapshotManager {
    /// 回退时保存重做状态：捕获当前工作区树 + 被截断的消息。
    ///
    /// 重做数据存于 `{root}/{session_id}/redo/` 下，按被回退消息的 ID 组织
    /// （前端索引与服务端消息列表可能错位——tool/system 消息被前端合并过滤，
    /// 数字索引不可靠；消息 ID 是稳定的定位键）。
    pub async fn save_redo(
        &self,
        session_id: &str,
        message_id: &str,
        messages: &[StructuredMessage],
    ) -> Result<()> {
        self.validate_session_id(session_id)?;
        let key = super::sanitize_key(message_id);

        let redo_dir = self.redo_dir(session_id);
        fs::create_dir_all(&redo_dir)
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 创建重做目录失败: {e}")))?;

        // 1. 捕获当前工作区树（对象库全局去重，内容已存在则不重复存储）。
        //
        // **增量捕获**：复用 `latest.cache.json`（最近一次轮内捕获的 mtime/size
        // 缓存）短路未变更文件——大工作区（实测数千~数万文件）全树哈希是
        // 「回退处理中」延迟的主要来源；缓存缺失/损坏退化为全量（不阻断回退）。
        // 缓存**写回**仍用 redo 自己的 cache_path：不污染 latest 指针（后者语义
        // 是「最近一次轮内捕获」，供下一轮 capture 复用）。
        if self.workdir.exists() {
            let tree_path = redo_dir.join(format!("tree-{key}.json"));
            let cache_path = redo_dir.join(format!("tree-{key}.cache.json"));
            let prev_cache = self
                .load_cache(&self.latest_cache_path(session_id))
                .await
                .unwrap_or(None);
            self.capture_tree_to(&self.workdir, &tree_path, &cache_path, prev_cache.as_ref())
                .await?;
        }

        // 2. 保存被截断的消息
        let messages_path = redo_dir.join(format!("messages-{key}.json"));
        let json = serde_json::to_string(messages)
            .map_err(|e| TianyanError::Custom(format!("snapshot: 序列化重做消息失败: {e}")))?;
        fs::write(&messages_path, json)
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 写入重做消息失败: {e}")))?;

        tracing::debug!(session = %session_id, message_id, "已保存重做状态");
        Ok(())
    }

    /// 重做：恢复重做工作区树,返回被截断的消息与恢复的文件数。
    ///
    /// 重做成功后自动清理该消息的重做数据（一次性语义）。
    pub async fn load_redo(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<Option<(Vec<StructuredMessage>, usize)>> {
        self.validate_session_id(session_id)?;

        let key = super::sanitize_key(message_id);
        let redo_dir = self.redo_dir(session_id);
        let messages_path = redo_dir.join(format!("messages-{key}.json"));
        if !messages_path.exists() {
            return Ok(None);
        }

        // 1. 恢复工作区树
        let mut restored = 0usize;
        let tree_path = redo_dir.join(format!("tree-{key}.json"));
        if tree_path.exists() {
            let content = fs::read_to_string(&tree_path)
                .await
                .map_err(|e| TianyanError::Custom(format!("snapshot: 读取重做树失败: {e}")))?;
            let tree: SnapshotTree = serde_json::from_str(&content)
                .map_err(|e| TianyanError::Custom(format!("snapshot: 解析重做树失败: {e}")))?;
            // 增量：复用 latest 缓存短路未变更文件（同 save_redo 的加速路径）
            let prev_cache = self
                .load_cache(&self.latest_cache_path(session_id))
                .await
                .unwrap_or(None);
            restored = self
                .restore_tree(tree, &self.workdir, prev_cache.as_ref())
                .await?;
        }

        // 2. 读取被截断的消息
        let content = fs::read_to_string(&messages_path)
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 读取重做消息失败: {e}")))?;
        let messages: Vec<StructuredMessage> = serde_json::from_str(&content)
            .map_err(|e| TianyanError::Custom(format!("snapshot: 解析重做消息失败: {e}")))?;

        // 3. 清理重做数据（一次性）
        if let Err(e) = fs::remove_file(&messages_path).await {
            tracing::debug!(error = %e, session = %session_id, "snapshot: 清理重做消息文件失败");
        }
        if let Err(e) = fs::remove_file(&tree_path).await {
            tracing::debug!(error = %e, session = %session_id, "snapshot: 清理重做树文件失败");
        }

        tracing::info!(session = %session_id, message_id, restored, "已重做工作区文件");
        Ok(Some((messages, restored)))
    }

    /// 检查指定消息的重做状态是否存在。
    pub async fn has_redo(&self, session_id: &str, message_id: &str) -> bool {
        self.redo_dir(session_id)
            .join(format!("messages-{}.json", super::sanitize_key(message_id)))
            .exists()
    }

    /// 重做数据目录（`{root}/{session_id}/redo/`）。
    pub(crate) fn redo_dir(&self, session_id: &str) -> PathBuf {
        self.root.join(session_id).join("redo")
    }
}

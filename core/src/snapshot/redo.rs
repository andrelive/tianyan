//! 重做（redo）子系统：回退后恢复工作区树。
//!
//! 与 capture/restore/diff/gc 完全正交（deletion test 正）：重做数据存于
//! `{root}/{session_id}/redo/`，按被回退消息的 ID 组织（消息 ID 是两端共享的
//! 稳定定位键——前端索引与服务端消息列表可能错位）。
//!
//! **本子系统只承担「文件」那一半**（ADR-056）：被回退的**消息**不再另存副本——
//! 回退只把会话表的可见水位前移，行留在表里，重做把水位移回即可（见
//! [`SessionWorkingSet::redo_visible`](crate::agent::working_set::SessionWorkingSet::redo_visible)）。
//! 此前这里还会写 `messages-{key}.json`（整段 `serde_json` 序列化落盘、重做时读回
//! 再整表重写），那套机制已随水位方案退役；老版本残留的 `messages-*.json` 会在下次
//! 回退写入时被清理。

use std::path::PathBuf;

use tokio::fs;

use crate::common::error::{Result, TianyanError};

use super::{RestoreReport, SnapshotManager, SnapshotTree};

impl SnapshotManager {
    /// 回退时保存重做状态：捕获当前工作区树。
    ///
    /// 重做数据存于 `{root}/{session_id}/redo/` 下，按被回退消息的 ID 组织
    /// （前端索引与服务端消息列表可能错位——tool/system 消息被前端合并过滤，
    /// 数字索引不可靠；消息 ID 是稳定的定位键）。
    ///
    /// **一次回退只保留一份**：重做只叠一层（再次回退时旧的那层已没有回到路径），
    /// 所以写入前先清掉本会话 redo 目录里其它锚点的文件——否则连续回退会堆下多份
    /// 完整树（每份可能几十 MB），要等 GC 才回收。
    ///
    /// **返回刚捕获的「当前工作区树」**：回退事务紧接着要调 `restore`，而它的
    /// 「当前状态」与这棵树完全等价——复用可省掉一次全树扫描（实测 949 文件省
    /// 417 ms）。workdir 不存在时返回空树（此时 `restore` 本身也会 early return）。
    pub async fn save_redo(&self, session_id: &str, message_id: &str) -> Result<SnapshotTree> {
        self.validate_session_id(session_id)?;
        let key = super::sanitize_key(message_id);

        let redo_dir = self.redo_dir(session_id);
        fs::create_dir_all(&redo_dir)
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 创建重做目录失败: {e}")))?;

        // 一层撤销：清掉其它锚点的树 / 缓存 / 旧版消息副本（本次要写的除外）
        let keep_tree = format!("tree-{key}.json");
        let keep_cache = format!("tree-{key}.cache.json");
        if let Ok(mut entries) = fs::read_dir(&redo_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                if name != keep_tree && name != keep_cache {
                    if let Err(e) = fs::remove_file(entry.path()).await {
                        tracing::debug!(
                            error = %e, file = %name,
                            "snapshot: 清理旧重做文件失败（不阻断回退）"
                        );
                    }
                }
            }
        }

        // 捕获当前工作区树（对象库全局去重，内容已存在则不重复存储）。
        //
        // **增量捕获**：复用 `latest.cache.json`（最近一次轮内捕获的 mtime/size
        // 缓存）短路未变更文件——大工作区（实测数千~数万文件）全树哈希是
        // 「回退处理中」延迟的主要来源；缓存缺失/损坏退化为全量（不阻断回退）。
        // 缓存**写回**仍用 redo 自己的 cache_path：不污染 latest 指针（后者语义
        // 是「最近一次轮内捕获」，供下一轮 capture 复用）。
        let mut captured = SnapshotTree::new();
        if self.workdir.exists() {
            let tree_path = redo_dir.join(&keep_tree);
            let cache_path = redo_dir.join(&keep_cache);
            let prev_cache = self
                .load_cache(&self.latest_cache_path(session_id))
                .await
                .unwrap_or(None);
            // racy 判据的 T 取自 **prev_cache 的来源**（`latest.cache.json`），
            // 而不是本次要写的 `cache_path`（redo 目录下按 key 一份，可能是旧残渣）
            let prev_cache_at_ns = self.snapshot_at_for(session_id).await;
            captured = self
                .capture_tree_to(
                    &self.workdir,
                    &tree_path,
                    &cache_path,
                    prev_cache.as_ref(),
                    prev_cache_at_ns,
                )
                .await?;
        }

        tracing::debug!(session = %session_id, message_id, "已保存重做状态");
        Ok(captured)
    }

    /// 重做：恢复该锚点的重做工作区树，返回恢复报告（一次性语义：读完即删）。
    ///
    /// 无重做树（该锚点未捕获 / 已被 GC / 旧版数据）时返回**空报告**——消息层的
    /// 重做由可见水位独立承担，不因缺树而失败（回退本来就允许「只回退消息」）。
    ///
    /// 树同样走 v2 格式（`save_redo` 经 [`SnapshotManager::capture_tree_to`] 捕获），
    /// 因此重做的删除也受同一套护栏约束——重做把「回退前的工作区」放回来，
    /// 期间用户新建的文件同样不能因归属不明被删。
    pub async fn restore_redo_tree(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<RestoreReport> {
        self.validate_session_id(session_id)?;

        let key = super::sanitize_key(message_id);
        let redo_dir = self.redo_dir(session_id);
        let tree_path = redo_dir.join(format!("tree-{key}.json"));
        if !tree_path.exists() {
            return Ok(RestoreReport::default());
        }

        // 读取与解析统一走 `read_tree_file`（v1/v2 + gzip/明文兼容）
        let tree = SnapshotManager::read_tree_file(&tree_path).await?;
        // 增量：复用 latest 缓存短路未变更文件（同 save_redo 的加速路径）
        let prev_cache = self
            .load_cache(&self.latest_cache_path(session_id))
            .await
            .unwrap_or(None);
        let snapshot_at_ns = self.snapshot_at_for(session_id).await;
        let report = self
            .restore_tree(&tree, &self.workdir, prev_cache.as_ref(), snapshot_at_ns)
            .await?;

        // 一次性：树文件消费即删（缓存是下次捕获的加速副产物，由 GC 按孤儿清理）
        if let Err(e) = fs::remove_file(&tree_path).await {
            tracing::debug!(error = %e, session = %session_id, "snapshot: 清理重做树文件失败");
        }

        tracing::info!(
            session = %session_id,
            message_id,
            written = report.written,
            deleted = report.deleted,
            suppressed = report.deletion_suppressed,
            "已重做工作区文件"
        );
        Ok(report)
    }

    /// 检查指定消息的重做**树**是否存在。
    ///
    /// 只回答「文件那一半」——消息层的可重做性看会话可见水位
    /// （`SessionStore::visible_from_seq`），两者独立（缺树仍可回退/重做消息）。
    pub async fn has_redo(&self, session_id: &str, message_id: &str) -> bool {
        self.redo_dir(session_id)
            .join(format!("tree-{}.json", super::sanitize_key(message_id)))
            .exists()
    }

    /// 重做数据目录（`{root}/{session_id}/redo/`）。
    pub(crate) fn redo_dir(&self, session_id: &str) -> PathBuf {
        self.root.join(session_id).join("redo")
    }
}

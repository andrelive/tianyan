//! 目标（goal）存储。
//!
//! 长期目标管理 + 进度跟踪：目标可关联待办（todos.goal_id），
//! 进度按关联待办的完成比例自动计算。目标与会话绑定（`session_id`，
//! 由 `goal` 动态工具在会话内创建；会话页仅展示当前会话的活跃目标）。
//! 持久化为 `{data_dir}/goals.json`
//! （与 todos.json 同类的运行期结构化产物，不经 VFS）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::common::error::{Result, TianyanError};

/// 目标状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    /// 进行中。
    Active,
    /// 已完成。
    Completed,
    /// 已归档（不再跟踪）。
    Archived,
}

impl GoalStatus {
    /// 解析状态字符串（API 语义：snake_case；未知值返回 None）。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "completed" => Some(Self::Completed),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

/// 目标条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    /// 唯一 ID。
    pub id: String,
    /// 标题。
    pub title: String,
    /// 详细描述（可选）。
    pub description: Option<String>,
    /// 状态。
    pub status: GoalStatus,
    /// 归属会话 id（会话绑定；None = 历史遗留/无会话归属，不进会话面板）。
    #[serde(default)]
    pub session_id: Option<String>,
    /// 创建时间（epoch 秒）。
    pub created_at: i64,
    /// 更新时间（epoch 秒）。
    pub updated_at: i64,
    /// 完成时间（epoch 秒；未完成时为 None）。
    pub completed_at: Option<i64>,
    /// 目标日期（epoch 秒；可选）。
    pub target_date: Option<i64>,
}

impl Goal {
    /// 创建新目标（时间戳由调用方注入，便于测试）。
    pub fn new(
        title: String,
        description: Option<String>,
        target_date: Option<i64>,
        session_id: Option<String>,
        now: i64,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            description,
            status: GoalStatus::Active,
            session_id,
            created_at: now,
            updated_at: now,
            completed_at: None,
            target_date,
        }
    }
}

/// 目标存储：内存态 + JSON 文件持久化（写时全量落盘）。
pub struct GoalStore {
    file_path: PathBuf,
    goals: Arc<RwLock<Vec<Goal>>>,
    /// 是否已从磁盘加载（避免删除全部后 list 重新加载复活旧条目）。
    loaded: Arc<std::sync::atomic::AtomicBool>,
}

impl GoalStore {
    /// 创建存储（data_dir 用于定位 goals.json；不立即读盘，首次 list 时加载）。
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            file_path: data_dir.join("goals.json"),
            goals: Arc::new(RwLock::new(Vec::new())),
            loaded: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// 从磁盘加载（文件缺失/损坏时静默空列表，不阻塞启动）。
    async fn load(&self) {
        let content = match std::fs::read_to_string(&self.file_path) {
            Ok(c) => c,
            Err(_) => return,
        };
        match serde_json::from_str::<Vec<Goal>>(&content) {
            Ok(goals) => {
                *self.goals.write().await = goals;
            }
            Err(e) => {
                tracing::warn!(error = %e, path = %self.file_path.display(), "目标加载失败（使用空列表）")
            }
        }
    }

    /// 持久化到磁盘（写失败仅告警——内存态仍可用，下次写重试）。
    async fn save(&self) {
        let goals = self.goals.read().await;
        match serde_json::to_string_pretty(&*goals) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.file_path, json) {
                    tracing::error!(error = %e, path = %self.file_path.display(), "目标持久化失败");
                }
            }
            Err(e) => tracing::error!(error = %e, "目标序列化失败"),
        }
    }

    /// 列出全部目标（按创建时间升序；首次调用时加载磁盘）。
    pub async fn list(&self) -> Vec<Goal> {
        if !self.loaded.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.load().await;
        }
        let goals = self.goals.read().await;
        let mut v = goals.clone();
        v.sort_by_key(|a| a.created_at);
        v
    }

    /// 创建目标（持久化后返回条目）。
    pub async fn create(
        &self,
        title: String,
        description: Option<String>,
        target_date: Option<i64>,
        session_id: Option<String>,
    ) -> Result<Goal> {
        let title = title.trim().to_string();
        if title.is_empty() {
            return Err(TianyanError::invalid_input("goals: 目标标题不能为空"));
        }
        let now = chrono::Utc::now().timestamp();
        let goal = Goal::new(title, description, target_date, session_id, now);
        self.goals.write().await.push(goal.clone());
        self.save().await;
        Ok(goal)
    }

    /// 更新目标（部分字段；返回更新后的条目，不存在返回 None）。
    pub async fn update(
        &self,
        id: &str,
        title: Option<String>,
        description: Option<String>,
        status: Option<GoalStatus>,
        target_date: Option<i64>,
    ) -> Result<Option<Goal>> {
        let mut goals = self.goals.write().await;
        let goal = goals
            .iter_mut()
            .find(|g| g.id == id)
            .ok_or_else(|| TianyanError::not_found(format!("goals: 目标不存在：{id}")))?;
        if let Some(t) = title {
            let t = t.trim().to_string();
            if t.is_empty() {
                return Err(TianyanError::invalid_input("goals: 目标标题不能为空"));
            }
            goal.title = t;
        }
        if let Some(d) = description {
            goal.description = Some(d);
        }
        if let Some(s) = status {
            goal.status = s;
            goal.completed_at = if s == GoalStatus::Completed {
                Some(chrono::Utc::now().timestamp())
            } else {
                None
            };
        }
        if let Some(d) = target_date {
            goal.target_date = if d <= 0 { None } else { Some(d) };
        }
        goal.updated_at = chrono::Utc::now().timestamp();
        let updated = goal.clone();
        drop(goals);
        self.save().await;
        Ok(Some(updated))
    }

    /// 列出归属指定会话的目标（按创建时间升序；会话页数据源）。
    pub async fn list_by_session(&self, session_id: &str) -> Vec<Goal> {
        self.list()
            .await
            .into_iter()
            .filter(|g| g.session_id.as_deref() == Some(session_id))
            .collect()
    }

    /// 删除归属指定会话的全部目标（会话删除级联清理）。返回删除数。
    pub async fn delete_by_session(&self, session_id: &str) -> usize {
        if !self.loaded.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.load().await;
        }
        let mut goals = self.goals.write().await;
        let before = goals.len();
        goals.retain(|g| g.session_id.as_deref() != Some(session_id));
        let removed = before - goals.len();
        drop(goals);
        if removed > 0 {
            self.save().await;
        }
        removed
    }

    /// 删除目标（返回是否删除）。
    pub async fn delete(&self, id: &str) -> Result<bool> {
        let mut goals = self.goals.write().await;
        let before = goals.len();
        goals.retain(|g| g.id != id);
        let removed = goals.len() != before;
        drop(goals);
        if removed {
            self.save().await;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (GoalStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (GoalStore::new(dir.path()), dir)
    }

    #[tokio::test]
    async fn test_create_list_update_delete() {
        let (store, _dir) = temp_store();
        let goal = store
            .create("学会 Rust".into(), Some("完成 0.2 开发".into()), None, None)
            .await
            .unwrap();
        assert_eq!(goal.status, GoalStatus::Active);

        let list = store.list().await;
        assert_eq!(list.len(), 1);

        let updated = store
            .update(&goal.id, None, None, Some(GoalStatus::Completed), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.status, GoalStatus::Completed);
        assert!(updated.completed_at.is_some());

        assert!(store.delete(&goal.id).await.unwrap());
        assert!(!store.delete(&goal.id).await.unwrap());
        assert!(store.list().await.is_empty());
    }

    #[tokio::test]
    async fn test_persists_across_instances() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = GoalStore::new(dir.path());
            store
                .create("持久化目标".into(), None, None, None)
                .await
                .unwrap();
        }
        {
            let store = GoalStore::new(dir.path());
            let list = store.list().await;
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].title, "持久化目标");
        }
    }

    #[tokio::test]
    async fn test_empty_title_rejected() {
        let (store, _dir) = temp_store();
        let err = store
            .create("  ".into(), None, None, None)
            .await
            .unwrap_err();
        assert!(err.is_invalid_input());
    }

    #[tokio::test]
    async fn test_update_missing_returns_not_found() {
        let (store, _dir) = temp_store();
        let err = store
            .update("nope", None, None, None, None)
            .await
            .unwrap_err();
        assert!(err.is_not_found());
    }

    #[tokio::test]
    async fn test_session_binding() {
        let (store, _dir) = temp_store();
        let a = store
            .create("会话内目标".into(), None, None, Some("s-1".into()))
            .await
            .unwrap();
        let _b = store
            .create("其他会话目标".into(), None, None, Some("s-2".into()))
            .await
            .unwrap();
        let _c = store
            .create("无归属目标".into(), None, None, None)
            .await
            .unwrap();

        let mine = store.list_by_session("s-1").await;
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].id, a.id);

        assert_eq!(store.delete_by_session("s-2").await, 1);
        assert_eq!(store.delete_by_session("s-2").await, 0);
        assert_eq!(store.list().await.len(), 2);
    }
}

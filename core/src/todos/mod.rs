//! 待办清单（todolist）存储。
//!
//! 用户跟踪多步任务的结构化数据：创建/完成/删除/优先级/关联目标。
//! 持久化为 `{data_dir}/todos.json`（与定时智能体任务
//! `scheduled_agent_tasks.json` 同类的运行期结构化产物，不经 VFS——
//! VFS 面向文档型上下文内容，频繁小更新的结构化 CRUD 数据不匹配其
//! 文档模型；会话（ADR-018）/快照（ADR-006）同类例外）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::common::error::{Result, TianyanError};

/// 待办状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// 未开始。
    Pending,
    /// 进行中。
    InProgress,
    /// 已完成。
    Completed,
}

impl TodoStatus {
    /// 解析状态字符串（API 语义：snake_case；未知值返回 None）。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

/// 优先级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoPriority {
    /// 低。
    Low,
    /// 中（默认）。
    Medium,
    /// 高。
    High,
}

impl TodoPriority {
    /// 解析优先级字符串（未知值返回 None）。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// 待办条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    /// 唯一 ID。
    pub id: String,
    /// 标题。
    pub title: String,
    /// 详细描述（可选）。
    pub description: Option<String>,
    /// 状态。
    pub status: TodoStatus,
    /// 优先级。
    pub priority: TodoPriority,
    /// 关联目标 id（可选；目标进度按关联待办自动计算）。
    pub goal_id: Option<String>,
    /// 创建时间（epoch 秒）。
    pub created_at: i64,
    /// 更新时间（epoch 秒）。
    pub updated_at: i64,
    /// 完成时间（epoch 秒；未完成时为 None）。
    pub completed_at: Option<i64>,
    /// 截止时间（epoch 秒；可选）。
    pub due_at: Option<i64>,
}

impl TodoItem {
    /// 创建新条目（时间戳由调用方注入，便于测试）。
    pub fn new(
        title: String,
        description: Option<String>,
        priority: TodoPriority,
        goal_id: Option<String>,
        due_at: Option<i64>,
        now: i64,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            description,
            status: TodoStatus::Pending,
            priority,
            goal_id,
            created_at: now,
            updated_at: now,
            completed_at: None,
            due_at,
        }
    }
}

/// 待办存储：内存态 + JSON 文件持久化（写时全量落盘）。
pub struct TodoStore {
    file_path: PathBuf,
    items: Arc<RwLock<Vec<TodoItem>>>,
    /// 是否已从磁盘加载（避免删除全部后 list 重新加载复活旧条目）。
    loaded: Arc<std::sync::atomic::AtomicBool>,
}

impl TodoStore {
    /// 创建存储（data_dir 用于定位 todos.json；不立即读盘，首次 list 时加载）。
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            file_path: data_dir.join("todos.json"),
            items: Arc::new(RwLock::new(Vec::new())),
            loaded: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// 从磁盘加载（文件缺失/损坏时静默空列表，不阻塞启动）。
    async fn load(&self) {
        let content = match std::fs::read_to_string(&self.file_path) {
            Ok(c) => c,
            Err(_) => return,
        };
        match serde_json::from_str::<Vec<TodoItem>>(&content) {
            Ok(items) => {
                *self.items.write().await = items;
            }
            Err(e) => tracing::warn!(error = %e, path = %self.file_path.display(), "待办加载失败（使用空列表）"),
        }
    }

    /// 持久化到磁盘（写失败仅告警——内存态仍可用，下次写重试）。
    async fn save(&self) {
        let items = self.items.read().await;
        match serde_json::to_string_pretty(&*items) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.file_path, json) {
                    tracing::error!(error = %e, path = %self.file_path.display(), "待办持久化失败");
                }
            }
            Err(e) => tracing::error!(error = %e, "待办序列化失败"),
        }
    }

    /// 列出全部待办（按创建时间升序；首次调用时加载磁盘）。
    pub async fn list(&self) -> Vec<TodoItem> {
        if !self.loaded.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.load().await;
        }
        let items = self.items.read().await;
        let mut v = items.clone();
        v.sort_by_key(|a| a.created_at);
        v
    }

    /// 创建待办（持久化后返回条目）。
    pub async fn create(
        &self,
        title: String,
        description: Option<String>,
        priority: TodoPriority,
        goal_id: Option<String>,
        due_at: Option<i64>,
    ) -> Result<TodoItem> {
        let title = title.trim().to_string();
        if title.is_empty() {
            return Err(TianyanError::invalid_input("todos: 待办标题不能为空"));
        }
        let now = chrono::Utc::now().timestamp();
        let item = TodoItem::new(title, description, priority, goal_id, due_at, now);
        self.items.write().await.push(item.clone());
        self.save().await;
        Ok(item)
    }

    /// 更新待办（部分字段；返回更新后的条目，不存在返回 None）。
    pub async fn update(
        &self,
        id: &str,
        title: Option<String>,
        description: Option<String>,
        status: Option<TodoStatus>,
        priority: Option<TodoPriority>,
        goal_id: Option<String>,
        due_at: Option<i64>,
    ) -> Result<Option<TodoItem>> {
        let mut items = self.items.write().await;
        let item = items
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or_else(|| TianyanError::not_found(format!("todos: 待办不存在：{id}")))?;
        if let Some(t) = title {
            let t = t.trim().to_string();
            if t.is_empty() {
                return Err(TianyanError::invalid_input("todos: 待办标题不能为空"));
            }
            item.title = t;
        }
        if let Some(d) = description {
            item.description = Some(d);
        }
        if let Some(s) = status {
            item.status = s;
            item.completed_at = if s == TodoStatus::Completed {
                Some(chrono::Utc::now().timestamp())
            } else {
                None
            };
        }
        if let Some(pr) = priority {
            item.priority = pr;
        }
        if let Some(g) = goal_id {
            item.goal_id = if g.is_empty() { None } else { Some(g) };
        }
        if let Some(d) = due_at {
            item.due_at = if d <= 0 { None } else { Some(d) };
        }
        item.updated_at = chrono::Utc::now().timestamp();
        let updated = item.clone();
        drop(items);
        self.save().await;
        Ok(Some(updated))
    }

    /// 删除待办（返回是否删除）。
    pub async fn delete(&self, id: &str) -> Result<bool> {
        let mut items = self.items.write().await;
        let before = items.len();
        items.retain(|i| i.id != id);
        let removed = items.len() != before;
        drop(items);
        if removed {
            self.save().await;
        }
        Ok(removed)
    }

    /// 按目标 id 统计（目标进度计算用）：(总数, 已完成数)。
    pub async fn count_by_goal(&self, goal_id: &str) -> (usize, usize) {
        let items = self.items.read().await;
        let linked: Vec<&TodoItem> = items
            .iter()
            .filter(|i| i.goal_id.as_deref() == Some(goal_id))
            .collect();
        let total = linked.len();
        let done = linked
            .iter()
            .filter(|i| i.status == TodoStatus::Completed)
            .count();
        (total, done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (TodoStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (TodoStore::new(dir.path()), dir)
    }

    #[tokio::test]
    async fn test_create_list_update_delete() {
        let (store, _dir) = temp_store();
        let item = store
            .create("写周报".into(), Some("总结本周进展".into()), TodoPriority::High, None, None)
            .await
            .unwrap();
        assert_eq!(item.status, TodoStatus::Pending);
        assert_eq!(item.priority, TodoPriority::High);

        let list = store.list().await;
        assert_eq!(list.len(), 1);

        // 更新状态 → completed_at 填充
        let updated = store
            .update(&item.id, None, None, Some(TodoStatus::Completed), None, None, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.status, TodoStatus::Completed);
        assert!(updated.completed_at.is_some());

        // 删除
        assert!(store.delete(&item.id).await.unwrap());
        assert!(!store.delete(&item.id).await.unwrap());
        assert!(store.list().await.is_empty());
    }

    #[tokio::test]
    async fn test_persists_across_instances() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = TodoStore::new(dir.path());
            store
                .create("持久化测试".into(), None, TodoPriority::Medium, None, None)
                .await
                .unwrap();
        }
        {
            let store = TodoStore::new(dir.path());
            let list = store.list().await;
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].title, "持久化测试");
        }
    }

    #[tokio::test]
    async fn test_empty_title_rejected() {
        let (store, _dir) = temp_store();
        let err = store
            .create("  ".into(), None, TodoPriority::Medium, None, None)
            .await
            .unwrap_err();
        assert!(err.is_invalid_input());
    }

    #[tokio::test]
    async fn test_update_missing_returns_not_found() {
        let (store, _dir) = temp_store();
        let err = store
            .update("nope", None, None, None, None, None, None)
            .await
            .unwrap_err();
        assert!(err.is_not_found());
    }

    #[tokio::test]
    async fn test_count_by_goal() {
        let (store, _dir) = temp_store();
        let gid = "goal-1".to_string();
        store
            .create("a".into(), None, TodoPriority::Low, Some(gid.clone()), None)
            .await
            .unwrap();
        let b = store
            .create("b".into(), None, TodoPriority::Low, Some(gid.clone()), None)
            .await
            .unwrap();
        store
            .update(&b.id, None, None, Some(TodoStatus::Completed), None, None, None)
            .await
            .unwrap();
        let (total, done) = store.count_by_goal(&gid).await;
        assert_eq!((total, done), (2, 1));
    }
}

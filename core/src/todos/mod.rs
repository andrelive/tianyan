//! 待办清单（todolist）存储。
//!
//! 会话绑定的智能体推理辅助工具（对齐 DSH todo_write 语义）：由 `todo`
//! 动态工具在会话内创建/推进/完成，条目归属创建它的会话（`session_id`），
//! 会话页仅展示当前会话的活跃待办，全部完成后面板消失——不是全局计划页。
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
    /// 父待办 id（可选；子待办挂靠，面板缩进展示）。
    #[serde(default)]
    pub parent_id: Option<String>,
    /// 归属会话 id（会话绑定；None = 历史遗留/无会话归属，不进会话面板）。
    #[serde(default)]
    pub session_id: Option<String>,
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
        session_id: Option<String>,
        now: i64,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            description,
            status: TodoStatus::Pending,
            priority,
            goal_id,
            parent_id: None,
            session_id,
            created_at: now,
            updated_at: now,
            completed_at: None,
            due_at,
        }
    }
}

/// 批量创建输入（工具层一次调用写整份清单）。
#[derive(Debug, Clone)]
pub struct TodoDraft {
    /// 标题（必填，非空）。
    pub title: String,
    /// 详细描述。
    pub description: Option<String>,
    /// 初始状态（缺省 Pending）。
    pub status: Option<TodoStatus>,
    /// 优先级（缺省 Medium）。
    pub priority: Option<TodoPriority>,
    /// 关联目标 id。
    pub goal_id: Option<String>,
    /// 父待办 id（必须已存在）。
    pub parent_id: Option<String>,
}

/// 批量更新输入（id + 可选字段；None = 不改动）。
#[derive(Debug, Clone, Default)]
pub struct TodoPatch {
    /// 目标待办 id（必须已存在）。
    pub id: String,
    /// 新标题。
    pub title: Option<String>,
    /// 新描述。
    pub description: Option<String>,
    /// 新状态。
    pub status: Option<TodoStatus>,
    /// 新优先级。
    pub priority: Option<TodoPriority>,
    /// 新关联目标 id（空串清除）。
    pub goal_id: Option<String>,
    /// 新父待办 id（空串清除；必须已存在）。
    pub parent_id: Option<String>,
    /// 新截止时间（epoch 秒；<=0 清除）。
    pub due_at: Option<i64>,
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

    /// 确保已从磁盘加载并返回写锁守卫。
    ///
    /// 读盘在写锁内进行（双重检查）：并发首调用者阻塞到加载完成，
    /// 拿到的是已加载数据——修复「首个 create 在加载完成前 push 到空列表、
    /// save 时把磁盘上其他会话待办全部覆盖」的竞态（曾导致
    /// 「todo 不存在或不属于当前会话」的误报）。
    async fn ensure_loaded(&self) -> tokio::sync::RwLockWriteGuard<'_, Vec<TodoItem>> {
        let mut guard = self.items.write().await;
        if !self.loaded.swap(true, std::sync::atomic::Ordering::SeqCst) {
            match std::fs::read_to_string(&self.file_path) {
                Ok(content) => match serde_json::from_str::<Vec<TodoItem>>(&content) {
                    Ok(items) => *guard = items,
                    Err(e) => {
                        tracing::warn!(error = %e, path = %self.file_path.display(), "待办加载失败（使用空列表）")
                    }
                },
                Err(_) => { /* 文件缺失 = 首次使用，空列表起步 */ }
            }
        }
        guard
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
        let guard = self.ensure_loaded().await;
        let mut v = guard.clone();
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
        session_id: Option<String>,
    ) -> Result<TodoItem> {
        let title = title.trim().to_string();
        if title.is_empty() {
            return Err(TianyanError::invalid_input("todos: 待办标题不能为空"));
        }
        let now = chrono::Utc::now().timestamp();
        let item = TodoItem::new(
            title,
            description,
            priority,
            goal_id,
            due_at,
            session_id,
            now,
        );
        let mut items = self.ensure_loaded().await;
        items.push(item.clone());
        drop(items);
        self.save().await;
        Ok(item)
    }

    /// 更新待办（部分字段；返回更新后的条目，不存在返回 None）。
    #[allow(clippy::too_many_arguments)]
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
        let patched = self
            .update_many(vec![TodoPatch {
                id: id.to_string(),
                title,
                description,
                status,
                priority,
                goal_id,
                parent_id: None,
                due_at,
            }])
            .await?;
        Ok(patched.into_iter().next())
    }

    /// 删除待办（返回是否删除）。
    pub async fn delete(&self, id: &str) -> Result<bool> {
        Ok(self.delete_many(&[id.to_string()]).await? > 0)
    }

    /// 批量创建待办（单次落盘；工具层一次调用写整份清单）。
    ///
    /// parent_id 指向的父待办必须已存在（同批新建的条目互相不可挂靠——
    /// id 由存储生成，调用方事先不知道）。
    pub async fn create_many(
        &self,
        session_id: Option<String>,
        drafts: Vec<TodoDraft>,
    ) -> Result<Vec<TodoItem>> {
        if drafts.is_empty() {
            return Err(TianyanError::invalid_input(
                "todos: 批量创建至少需要一条待办",
            ));
        }
        for d in &drafts {
            if d.title.trim().is_empty() {
                return Err(TianyanError::invalid_input("todos: 待办标题不能为空"));
            }
        }
        let mut items = self.ensure_loaded().await;
        for d in &drafts {
            if let Some(p) = &d.parent_id {
                if !items.iter().any(|i| i.id == *p) {
                    return Err(TianyanError::invalid_input(format!(
                        "todos: 父待办不存在：{p}"
                    )));
                }
            }
        }
        let now = chrono::Utc::now().timestamp();
        let mut created = Vec::with_capacity(drafts.len());
        for d in drafts {
            let mut item = TodoItem::new(
                d.title.trim().to_string(),
                d.description,
                d.priority.unwrap_or(TodoPriority::Medium),
                d.goal_id,
                None,
                session_id.clone(),
                now,
            );
            if let Some(s) = d.status {
                item.status = s;
                if s == TodoStatus::Completed {
                    item.completed_at = Some(now);
                }
            }
            item.parent_id = d.parent_id;
            created.push(item);
        }
        items.extend(created.iter().cloned());
        drop(items);
        self.save().await;
        Ok(created)
    }

    /// 批量更新待办（单次落盘；原子——任一 id 不存在则整体不上盘）。
    pub async fn update_many(&self, patches: Vec<TodoPatch>) -> Result<Vec<TodoItem>> {
        if patches.is_empty() {
            return Err(TianyanError::invalid_input("todos: 批量更新至少需要一条"));
        }
        let mut items = self.ensure_loaded().await;
        // 预校验（原子性）：全部 id 存在、标题非空、parent_id 存在
        let missing: Vec<String> = patches
            .iter()
            .filter(|p| !items.iter().any(|i| i.id == p.id))
            .map(|p| p.id.clone())
            .collect();
        if !missing.is_empty() {
            return Err(TianyanError::not_found(format!(
                "todos: 待办不存在：{}",
                missing.join(", ")
            )));
        }
        for p in &patches {
            if p.title.as_deref().unwrap_or("x").trim().is_empty() {
                return Err(TianyanError::invalid_input("todos: 待办标题不能为空"));
            }
            if let Some(pid) = &p.parent_id {
                if !pid.is_empty() && !items.iter().any(|i| i.id == *pid) {
                    return Err(TianyanError::invalid_input(format!(
                        "todos: 父待办不存在：{pid}"
                    )));
                }
            }
        }
        let now = chrono::Utc::now().timestamp();
        let mut updated = Vec::with_capacity(patches.len());
        for p in &patches {
            if let Some(item) = items.iter_mut().find(|i| i.id == p.id) {
                if let Some(t) = &p.title {
                    item.title = t.trim().to_string();
                }
                if p.description.is_some() {
                    item.description = p.description.clone();
                }
                if let Some(s) = p.status {
                    item.status = s;
                    item.completed_at = if s == TodoStatus::Completed {
                        Some(now)
                    } else {
                        None
                    };
                }
                if let Some(pr) = p.priority {
                    item.priority = pr;
                }
                if let Some(g) = &p.goal_id {
                    item.goal_id = if g.is_empty() { None } else { Some(g.clone()) };
                }
                if let Some(pid) = &p.parent_id {
                    item.parent_id = if pid.is_empty() {
                        None
                    } else {
                        Some(pid.clone())
                    };
                }
                if let Some(d) = p.due_at {
                    item.due_at = if d <= 0 { None } else { Some(d) };
                }
                item.updated_at = now;
                updated.push(item.clone());
            }
        }
        drop(items);
        self.save().await;
        Ok(updated)
    }

    /// 批量删除待办（单次落盘）。返回实际删除数。
    pub async fn delete_many(&self, ids: &[String]) -> Result<usize> {
        if ids.is_empty() {
            return Err(TianyanError::invalid_input(
                "todos: 批量删除至少需要一个 id",
            ));
        }
        let mut items = self.ensure_loaded().await;
        let before = items.len();
        items.retain(|i| !ids.iter().any(|id| id == &i.id));
        let removed = before - items.len();
        drop(items);
        if removed > 0 {
            self.save().await;
        }
        Ok(removed)
    }

    /// 列出归属指定会话的待办（按创建时间升序；会话页数据源）。
    pub async fn list_by_session(&self, session_id: &str) -> Vec<TodoItem> {
        self.list()
            .await
            .into_iter()
            .filter(|i| i.session_id.as_deref() == Some(session_id))
            .collect()
    }

    /// 删除归属指定会话的全部待办（会话删除级联清理）。返回删除数。
    pub async fn delete_by_session(&self, session_id: &str) -> usize {
        let mut items = self.ensure_loaded().await;
        let before = items.len();
        items.retain(|i| i.session_id.as_deref() != Some(session_id));
        let removed = before - items.len();
        drop(items);
        if removed > 0 {
            self.save().await;
        }
        removed
    }

    /// 按目标 id 统计（目标进度计算用）：(总数, 已完成数)。
    pub async fn count_by_goal(&self, goal_id: &str) -> (usize, usize) {
        let items = self.ensure_loaded().await;
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
            .create(
                "写周报".into(),
                Some("总结本周进展".into()),
                TodoPriority::High,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(item.status, TodoStatus::Pending);
        assert_eq!(item.priority, TodoPriority::High);

        let list = store.list().await;
        assert_eq!(list.len(), 1);

        // 更新状态 → completed_at 填充
        let updated = store
            .update(
                &item.id,
                None,
                None,
                Some(TodoStatus::Completed),
                None,
                None,
                None,
            )
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
                .create(
                    "持久化测试".into(),
                    None,
                    TodoPriority::Medium,
                    None,
                    None,
                    None,
                )
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
            .create("  ".into(), None, TodoPriority::Medium, None, None, None)
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
    async fn test_create_before_load_does_not_clobber_file() {
        // 回归：新实例首次操作若是 create（未经 list 加载），不得把磁盘上
        // 既有条目覆盖掉（曾导致「todo 不存在或不属于当前会话」误报）。
        let dir = tempfile::tempdir().unwrap();
        {
            let store = TodoStore::new(dir.path());
            store
                .create(
                    "既有待办".into(),
                    None,
                    TodoPriority::Medium,
                    None,
                    None,
                    Some("s-1".into()),
                )
                .await
                .unwrap();
        }
        {
            let store = TodoStore::new(dir.path());
            // 首个操作即批量 create（无任何 list 前置）
            store
                .create_many(
                    Some("s-2".into()),
                    vec![TodoDraft {
                        title: "新待办".into(),
                        description: None,
                        status: None,
                        priority: None,
                        goal_id: None,
                        parent_id: None,
                    }],
                )
                .await
                .unwrap();
            let all = store.list().await;
            assert_eq!(all.len(), 2, "既有待办不得被覆盖丢失");
        }
        // 落盘后重启同样双条可见
        let store = TodoStore::new(dir.path());
        assert_eq!(store.list().await.len(), 2);
    }

    #[tokio::test]
    async fn test_batch_create_update_delete() {
        let (store, _dir) = temp_store();
        // 批量创建：3 条，1 条直接 in_progress，1 条挂靠首条为子待办
        let created = store
            .create_many(
                Some("s-1".into()),
                vec![
                    TodoDraft {
                        title: "父任务".into(),
                        description: None,
                        status: Some(TodoStatus::InProgress),
                        priority: Some(TodoPriority::High),
                        goal_id: None,
                        parent_id: None,
                    },
                    TodoDraft {
                        title: "子任务".into(),
                        description: None,
                        status: None,
                        priority: None,
                        goal_id: None,
                        parent_id: None, // 占位，下方用真实 id 重建
                    },
                    TodoDraft {
                        title: "独立任务".into(),
                        description: None,
                        status: None,
                        priority: None,
                        goal_id: None,
                        parent_id: None,
                    },
                ],
            )
            .await
            .unwrap();
        assert_eq!(created.len(), 3);
        assert_eq!(created[0].status, TodoStatus::InProgress);

        // 子待办挂靠：parent_id 指向同批首条
        let child = store
            .create_many(
                Some("s-1".into()),
                vec![TodoDraft {
                    title: "真正的子任务".into(),
                    description: None,
                    status: None,
                    priority: None,
                    goal_id: None,
                    parent_id: Some(created[0].id.clone()),
                }],
            )
            .await
            .unwrap();
        assert_eq!(child[0].parent_id.as_deref(), Some(created[0].id.as_str()));

        // 批量更新：两条状态一次推进
        let updated = store
            .update_many(vec![
                TodoPatch {
                    id: created[1].id.clone(),
                    status: Some(TodoStatus::Completed),
                    ..Default::default()
                },
                TodoPatch {
                    id: created[2].id.clone(),
                    status: Some(TodoStatus::InProgress),
                    ..Default::default()
                },
            ])
            .await
            .unwrap();
        assert_eq!(updated.len(), 2);
        assert_eq!(updated[0].status, TodoStatus::Completed);
        assert!(updated[0].completed_at.is_some());

        // 原子性：批量中混入不存在 id → 整体报错不落盘
        let err = store
            .update_many(vec![
                TodoPatch {
                    id: created[0].id.clone(),
                    status: Some(TodoStatus::Completed),
                    ..Default::default()
                },
                TodoPatch {
                    id: "nope".into(),
                    status: Some(TodoStatus::Completed),
                    ..Default::default()
                },
            ])
            .await
            .unwrap_err();
        assert!(err.is_not_found());

        // 批量删除
        let removed = store
            .delete_many(&[created[1].id.clone(), created[2].id.clone()])
            .await
            .unwrap();
        assert_eq!(removed, 2);
        assert_eq!(store.list().await.len(), 2);
    }

    #[tokio::test]
    async fn test_parent_id_validation() {
        let (store, _dir) = temp_store();
        // 创建时父待办不存在 → invalid_input
        let err = store
            .create_many(
                Some("s-1".into()),
                vec![TodoDraft {
                    title: "孤儿".into(),
                    description: None,
                    status: None,
                    priority: None,
                    goal_id: None,
                    parent_id: Some("ghost".into()),
                }],
            )
            .await
            .unwrap_err();
        assert!(err.is_invalid_input());

        // 更新时挂靠不存在的父 → invalid_input
        let item = store
            .create(
                "普通待办".into(),
                None,
                TodoPriority::Medium,
                None,
                None,
                Some("s-1".into()),
            )
            .await
            .unwrap();
        let err = store
            .update_many(vec![TodoPatch {
                id: item.id.clone(),
                parent_id: Some("ghost".into()),
                ..Default::default()
            }])
            .await
            .unwrap_err();
        assert!(err.is_invalid_input());

        // 空串清除父挂靠
        let parent = store
            .create(
                "父".into(),
                None,
                TodoPriority::Medium,
                None,
                None,
                Some("s-1".into()),
            )
            .await
            .unwrap();
        store
            .update_many(vec![TodoPatch {
                id: item.id.clone(),
                parent_id: Some(parent.id.clone()),
                ..Default::default()
            }])
            .await
            .unwrap();
        let cleared = store
            .update_many(vec![TodoPatch {
                id: item.id.clone(),
                parent_id: Some(String::new()),
                ..Default::default()
            }])
            .await
            .unwrap();
        assert_eq!(cleared[0].parent_id, None);
    }

    #[tokio::test]
    async fn test_count_by_goal() {
        let (store, _dir) = temp_store();
        let gid = "goal-1".to_string();
        store
            .create(
                "a".into(),
                None,
                TodoPriority::Low,
                Some(gid.clone()),
                None,
                None,
            )
            .await
            .unwrap();
        let b = store
            .create(
                "b".into(),
                None,
                TodoPriority::Low,
                Some(gid.clone()),
                None,
                None,
            )
            .await
            .unwrap();
        store
            .update(
                &b.id,
                None,
                None,
                Some(TodoStatus::Completed),
                None,
                None,
                None,
            )
            .await
            .unwrap();
        let (total, done) = store.count_by_goal(&gid).await;
        assert_eq!((total, done), (2, 1));
    }

    #[tokio::test]
    async fn test_session_binding() {
        let (store, _dir) = temp_store();
        let a = store
            .create(
                "会话内待办".into(),
                None,
                TodoPriority::Medium,
                None,
                None,
                Some("s-1".into()),
            )
            .await
            .unwrap();
        let _b = store
            .create(
                "其他会话待办".into(),
                None,
                TodoPriority::Medium,
                None,
                None,
                Some("s-2".into()),
            )
            .await
            .unwrap();
        let _c = store
            .create(
                "无归属待办".into(),
                None,
                TodoPriority::Medium,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // list_by_session 只返回归属会话的条目
        let mine = store.list_by_session("s-1").await;
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].id, a.id);
        assert!(store.list_by_session("s-9").await.is_empty());

        // delete_by_session 只清理归属会话的条目（旧数据 None 保留）
        assert_eq!(store.delete_by_session("s-2").await, 1);
        assert_eq!(store.delete_by_session("s-2").await, 0);
        assert_eq!(store.list().await.len(), 2);
    }
}

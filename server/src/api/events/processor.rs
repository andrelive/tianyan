//! 事件消费处理器：订阅事件总线，按规则执行动作（任务触发 / 会话唤醒）。
//!
//! 装配（由集成者完成）：构造 [`ProcessorDeps`]（Agent + 会话管理器 +
//! 可选任务触发器），调用 [`EventProcessor::start`] 在独立 task 运行。
//! 规则来自配置 `events.rules`（用 [`tianyan::events::parse_rules`] 解析）。

use std::sync::Arc;

use async_trait::async_trait;
use tianyan::agent::AgentCoordinator;
use tianyan::events::{Event, EventAction, EventRule};
use tianyan::scheduler::{TaskContext, TaskScheduler};
use tianyan::session::SessionManager;

/// 任务触发抽象。
///
/// 集成者提供实现（默认可用 [`SchedulerTaskTrigger`] 包装
/// `TaskScheduler` + `TaskContext`）；未装配时 `task:` 动作仅记日志。
#[async_trait]
pub trait TaskTrigger: Send + Sync {
    /// 按任务 ID 或显示名触发调度器任务。
    ///
    /// 返回是否找到并触发了任务（未注册的任务返回 `false`，不报错）。
    async fn trigger_task(&self, task_id_or_name: &str) -> bool;
}

/// 基于 [`TaskScheduler`] 的默认任务触发器。
///
/// 先按任务 ID 精确匹配，再按显示名匹配（`get_task_info`），最后调用
/// [`TaskScheduler::execute_task`] 立即执行。
#[derive(Clone)]
pub struct SchedulerTaskTrigger {
    /// 任务调度器（共享 Arc，与 cron 调度同一实例）。
    scheduler: Arc<TaskScheduler>,
    /// 任务执行上下文（与 `start_server` 启动 cron 调度时共用）。
    ctx: Arc<TaskContext>,
}

impl SchedulerTaskTrigger {
    /// 创建调度器触发器。
    pub fn new(scheduler: Arc<TaskScheduler>, ctx: Arc<TaskContext>) -> Self {
        Self { scheduler, ctx }
    }
}

#[async_trait]
impl TaskTrigger for SchedulerTaskTrigger {
    async fn trigger_task(&self, task_id_or_name: &str) -> bool {
        let scheduler = &self.scheduler;
        let ids = scheduler.get_task_ids().await;
        // 1. 按 ID 精确匹配
        let target = if ids.iter().any(|id| id == task_id_or_name) {
            Some(task_id_or_name.to_string())
        } else {
            // 2. 按显示名匹配（get_task_info 返回 (name, cron, run_count)）
            let mut by_name = None;
            for id in &ids {
                if scheduler
                    .get_task_info(id)
                    .await
                    .is_some_and(|info| info.0 == task_id_or_name)
                {
                    by_name = Some(id.clone());
                    break;
                }
            }
            by_name
        };
        let Some(target) = target else {
            return false;
        };
        self.scheduler
            .execute_task(&target, &self.ctx)
            .await
            .is_some()
    }
}

/// 事件处理器依赖（集成者装配）。
#[derive(Clone)]
pub struct ProcessorDeps {
    /// 主 Agent（`wake:` 动作唤醒会话用）。
    pub agent: Arc<dyn AgentCoordinator>,
    /// 会话管理器（按前缀查找会话）。
    pub session_manager: Arc<dyn SessionManager>,
    /// 任务触发器（`None` = `task:` 动作仅记日志）。
    pub task_trigger: Option<Arc<dyn TaskTrigger>>,
}

impl ProcessorDeps {
    /// 创建依赖（不含任务触发器）。
    pub fn new(agent: Arc<dyn AgentCoordinator>, session_manager: Arc<dyn SessionManager>) -> Self {
        Self {
            agent,
            session_manager,
            task_trigger: None,
        }
    }

    /// 装配任务触发器。
    pub fn with_task_trigger(mut self, trigger: Arc<dyn TaskTrigger>) -> Self {
        self.task_trigger = Some(trigger);
        self
    }
}

/// 事件消费处理器。
pub struct EventProcessor;

impl EventProcessor {
    /// 启动事件消费循环（独立 task）。
    ///
    /// 订阅事件总线，对每个事件执行所有命中的规则动作：
    /// - `task:<名>` → 经 [`TaskTrigger`] 触发调度器任务；
    /// - `wake:<前缀>` → 唤醒会话 ID 以该前缀开头的所有会话。
    ///
    /// 订阅在调用方上下文完成（`start` 返回后发布的事件不丢失），
    /// 返回 `JoinHandle`（可 `abort()` 停止；随运行时销毁自动中止）。
    pub fn start(
        bus: tianyan::events::EventBus,
        rules: Vec<EventRule>,
        deps: ProcessorDeps,
    ) -> tokio::task::JoinHandle<()> {
        // 先订阅再 spawn：保证调用方在 start 返回后发布的事件必达
        let mut rx = bus.subscribe();
        tokio::spawn(async move {
            tracing::info!(rule_count = rules.len(), "事件处理器已启动");
            loop {
                let Some(event) = rx.recv().await else {
                    tracing::info!("事件总线已关闭，事件处理器退出");
                    break;
                };
                Self::process_event(&event, &rules, &deps).await;
            }
        })
    }

    /// 处理单个事件：执行所有命中的规则动作。
    async fn process_event(event: &Event, rules: &[EventRule], deps: &ProcessorDeps) {
        let matched: Vec<&EventRule> = rules.iter().filter(|r| r.matches(event)).collect();
        if matched.is_empty() {
            return;
        }
        tracing::info!(event = ?event, hit = matched.len(), "事件命中规则");
        for rule in matched {
            match &rule.action {
                EventAction::Task { name } => Self::trigger_task(name, deps).await,
                EventAction::Wake { session_prefix } => {
                    Self::wake_sessions(session_prefix, deps).await
                }
            }
        }
    }

    /// 触发调度器任务（未装配触发器或任务未注册时仅记日志）。
    async fn trigger_task(name: &str, deps: &ProcessorDeps) {
        let Some(trigger) = &deps.task_trigger else {
            tracing::warn!(
                task = %name,
                "未装配任务触发器，忽略 task 动作（集成者需提供 TaskTrigger）"
            );
            return;
        };
        if trigger.trigger_task(name).await {
            tracing::info!(task = %name, "事件触发任务执行");
        } else {
            tracing::warn!(task = %name, "事件触发任务未找到（未注册）");
        }
    }

    /// 唤醒会话 ID 以指定前缀开头的所有会话。
    async fn wake_sessions(prefix: &str, deps: &ProcessorDeps) {
        let sessions = match deps.session_manager.list_sessions().await {
            Ok(sessions) => sessions,
            Err(e) => {
                tracing::error!(error = %e, "列出会话失败，跳过 wake 动作");
                return;
            }
        };
        let targets: Vec<String> = sessions
            .iter()
            .filter(|s| s.session_id.starts_with(prefix))
            .map(|s| s.session_id.clone())
            .collect();
        if targets.is_empty() {
            tracing::warn!(prefix = %prefix, "没有会话 ID 以该前缀开头，跳过 wake 动作");
            return;
        }
        for session_id in targets {
            tracing::info!(session = %session_id, "事件唤醒会话");
            deps.agent.wake_session(&session_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;
    use std::time::Duration;
    use tianyan::agent::background::BackgroundTask;
    use tianyan::agent::{AgentResponse, AgentState, AgentStreamChunk};
    use tianyan::common::types::{Message, StructuredMessage};
    use tianyan::events::EventBus;
    use tianyan::executor::approval::{ApprovalDecision, ApprovalStatusSnapshot};
    use tianyan::session::Session;

    /// 轮询等待条件成立（最多 5s）。
    async fn wait_until<F: Fn() -> bool>(cond: F) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(tokio::time::Instant::now() < deadline, "等待条件超时（5s）");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// 记录 wake_session 调用（其余方法为最小桩）的假 Agent。
    #[derive(Clone, Default)]
    struct RecordingAgent {
        woken: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl AgentCoordinator for RecordingAgent {
        async fn process_message(
            &self,
            _session_id: &str,
            _message: &Message,
            _model: Option<&str>,
            _thinking_effort: Option<String>,
        ) -> tianyan::Result<AgentResponse> {
            Err(tianyan::TianyanError::Custom("测试桩".to_string()))
        }
        async fn process_message_stream(
            &self,
            _session_id: &str,
            _message: &Message,
            _model: Option<&str>,
            _cancel: Option<Arc<AtomicBool>>,
            _thinking_effort: Option<String>,
        ) -> tianyan::Result<tokio::sync::mpsc::Receiver<tianyan::Result<AgentStreamChunk>>>
        {
            Err(tianyan::TianyanError::Custom("测试桩".to_string()))
        }
        async fn handle_clarification(
            &self,
            _session_id: &str,
            _answers: &str,
        ) -> tianyan::Result<AgentResponse> {
            Err(tianyan::TianyanError::Custom("测试桩".to_string()))
        }
        async fn approval_status(&self) -> tianyan::Result<ApprovalStatusSnapshot> {
            Err(tianyan::TianyanError::Custom("测试桩".to_string()))
        }
        async fn respond_approval(
            &self,
            _request_id: &str,
            _decision: ApprovalDecision,
            _reason: Option<String>,
            _edited_command: Option<String>,
        ) -> tianyan::Result<()> {
            Err(tianyan::TianyanError::Custom("测试桩".to_string()))
        }
        async fn get_state(&self) -> AgentState {
            AgentState::default()
        }
        async fn background_tasks(&self) -> Vec<BackgroundTask> {
            Vec::new()
        }
        async fn cancel_background_task(&self, _task_id: &str) -> tianyan::Result<bool> {
            Ok(false)
        }
        async fn compress_session(&self, _session_id: &str) -> tianyan::Result<bool> {
            Ok(false)
        }
        async fn wake_session(&self, session_id: &str) {
            self.woken
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(session_id.to_string());
        }
    }

    /// 返回预置会话列表的内存会话管理器（其余方法为最小桩）。
    struct FakeSessionManager {
        sessions: Vec<Session>,
    }

    #[async_trait]
    impl SessionManager for FakeSessionManager {
        async fn create_session(&self, id: &str, _message: Message) -> tianyan::Result<Session> {
            Ok(Session::new(id))
        }
        async fn get_session(&self, _id: &str) -> tianyan::Result<Option<Session>> {
            Ok(None)
        }
        async fn update_session(&self, _session: &Session) -> tianyan::Result<()> {
            Ok(())
        }
        async fn add_structured_message(
            &self,
            _session_id: &str,
            _msg: StructuredMessage,
        ) -> tianyan::Result<()> {
            Ok(())
        }
        async fn rewrite_messages(
            &self,
            _session_id: &str,
            _messages: &[StructuredMessage],
        ) -> tianyan::Result<()> {
            Ok(())
        }
        async fn list_sessions(&self) -> tianyan::Result<Vec<Session>> {
            Ok(self.sessions.clone())
        }
        async fn delete_session(&self, _id: &str) -> tianyan::Result<()> {
            Ok(())
        }
    }

    /// 记录触发调用（可配置是否命中）的假任务触发器。
    #[derive(Clone, Default)]
    struct RecordingTrigger {
        calls: Arc<Mutex<Vec<String>>>,
        found: Arc<Mutex<bool>>,
    }

    #[async_trait]
    impl TaskTrigger for RecordingTrigger {
        async fn trigger_task(&self, task_id_or_name: &str) -> bool {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(task_id_or_name.to_string());
            *self.found.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    fn test_rules() -> Vec<EventRule> {
        vec![
            EventRule {
                pattern: "webhook:ping".to_string(),
                action: EventAction::Wake {
                    session_prefix: "session_".to_string(),
                },
            },
            EventRule {
                pattern: "*.rs".to_string(),
                action: EventAction::Task {
                    name: "fmt".to_string(),
                },
            },
        ]
    }

    #[tokio::test]
    async fn test_wake_action_wakes_matching_sessions() {
        let bus = EventBus::new();
        let agent = RecordingAgent::default();
        let deps = ProcessorDeps::new(
            Arc::new(agent.clone()),
            Arc::new(FakeSessionManager {
                sessions: vec![
                    Session::new("session_alpha"),
                    Session::new("session_beta"),
                    Session::new("other"),
                ],
            }),
        );
        let _handle = EventProcessor::start(bus.clone(), test_rules(), deps);

        bus.publish(Event::Webhook {
            name: "ping".to_string(),
            payload: serde_json::json!({}),
        });

        let agent_clone = agent.clone();
        wait_until(move || {
            agent_clone
                .woken
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len()
                == 2
        })
        .await;
        let woken = agent
            .woken
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert_eq!(
            woken,
            vec!["session_alpha".to_string(), "session_beta".to_string()]
        );
    }

    #[tokio::test]
    async fn test_wake_action_without_matching_session() {
        let bus = EventBus::new();
        let agent = RecordingAgent::default();
        let deps = ProcessorDeps::new(
            Arc::new(agent.clone()),
            Arc::new(FakeSessionManager {
                sessions: vec![Session::new("other_only")],
            }),
        );
        let _handle = EventProcessor::start(bus.clone(), test_rules(), deps);

        bus.publish(Event::Webhook {
            name: "ping".to_string(),
            payload: serde_json::json!({}),
        });

        // 无匹配会话：短暂等待后确认没有唤醒调用
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            agent
                .woken
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "无匹配会话时不应唤醒"
        );
    }

    #[tokio::test]
    async fn test_task_action_uses_trigger() {
        let bus = EventBus::new();
        let trigger = RecordingTrigger::default();
        *trigger.found.lock().unwrap_or_else(|e| e.into_inner()) = true;
        let deps = ProcessorDeps::new(
            Arc::new(RecordingAgent::default()),
            Arc::new(FakeSessionManager { sessions: vec![] }),
        )
        .with_task_trigger(Arc::new(trigger.clone()));
        let _handle = EventProcessor::start(bus.clone(), test_rules(), deps);

        bus.publish(Event::FileCreated {
            path: PathBuf::from("src/main.rs"),
        });

        let trigger_clone = trigger.clone();
        wait_until(move || {
            !trigger_clone
                .calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
        })
        .await;
        let calls = trigger
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert_eq!(calls, vec!["fmt".to_string()]);
    }

    #[tokio::test]
    async fn test_rule_matching_is_scoped() {
        let bus = EventBus::new();
        let agent = RecordingAgent::default();
        let trigger = RecordingTrigger::default();
        let deps = ProcessorDeps::new(
            Arc::new(agent.clone()),
            Arc::new(FakeSessionManager {
                sessions: vec![Session::new("session_a")],
            }),
        )
        .with_task_trigger(Arc::new(trigger.clone()));
        let _handle = EventProcessor::start(bus.clone(), test_rules(), deps);

        // 文件事件：只命中 `*.rs => task:fmt`（不触发 webhook wake 规则）
        bus.publish(Event::FileCreated {
            path: PathBuf::from("a.rs"),
        });
        // webhook 事件（名称不匹配 ping）：两条规则都不命中
        bus.publish(Event::Webhook {
            name: "other".to_string(),
            payload: serde_json::json!({}),
        });

        let trigger_clone = trigger.clone();
        wait_until(move || {
            !trigger_clone
                .calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
        })
        .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let calls = trigger
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert_eq!(calls, vec!["fmt".to_string()], "webhook 名不匹配不应触发");
        assert!(
            agent
                .woken
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "文件事件不应触发 wake 规则"
        );
    }
}

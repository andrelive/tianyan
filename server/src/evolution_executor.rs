//! 演化综述执行器（ADR-017：server 侧实现）。
//!
//! 在专用演化会话中运行主 agent 一轮：消息 = 综述指令 + 任务采集的输入包。
//! 主 agent 的工具集不含 VFS 注册表写入工具（记忆/技能/角色只能经 core 服务
//! 写入），因此其回复只能是 diff 计划文本——提议/提交分离由架构强制。

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use tianyan::agent::AgentCoordinator;
use tianyan::common::error::TianyanError;
use tianyan::common::types::Message;
use tianyan::scheduler::tasks::EvolutionReviewExecutor;

/// 基于主 agent 的演化综述执行器。
pub struct AgentEvolutionExecutor {
    agent: Arc<RwLock<Arc<dyn AgentCoordinator>>>,
    /// 会话工作集注册表（ADR-035 §3：演化会话被 agent 处理过 → 缓存已建，
    /// 清理必须经工作集（ADR-039：写路径唯一入口），旁路直删会残留缓存）。
    working_sets: Arc<tianyan::agent::working_set::WorkingSetRegistry>,
    model: String,
    review_role: String,
}

impl AgentEvolutionExecutor {
    /// 创建执行器。
    ///
    /// agent 为共享句柄（配置热重载后自动指向新实例）；review_role 为
    /// 综述角色名（evolution_reviewer，供提示词引用）。
    pub fn new(
        agent: Arc<RwLock<Arc<dyn AgentCoordinator>>>,
        working_sets: Arc<tianyan::agent::working_set::WorkingSetRegistry>,
        model: String,
        review_role: String,
    ) -> Self {
        Self {
            agent,
            working_sets,
            model,
            review_role,
        }
    }
}

#[async_trait]
impl EvolutionReviewExecutor for AgentEvolutionExecutor {
    async fn review(&self, input: &str) -> Result<String, TianyanError> {
        let session_id = format!("evolution-{}", chrono::Utc::now().timestamp());
        // 拼接角色指令 + 输入包（input 含字面花括号，不能用 format! 包裹）
        let mut text = String::new();
        text.push_str("【自演化综述】请以演化综述员（");
        text.push_str(&self.review_role);
        text.push_str(
            "）身份执行本次任务。你的回复将被程序解析：只输出要求的 JSON，不要输出其他内容。",
        );
        text.push('\n');
        text.push_str(input);

        let msg = Message::user(text);
        let agent = self.agent.read().await.clone();
        let resp = agent
            .process_message(&session_id, &msg, Some(&self.model), None)
            .await;

        // 清理专用会话（避免进入会话列表与记忆提取）——成功/失败都清理：
        // 失败路径此前 `?` 提前返回跳过清理，专用会话残留进前端会话列表。
        //
        // ADR-035 §3 收口 ④：该会话刚经 `agent.process_message` 处理（工作集已
        // 缓存），清理必须经工作集——旁路直删会留下已删会话的缓存条目。
        // ADR-039 §4：唯一写入口（清理失败仅 debug——专用会话不阻塞演化结果）
        if let Ok(ws) = self.working_sets.ensure(&session_id).await {
            if let Err(e) = ws.delete().await {
                tracing::debug!(session_id = %session_id, error = %e, "演化会话清理失败（工作集路径）");
            }
        }
        self.working_sets.remove(&session_id).await;
        resp.map(|r| r.content)
    }
}

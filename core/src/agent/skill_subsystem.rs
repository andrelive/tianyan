//! Agent 的技能子系统。
//!
//! 封装技能执行器、注册表和学习引擎，
//! 三者协同构成完整的技能生命周期管理。

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::skills::{SkillExecutor, SkillLearningEngine, SkillRegistry};

/// Agent 的技能子系统。
#[derive(Clone)]
pub struct AgentSkills {
    /// 技能执行器（参数验证、安全检查、超时控制）。
    pub executor: Option<Arc<SkillExecutor>>,
    /// 技能注册表。
    pub registry: Arc<RwLock<SkillRegistry>>,
    /// 技能学习引擎（从执行历史自动生成技能）。
    pub learning_engine: Option<SkillLearningEngine>,
}

impl AgentSkills {
    /// 创建新的技能子系统。
    pub fn new(
        executor: Option<Arc<SkillExecutor>>,
        registry: Arc<RwLock<SkillRegistry>>,
        learning_engine: Option<SkillLearningEngine>,
    ) -> Self {
        Self {
            executor,
            registry,
            learning_engine,
        }
    }
}

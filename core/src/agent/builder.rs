use std::sync::Arc;

use tokio::sync::Mutex as TokioMutex;

use tokio::sync::RwLock;

use crate::agent::r#loop::{AgentLoop, AgentLoopConfig};
use crate::agent::tool_registry::ToolRegistry;
use crate::common::error::{Result, TianyanError};
use crate::config::AgentConfig;
use crate::context::compression::{CompressionConfig, ContextCompressor};
use crate::context::pipeline::ContextPipeline;
use crate::context::DualLayerRetriever;
use crate::executor::SecurityPolicy;
use crate::model::ChatService;
use crate::observability::AgentMetrics;
use crate::session::SessionManager;
use crate::skills::learning::{SkillLearningConfig, SkillLearningEngine};
use crate::skills::{SkillExecutor, SkillRegistry};
use crate::vfs::VirtualFileSystem;

use super::coordinator::Agent;

/// 用于创建智能体的构建器。
pub struct AgentBuilder {
    config: AgentConfig,
    model_service: Option<Arc<dyn ChatService>>,
    retriever: Option<Arc<DualLayerRetriever>>,
    vfs: Option<Arc<dyn VirtualFileSystem>>,
    skill_executor: Option<Arc<SkillExecutor>>,
    skill_registry: Option<Arc<RwLock<SkillRegistry>>>,
    session_manager: Option<Arc<dyn SessionManager>>,
}

impl AgentBuilder {
    pub fn new() -> Self {
        Self {
            config: AgentConfig::default(),
            model_service: None,
            retriever: None,
            vfs: None,
            skill_executor: None,
            skill_registry: None,
            session_manager: None,
        }
    }

    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_model_service(mut self, service: Arc<dyn ChatService>) -> Self {
        self.model_service = Some(service);
        self
    }

    pub fn with_retriever(mut self, retriever: Arc<DualLayerRetriever>) -> Self {
        self.retriever = Some(retriever);
        self
    }

    pub fn with_vfs(mut self, vfs: Arc<dyn VirtualFileSystem>) -> Self {
        self.vfs = Some(vfs);
        self
    }

    pub fn with_skill_executor(mut self, executor: Arc<SkillExecutor>) -> Self {
        self.skill_executor = Some(executor);
        self
    }

    pub fn with_skill_registry(mut self, registry: Arc<RwLock<SkillRegistry>>) -> Self {
        self.skill_registry = Some(registry);
        self
    }

    pub fn with_session_manager(mut self, session_manager: Arc<dyn SessionManager>) -> Self {
        self.session_manager = Some(session_manager);
        self
    }

    pub fn build(self) -> Result<Agent> {
        let model_service = self
            .model_service
            .ok_or_else(|| TianyanError::Internal("需要模型服务".to_string()))?;

        let retriever = self
            .retriever
            .ok_or_else(|| TianyanError::Internal("需要检索器".to_string()))?;

        let vfs = self
            .vfs
            .ok_or_else(|| TianyanError::Internal("需要虚拟文件系统".to_string()))?;

        let skill_registry = self
            .skill_registry
            .unwrap_or_else(|| Arc::new(RwLock::new(SkillRegistry::new())));

        let session_manager = self
            .session_manager
            .ok_or_else(|| TianyanError::Internal("需要会话管理器".to_string()))?;

        // 构建可观测性指标（tool_registry 依赖）
        let metrics = AgentMetrics::new();

        let tool_registry = ToolRegistry::new(SecurityPolicy::default())
            .with_vfs(vfs.clone())
            .with_model_service(model_service.clone())
            .with_metrics(metrics.clone());
        let tool_registry = if let Some(ref executor) = self.skill_executor {
            tool_registry.with_skill_executor(executor.clone())
        } else {
            tool_registry
        };

        let agent_loop = AgentLoop::new(
            model_service.clone(),
            tool_registry,
            session_manager.clone(),
            AgentLoopConfig {
                max_turns: self.config.max_turns,
                model: "default".to_string(),
            },
        );

        // 构建上下文管线
        let context_pipeline = ContextPipeline::new(
            vfs.clone(),
            retriever.clone(),
            Arc::new(TokioMutex::new(ContextCompressor::new(
                model_service.clone(),
                CompressionConfig {
                    preserve_recent_messages: 6,
                    ..CompressionConfig::default()
                },
            ))),
            self.config.default_top_k,
            self.config.learned_rules_top_k,
        );

        // 构建技能学习引擎
        let skill_learning_engine = if self.config.enable_skills {
            Some(SkillLearningEngine::new(
                model_service.clone(),
                vfs.clone(),
                SkillLearningConfig::default(),
            ))
        } else {
            None
        };

        Ok(Agent::new(
            self.config,
            model_service,
            vfs,
            context_pipeline,
            metrics,
            self.skill_executor,
            skill_registry,
            skill_learning_engine,
            agent_loop,
            session_manager,
        ))
    }
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_builder() {
        let builder = AgentBuilder::new();
        assert!(builder.config.enable_skills);
        assert!(builder.config.enable_memory);
    }
}

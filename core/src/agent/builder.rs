use std::sync::Arc;

use tokio::sync::Mutex as TokioMutex;

use tokio::sync::RwLock;

use crate::agent::harness::AgentHarness;
use crate::agent::r#loop::{AgentLoop, AgentLoopConfig};
use crate::agent::skill_subsystem::AgentSkills;
use crate::agent::tool_registry::ToolRegistry;
use crate::common::error::{Result, TianyanError};
use crate::config::AgentConfig;
use crate::context::compression::{CompressionConfig, ContextCompressor};
use crate::context::pipeline::ContextPipeline;
use crate::context::DualLayerRetriever;
use crate::executor::verification::VerificationGate;
use crate::executor::SecurityPolicy;
use crate::model::ChatService;
use crate::observability::AgentMetrics;
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

        let tool_registry = ToolRegistry::new(SecurityPolicy::default());
        let tool_registry = if let Some(ref executor) = self.skill_executor {
            tool_registry.with_skill_executor(executor.clone())
        } else {
            tool_registry
        };

        let agent_loop = AgentLoop::new(
            model_service.clone(),
            tool_registry,
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

        // 构建可观测性指标
        let metrics = AgentMetrics::new();

        // 组装 Harness 子系统
        let harness = AgentHarness::new(metrics);

        // 构建技能学习引擎
        let learning_engine = if self.config.enable_skills {
            Some(SkillLearningEngine::new(
                model_service.clone(),
                vfs.clone(),
                SkillLearningConfig::default(),
            ))
        } else {
            None
        };

        // 组装技能子系统
        let skills = AgentSkills::new(self.skill_executor, skill_registry, learning_engine);

        // 构建验证门控
        let verification_gate =
            VerificationGate::new(".").with_enabled(self.config.enable_verification);

        // LLM-as-Judge 默认关闭，需要时通过 AgentConfig 显式创建
        let llm_judge = None;

        Ok(Agent::new(
            self.config,
            model_service,
            vfs,
            context_pipeline,
            harness,
            skills,
            verification_gate,
            llm_judge,
            agent_loop,
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

use serde::{Deserialize, Serialize};

/// Planner 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannerConfig {
    /// 最大迭代次数（防止无限循环）
    pub max_iterations: usize,

    /// 最大递归深度
    pub max_depth: usize,

    /// 上下文管理配置
    pub context_config: ContextConfig,

    /// Executor 最大并发数
    pub executor_max_concurrency: usize,

    /// 最大重规划次数（执行失败后尝试重新规划的次数）
    pub max_recovery_attempts: usize,

    /// 是否启用自适应重规划
    pub enable_recovery: bool,
}

/// 上下文配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    /// 年龄衰减因子
    pub age_decay_factor: f32,
    /// 清理阈值
    pub cleanup_threshold: f32,
    /// 最大上下文 token 数（软限制）
    pub max_context_tokens: usize,
}

impl PlannerConfig {
    /// 验证配置。
    pub fn validate(&self) -> Result<(), String> {
        if self.max_iterations == 0 {
            return Err("max_iterations 必须大于 0".to_string());
        }
        if self.max_iterations > 100 {
            return Err("max_iterations 不能超过 100".to_string());
        }
        if self.max_depth == 0 {
            return Err("max_depth 必须大于 0".to_string());
        }
        if self.executor_max_concurrency == 0 {
            return Err("executor_max_concurrency 必须大于 0".to_string());
        }
        Ok(())
    }
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            max_depth: 5,
            context_config: ContextConfig::default(),
            executor_max_concurrency: 10,
            max_recovery_attempts: 3,
            enable_recovery: true,
        }
    }
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            age_decay_factor: 0.9,
            cleanup_threshold: 0.3,
            max_context_tokens: 8000,
        }
    }
}

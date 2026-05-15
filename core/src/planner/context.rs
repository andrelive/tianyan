use crate::planner::types::{Plan, StepResult, Turn};

/// 上下文管理器
#[derive(Debug, Clone)]
pub struct ContextManager {
    /// 所有历史轮次
    turns: Vec<Turn>,

    /// 年龄衰减因子
    age_decay_factor: f32,

    /// 清理阈值
    cleanup_threshold: f32,
}

impl ContextManager {
    /// 创建新的 ContextManager
    pub fn new(age_decay_factor: f32, cleanup_threshold: f32) -> Self {
        Self {
            turns: Vec::new(),
            age_decay_factor,
            cleanup_threshold,
        }
    }

    /// 计算某轮次中某步骤的权重
    fn calculate_weight(&self, turn_id: usize, step: &StepResult) -> f32 {
        let age = self.turns.len().saturating_sub(1).saturating_sub(turn_id);
        let importance = step.actual_importance.unwrap_or(0.5);

        importance * self.age_decay_factor.powi(age as i32)
    }

    /// 构建给 LLM 的上下文
    pub fn build_context(&self) -> Vec<Turn> {
        self.turns
            .iter()
            .enumerate()
            .filter(|(idx, _turn)| {
                // 最新轮次始终保留
                if *idx == self.turns.len().saturating_sub(1) {
                    return true;
                }

                // 检查该轮次是否有步骤的权重超过阈值
                self.turns[*idx]
                    .results
                    .iter()
                    .any(|result| self.calculate_weight(*idx, result) >= self.cleanup_threshold)
            })
            .map(|(_, turn)| turn.clone())
            .collect()
    }

    /// 添加新轮次
    pub fn add_turn(&mut self, plan: Plan, results: Vec<StepResult>) {
        self.turns.push(Turn {
            turn_id: self.turns.len(),
            plan,
            results,
            timestamp: std::time::Instant::now(),
        });
    }

    /// 获取所有轮次（用于调试）
    pub fn get_turns(&self) -> &[Turn] {
        &self.turns
    }

    /// 提高清理阈值（当上下文过大时调用）
    pub fn increase_cleanup_threshold(&mut self) {
        // 每次提高 0.1，最大不超过 0.8
        self.cleanup_threshold = (self.cleanup_threshold + 0.1).min(0.8);

        // 记录日志
        tracing::info!("上下文清理阈值提高到: {}", self.cleanup_threshold);
    }
}

impl Default for ContextManager {
    fn default() -> Self {
        Self {
            turns: Vec::new(),
            age_decay_factor: 0.9,
            cleanup_threshold: 0.3,
        }
    }
}

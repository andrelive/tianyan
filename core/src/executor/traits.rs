use async_trait::async_trait;

use super::types::{Step, StepResult};

/// 步骤执行器 trait。
///
/// 定义执行 Planner 生成的步骤的契约。
/// 纯机械执行，无思考能力。
#[async_trait]
pub trait ExecutorTrait: Send + Sync {
    /// 并行执行多个步骤。
    ///
    /// - `steps` - 要执行的步骤列表（不应包含 SubPlanner 动作）
    /// - returns: 按 step_id 排序的执行结果列表
    async fn execute_steps(&self, steps: Vec<Step>) -> Vec<StepResult>;
}

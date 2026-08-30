//! 执行历史记录（Harness Engineering：环境可读性）。
//!
//! **独立类型层**：observability（执行日志）记录、skills（GEPA 学习）消费——
//! 纯类型不依赖任何领域模块，observability 无需依赖 skills（反之）。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 执行历史记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionHistory {
    /// 任务描述。
    pub task_description: String,
    /// 执行步骤。
    pub steps: Vec<ExecutionStep>,
    /// 执行结果。
    pub result: String,
    /// 是否成功。
    pub success: bool,
    /// 执行耗时（毫秒）。
    pub execution_time_ms: u64,
    /// 使用的技能列表。
    pub skills_used: Vec<String>,
}

/// 执行步骤。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStep {
    /// 步骤描述。
    pub description: String,
    /// 动作类型。
    pub action: String,
    /// 参数。
    pub parameters: HashMap<String, String>,
    /// 执行结果。
    pub result: String,
}

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

/// 生成的技能。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedSkill {
    /// 技能 ID。
    pub id: String,
    /// 技能名称。
    pub name: String,
    /// 技能描述。
    pub description: String,
    /// 技能内容。
    pub content: String,
    /// 适用场景。
    pub applicable_scenarios: Vec<String>,
    /// 参数列表。
    pub parameters: Vec<SkillParameter>,
    /// 版本号。
    pub version: String,
    /// 来源历史 ID 列表。
    pub source_history: Vec<String>,
}

/// 技能参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillParameter {
    /// 参数名称。
    pub name: String,
    /// 参数类型。
    pub param_type: String,
    /// 参数描述。
    pub description: String,
    /// 是否必填。
    pub required: bool,
    /// 默认值。
    pub default_value: Option<String>,
}

/// 技能评估结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEvaluation {
    /// 技能 ID。
    pub skill_id: String,
    /// 总使用次数。
    pub total_uses: usize,
    /// 成功次数。
    pub successful_uses: usize,
    /// 成功率。
    pub success_rate: f32,
    /// 建议操作。
    pub recommended_action: SkillAction,
}

/// 技能操作建议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillAction {
    /// 保留。
    Keep,
    /// 改进。
    Improve,
    /// 废弃。
    Deprecate,
}

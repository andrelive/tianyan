use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionHistory {
    pub task_description: String,
    pub steps: Vec<ExecutionStep>,
    pub result: String,
    pub success: bool,
    pub execution_time_ms: u64,
    pub skills_used: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStep {
    pub description: String,
    pub action: String,
    pub parameters: HashMap<String, String>,
    pub result: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub applicable_scenarios: Vec<String>,
    pub parameters: Vec<SkillParameter>,
    pub version: String,
    pub source_history: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillParameter {
    pub name: String,
    pub param_type: String,
    pub description: String,
    pub required: bool,
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEvaluation {
    pub skill_id: String,
    pub total_uses: usize,
    pub successful_uses: usize,
    pub success_rate: f32,
    pub recommended_action: SkillAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillAction {
    Keep,
    Improve,
    Deprecate,
}

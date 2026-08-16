//! 计划阶段技能（软约束：对话语义驱动的只读规划）。
//!
//! 用户说"计划一下/做个方案/先别动手"时，模型调用 `call_skill("planning")`——
//! 本处理器**无副作用**，仅返回行为指南文本作为工具结果注入对话：
//! 模型读到指南后按"只读研究 → 结构化计划 → 询问是否执行"行动。
//! 指南文本留在对话历史中，对后续轮次持续可见（软约束跨轮生效）。
//!
//! 与显式 Plan 模式（UI 模式切换 + 写工具硬阻断）的取舍见 REJECTED.md：
//! 单用户场景下显式切换是额外状态机负担，软约束 + 审批流 + 快照回退已足够。

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;

use crate::common::error::Result;
use crate::skills::definition::SkillHandler;
use crate::skills::types::{ExecutionContext, SkillExecutionResult};

/// 计划阶段行为指南（注入对话，模型据此行动）。
const PLANNING_GUIDANCE: &str = "\
你已进入计划阶段（用户要求先规划再执行）。遵守以下约束：

1. **只读研究**：只使用只读工具收集信息（read_file / grep / search_knowledge /
   vfs_read / vfs_list / glob / list_dir / symbol_outline / lsp / web_search / web_fetch /
   discover_tests）。不得调用 write_file / apply_edit / apply_patch / execute_command /
   run_tests / verify_build / knowledge_ingest / delegate_to_agent（后台任务）。
2. **输出结构化计划**：按以下格式输出——
   - 目标：一句话明确要完成什么
   - 步骤：编号列表，每步注明涉及的文件/命令/风险
   - 风险与验证：潜在副作用 + 每步完成后的验证方式（测试/检查命令，仅描述不执行）
3. **结束询问**：计划输出完毕后，询问用户是否开始执行——用户确认前不执行任何写操作。
4. 用户确认执行后，恢复正常（执行）行为。";

/// 计划阶段处理器：无副作用，返回行为指南。
pub struct PlanningHandler;

impl PlanningHandler {
    /// 创建处理器。
    pub fn new() -> Self {
        Self
    }
}

impl Default for PlanningHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SkillHandler for PlanningHandler {
    async fn execute(
        &self,
        params: HashMap<String, Value>,
        _context: ExecutionContext,
    ) -> Result<SkillExecutionResult> {
        let goal = params
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let mut output = String::from("【计划阶段已激活】\n");
        output.push_str(PLANNING_GUIDANCE);
        if !goal.is_empty() {
            output.push_str(&format!("\n\n本次规划目标：{goal}"));
        }
        Ok(SkillExecutionResult::success(output))
    }

    fn skill_id(&self) -> &'static str {
        "planning"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_planning_returns_guidance() {
        let handler = PlanningHandler::new();
        let mut params = HashMap::new();
        params.insert(
            "goal".to_string(),
            Value::String("重构 auth 模块".to_string()),
        );
        let result = handler
            .execute(params, ExecutionContext::default())
            .await
            .expect("规划技能应成功");
        assert!(result.success);
        let output = result.output.unwrap_or_default();
        assert!(output.contains("计划阶段"), "应包含阶段标记");
        assert!(output.contains("只读研究"), "应包含只读约束");
        assert!(output.contains("write_file"), "应点名禁用工具");
        assert!(output.contains("重构 auth 模块"), "应携带目标");
        assert!(output.contains("是否开始执行"), "应包含结束询问");
    }

    #[tokio::test]
    async fn test_planning_without_goal() {
        let handler = PlanningHandler::new();
        let result = handler
            .execute(HashMap::new(), ExecutionContext::default())
            .await
            .expect("无目标也应成功");
        assert!(result.success);
        let output = result.output.unwrap_or_default();
        assert!(output.contains("计划阶段"), "应包含阶段标记");
    }
}

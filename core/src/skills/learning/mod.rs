mod generator;
mod types;

use std::collections::HashMap;
use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{ContextNamespace, TianyanUri};
use crate::model::ChatService;
use crate::vfs::VirtualFileSystem;

use generator::{build_skill_generation_prompt, parse_generated_skill};
pub use types::{
    ExecutionHistory, ExecutionStep, GeneratedSkill, SkillAction, SkillEvaluation, SkillParameter,
};

/// 技能学习配置。
#[derive(Debug, Clone)]
pub struct SkillLearningConfig {
    /// 是否启用自动学习。
    pub enable_auto_learning: bool,
    /// 成功阈值。
    pub success_threshold: usize,
    /// 最小历史长度。
    pub min_history_length: usize,
    /// 生成模型名称。
    pub generation_model: String,
    /// 技能存储前缀。
    pub skill_storage_prefix: String,
}

impl Default for SkillLearningConfig {
    fn default() -> Self {
        Self {
            enable_auto_learning: true,
            success_threshold: 2,
            min_history_length: 3,
            generation_model: String::new(),
            skill_storage_prefix: "skill/learned".to_string(),
        }
    }
}

/// GEPA 技能进化引擎。
#[derive(Clone)]
pub struct SkillLearningEngine {
    model_service: Arc<dyn ChatService>,
    vfs: Arc<dyn VirtualFileSystem>,
    config: SkillLearningConfig,
}

impl SkillLearningEngine {
    /// 创建新的技能学习引擎。
    pub fn new(
        model_service: Arc<dyn ChatService>,
        vfs: Arc<dyn VirtualFileSystem>,
        config: SkillLearningConfig,
    ) -> Self {
        Self {
            model_service,
            vfs,
            config,
        }
    }

    /// 从历史执行记录中学习生成新技能。
    pub async fn learn_from_history(
        &self,
        history: &[ExecutionHistory],
    ) -> Result<Vec<GeneratedSkill>> {
        if !self.config.enable_auto_learning || history.len() < self.config.min_history_length {
            return Ok(Vec::new());
        }

        let successful: Vec<_> = history.iter().filter(|h| h.success).collect();
        if successful.len() < self.config.success_threshold {
            return Ok(Vec::new());
        }

        let mut grouped: HashMap<String, Vec<&ExecutionHistory>> = HashMap::new();
        for exec in &successful {
            let key = self.categorize_task(&exec.task_description).await;
            grouped.entry(key).or_default().push(exec);
        }

        let mut generated_skills = Vec::new();

        for (task_type, executions) in grouped {
            if executions.len() >= self.config.success_threshold {
                match self.generate_skill(&task_type, &executions).await {
                    Ok(skill) => {
                        if let Err(e) = self.store_skill(&skill).await {
                            tracing::warn!(skill_id = %skill.id, error = %e, "存储技能失败");
                        } else {
                            generated_skills.push(skill);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(task_type = %task_type, error = %e, "生成技能失败");
                    }
                }
            }
        }

        tracing::info!(count = generated_skills.len(), "技能学习完成");
        Ok(generated_skills)
    }

    /// 使用 LLM 将任务描述分类到已知操作类型。
    ///
    /// 分类失败时回退到关键词匹配。
    async fn categorize_task(&self, description: &str) -> String {
        if let Ok(category) = self.categorize_via_llm(description).await {
            return category;
        }
        // LLM 不可用时回退到关键词匹配
        Self::categorize_by_keyword(description)
    }

    /// 通过 LLM 对任务描述进行分类。
    async fn categorize_via_llm(&self, description: &str) -> Result<String> {
        let prompt = format!(
            "将以下任务描述分类为以下类别之一：\n\
             - file_operation（文件操作）\n\
             - code_operation（代码操作）\n\
             - search_operation（搜索操作）\n\
             - test_operation（测试操作）\n\
             - deploy_operation（部署操作）\n\
             - analysis_operation（分析操作）\n\
             - general_operation（通用操作）\n\n\
             任务描述：{description}\n\n\
             只返回类别名称，不要解释。"
        );

        let response = self
            .model_service
            .chat(
                &self.config.generation_model,
                vec![crate::common::types::Message::user(prompt)],
            )
            .await?;

        let category = response.trim().to_lowercase();
        let valid = [
            "file_operation", "code_operation", "search_operation",
            "test_operation", "deploy_operation", "analysis_operation",
            "general_operation",
        ];
        if valid.contains(&category.as_str()) {
            Ok(category)
        } else {
            // LLM 返回了无效类别，回退到关键词
            Ok(Self::categorize_by_keyword(description))
        }
    }

    /// 基于关键词的任务分类（LLM 不可用时的回退）。
    fn categorize_by_keyword(description: &str) -> String {
        let lower = description.to_lowercase();

        if lower.contains("文件")
            || lower.contains("file")
            || lower.contains("目录")
            || lower.contains("folder")
        {
            "file_operation".to_string()
        } else if lower.contains("代码")
            || lower.contains("code")
            || lower.contains("编程")
            || lower.contains("programming")
        {
            "code_operation".to_string()
        } else if lower.contains("搜索")
            || lower.contains("search")
            || lower.contains("查找")
            || lower.contains("find")
        {
            "search_operation".to_string()
        } else if lower.contains("测试")
            || lower.contains("test")
            || lower.contains("验证")
            || lower.contains("verify")
        {
            "test_operation".to_string()
        } else if lower.contains("部署")
            || lower.contains("deploy")
            || lower.contains("发布")
            || lower.contains("release")
        {
            "deploy_operation".to_string()
        } else if lower.contains("分析")
            || lower.contains("analyze")
            || lower.contains("检查")
            || lower.contains("check")
        {
            "analysis_operation".to_string()
        } else {
            "general_operation".to_string()
        }
    }

    async fn generate_skill(
        &self,
        task_type: &str,
        executions: &[&ExecutionHistory],
    ) -> Result<GeneratedSkill> {
        let prompt = build_skill_generation_prompt(task_type, executions);

        let response = self
            .model_service
            .chat(
                &self.config.generation_model,
                vec![crate::common::types::Message::user(prompt)],
            )
            .await?;

        parse_generated_skill(&response, task_type, executions)
    }

    async fn store_skill(&self, skill: &GeneratedSkill) -> Result<()> {
        let uri = TianyanUri::new(ContextNamespace::Skill, vec![skill.id.clone()]);

        if let Some(parent) = uri.parent() {
            if !self.vfs.exists(&parent).await? {
                self.vfs.create_directory(&parent).await?;
            }
        }

        let skill_md = self.format_skill_as_markdown(skill);
        self.vfs.write_content(&uri, &skill_md).await?;

        let abstract_content = format!(
            "{} | 适用场景: {}",
            skill.description,
            skill.applicable_scenarios.join(", ")
        );
        self.vfs.write_abstract(&uri, &abstract_content).await?;

        tracing::info!(skill_id = %skill.id, name = %skill.name, "技能已存储");
        Ok(())
    }

    fn format_skill_as_markdown(&self, skill: &GeneratedSkill) -> String {
        let mut md = String::new();

        md.push_str(&format!("# {}\n\n", skill.name));
        md.push_str(&format!("**ID**: `{}`\n\n", skill.id));
        md.push_str(&format!("**版本**: {}\n\n", skill.version));
        md.push_str(&format!("**描述**: {}\n\n", skill.description));

        if !skill.applicable_scenarios.is_empty() {
            md.push_str("## 适用场景\n\n");
            for scenario in &skill.applicable_scenarios {
                md.push_str(&format!("- {}\n", scenario));
            }
            md.push('\n');
        }

        if !skill.parameters.is_empty() {
            md.push_str("## 参数\n\n");
            md.push_str("| 参数 | 类型 | 必需 | 描述 | 默认值 |\n");
            md.push_str("|------|------|------|------|--------|\n");
            for param in &skill.parameters {
                let default = param.default_value.as_deref().unwrap_or("-");
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    param.name,
                    param.param_type,
                    if param.required { "是" } else { "否" },
                    param.description,
                    default
                ));
            }
            md.push('\n');
        }

        md.push_str("## 使用说明\n\n");
        md.push_str(&skill.content);
        md.push('\n');

        if !skill.source_history.is_empty() {
            md.push_str("\n## 来源\n\n");
            md.push_str("基于以下成功执行生成：\n\n");
            for source in &skill.source_history {
                md.push_str(&format!("- {}\n", source));
            }
        }

        md
    }

    /// 评估技能执行效果。
    pub async fn evaluate_skill(
        &self,
        skill_id: &str,
        execution_results: &[bool],
    ) -> Result<SkillEvaluation> {
        let total = execution_results.len();
        let successes = execution_results.iter().filter(|&&r| r).count();
        let success_rate = if total > 0 {
            successes as f32 / total as f32
        } else {
            0.0
        };

        let evaluation = SkillEvaluation {
            skill_id: skill_id.to_string(),
            total_uses: total,
            successful_uses: successes,
            success_rate,
            recommended_action: if success_rate < 0.5 {
                SkillAction::Deprecate
            } else if success_rate < 0.8 {
                SkillAction::Improve
            } else {
                SkillAction::Keep
            },
        };

        Ok(evaluation)
    }

    /// 应用技能评估结果，更新技能生命周期。
    ///
    /// - Deprecate: 禁用心能（写入 `enabled: false` 标记）
    /// - Improve: 提升版本号
    /// - Keep: 不操作
    pub async fn apply_evaluation(&self, evaluation: &SkillEvaluation) -> Result<()> {
        let uri = TianyanUri::new(ContextNamespace::Skill, vec![evaluation.skill_id.clone()]);

        if !self.vfs.exists(&uri).await? {
            return Ok(());
        }

        match evaluation.recommended_action {
            SkillAction::Deprecate => {
                let marker = format!(
                    "# 已弃用\n\n成功率: {:.1}% ({}/{})\n评估时间: {}\n",
                    evaluation.success_rate * 100.0,
                    evaluation.successful_uses,
                    evaluation.total_uses,
                    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S")
                );
                self.vfs.write_content(&uri, &marker).await?;
                tracing::info!(
                    skill_id = %evaluation.skill_id,
                    success_rate = %evaluation.success_rate,
                    "技能已弃用"
                );
            }
            SkillAction::Improve => {
                // 在技能内容末尾追加改进标记，供下次 GEPA 生成时参考
                let improvement_note = format!(
                    "\n\n<!-- GEPA_IMPROVE: success_rate={:.1}% ({}/{}) -->\n",
                    evaluation.success_rate * 100.0,
                    evaluation.successful_uses,
                    evaluation.total_uses
                );
                self.vfs.append_content(&uri, &improvement_note).await?;
                tracing::info!(
                    skill_id = %evaluation.skill_id,
                    success_rate = %evaluation.success_rate,
                    "技能已标记待改进"
                );
            }
            SkillAction::Keep => {
                tracing::debug!(
                    skill_id = %evaluation.skill_id,
                    success_rate = %evaluation.success_rate,
                    "技能保持活跃"
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_learning_config_default() {
        let config = SkillLearningConfig::default();
        assert!(config.enable_auto_learning);
        assert_eq!(config.success_threshold, 2);
    }

    #[tokio::test]
    async fn test_categorize_task() {
        let engine = SkillLearningEngine::new(
            mock_chat_empty(),
            Arc::new(MockVfs::new()),
            SkillLearningConfig::default(),
        );

        // mock LLM 返回无效类别 → 回退到关键词匹配
        assert_eq!(engine.categorize_task("读取文件内容").await, "file_operation");
        assert_eq!(engine.categorize_task("搜索代码中的函数").await, "code_operation");
        assert_eq!(engine.categorize_task("运行单元测试").await, "test_operation");
        assert_eq!(engine.categorize_task("分析项目结构").await, "analysis_operation");
    }

    #[tokio::test]
    async fn test_skill_evaluation() {
        let engine = SkillLearningEngine::new(
            mock_chat_empty(),
            Arc::new(MockVfs::new()),
            SkillLearningConfig::default(),
        );

        let eval = engine
            .evaluate_skill("test-skill", &[true, true, true, false])
            .await
            .unwrap();

        assert_eq!(eval.total_uses, 4);
        assert_eq!(eval.successful_uses, 3);
        assert_eq!(eval.success_rate, 0.75);
        assert_eq!(eval.recommended_action, SkillAction::Improve);
    }

    use crate::model::types::{ChatChoice, ChatCompletionResponse};
    use crate::model::ChatService;
    use crate::test_utils::MockChatService;

    /// 创建返回空 JSON 响应的 mock ChatService（用于学习引擎测试）。
    fn mock_chat_empty() -> Arc<dyn crate::model::ChatService> {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().returning(|_| {
            Ok(ChatCompletionResponse {
                id: "mock".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "mock".to_string(),
                choices: vec![ChatChoice {
                    index: 0,
                    message: crate::common::types::Message::assistant("{}".to_string()),
                    finish_reason: Some("stop".to_string()),
                }],
                usage: Default::default(),
            })
        });
        Arc::new(mock)
    }

    use crate::test_utils::MockVfs;
}

mod generator;
mod types;

use std::collections::HashMap;
use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{ContextNamespace, TianyanUri};
use crate::model::ChatService;
use crate::storage::VirtualFileSystem;

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
            generation_model: "gpt-4o".to_string(),
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
            let key = self.categorize_task(&exec.task_description);
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

    fn categorize_task(&self, description: &str) -> String {
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

    #[test]
    fn test_categorize_task() {
        let engine = SkillLearningEngine::new(
            Arc::new(MockChatService),
            Arc::new(MockVfs),
            SkillLearningConfig::default(),
        );

        assert_eq!(engine.categorize_task("读取文件内容"), "file_operation");
        assert_eq!(engine.categorize_task("搜索代码中的函数"), "code_operation");
        assert_eq!(engine.categorize_task("运行单元测试"), "test_operation");
        assert_eq!(engine.categorize_task("分析项目结构"), "analysis_operation");
    }

    #[test]
    fn test_skill_evaluation() {
        let engine = SkillLearningEngine::new(
            Arc::new(MockChatService),
            Arc::new(MockVfs),
            SkillLearningConfig::default(),
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        let eval = rt.block_on(async {
            engine
                .evaluate_skill("test-skill", &[true, true, true, false])
                .await
                .unwrap()
        });

        assert_eq!(eval.total_uses, 4);
        assert_eq!(eval.successful_uses, 3);
        assert_eq!(eval.success_rate, 0.75);
        assert_eq!(eval.recommended_action, SkillAction::Improve);
    }

    struct MockChatService;
    #[async_trait::async_trait]
    impl crate::model::ChatService for MockChatService {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<crate::common::types::Message>,
        ) -> crate::common::error::Result<String> {
            Ok("{}".to_string())
        }
        async fn chat_completion(
            &self,
            _request: crate::model::ChatCompletionRequest,
        ) -> crate::common::error::Result<crate::model::ChatCompletionResponse> {
            unimplemented!()
        }
        async fn chat_completion_stream(
            &self,
            _request: crate::model::ChatCompletionRequest,
        ) -> crate::common::error::Result<
            tokio::sync::mpsc::Receiver<
                crate::common::error::Result<crate::model::ChatCompletionChunk>,
            >,
        > {
            unimplemented!()
        }
    }

    use crate::common::error::Result;
    use crate::common::types::{ContentLevel, ContextNamespace, SearchResult, TianyanUri};
    use crate::storage::{
        ContentMetadata, ContentStore, ContextEntry, VfsCore, VfsMetadata, VfsSearch,
        VirtualFileSystem,
    };
    use std::collections::HashMap;

    struct MockVfs;

    #[async_trait::async_trait]
    impl VfsCore for MockVfs {
        async fn initialize(&self) -> Result<()> {
            Ok(())
        }
        async fn exists(&self, _uri: &TianyanUri) -> Result<bool> {
            Ok(false)
        }
        async fn get_entry(&self, _uri: &TianyanUri) -> Result<ContextEntry> {
            unimplemented!()
        }
        async fn create_directory(&self, _uri: &TianyanUri) -> Result<ContextEntry> {
            unimplemented!()
        }
        async fn create_file(&self, _uri: &TianyanUri) -> Result<ContextEntry> {
            unimplemented!()
        }
        async fn delete(&self, _uri: &TianyanUri) -> Result<()> {
            Ok(())
        }
        async fn list(&self, _uri: &TianyanUri) -> Result<Vec<ContextEntry>> {
            Ok(vec![])
        }
        async fn move_entry(
            &self,
            _source: &TianyanUri,
            _destination: &TianyanUri,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl ContentStore for MockVfs {
        async fn write(
            &self,
            _uri: &TianyanUri,
            _level: ContentLevel,
            _content: &str,
        ) -> Result<()> {
            Ok(())
        }
        async fn read(&self, _uri: &TianyanUri, _level: ContentLevel) -> Result<String> {
            Ok("".to_string())
        }
        async fn append(&self, _uri: &TianyanUri, _content: &str) -> Result<()> {
            Ok(())
        }
        async fn has_content(&self, _uri: &TianyanUri, _level: ContentLevel) -> Result<bool> {
            Ok(false)
        }
    }

    #[async_trait::async_trait]
    impl VfsSearch for MockVfs {
        async fn search(
            &self,
            _query: &str,
            _limit: usize,
            _namespace: Option<ContextNamespace>,
        ) -> Result<Vec<SearchResult>> {
            Ok(vec![])
        }
        async fn search_by_visual(
            &self,
            _visual_vector: &[f32],
            _top_k: usize,
        ) -> Result<Vec<SearchResult>> {
            Ok(vec![])
        }
        async fn update_summary_vectors(
            &self,
            _uri: &TianyanUri,
            _abstract_content: &str,
            _overview_content: &str,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl VfsMetadata for MockVfs {
        async fn update_metadata(
            &self,
            _uri: &TianyanUri,
            _importance: f32,
            _custom: HashMap<String, serde_json::Value>,
        ) -> Result<()> {
            Ok(())
        }
        async fn get_all_content_metadata(
            &self,
            _uri: &TianyanUri,
        ) -> Result<HashMap<ContentLevel, ContentMetadata>> {
            Ok(HashMap::new())
        }
    }

    impl VirtualFileSystem for MockVfs {}
}

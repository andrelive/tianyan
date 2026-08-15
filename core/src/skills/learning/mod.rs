mod generator;
mod types;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::common::error::Result;
use crate::common::llm_judge::parse_llm_json;
use crate::common::types::{ContextNamespace, TianyanUri};
use crate::model::ChatService;
use crate::vfs::VirtualFileSystem;

use generator::{build_skill_generation_prompt, parse_generated_skill};
pub use types::{
    ExecutionHistory, ExecutionStep, GeneratedSkill, SkillAction, SkillEvaluation, SkillParameter,
    SkillVerification,
};

/// 候选技能质量审查提示词（G2b）。
///
/// 输出 JSON：`{"score": 0-10, "issues": ["问题1", ...]}`；
/// `score >= 6` 为正式技能，`< 6` 标记为试验性。
pub const CANDIDATE_VERIFICATION_PROMPT: &str = r#"你是技能质量审查员。评估以下 GEPA 自动生成的技能是否值得作为正式技能注册。

技能名称：{name}
技能描述：{description}
适用场景：{scenarios}
技能内容：
{content}

评分标准（0-10 分）：
- 描述清晰、步骤完整、可执行（3 分）
- 与一般常识操作相比有明确复用价值，值得长期保存（4 分）
- 内容不空洞、不重复、无自相矛盾（3 分）

输出 JSON（不要输出其他内容）：
{"score": <0-10 整数>, "issues": ["问题1", "问题2"]}

score >= 6 为正式技能；score < 6 标记为试验性技能。"#;

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
    /// 候选验证门（G2b，默认开启）：新生成技能注册前经 LLM 质量审查
    /// （0-10 打分），低于 [`candidate_score_threshold`] 的标记为
    /// "试验性"技能（仍注册可发现，但 L0 摘要带 [试验性] 提示谨慎使用）。
    /// 验证不可用（LLM 错误/解析失败）时降级为正式注册，不阻塞学习回路。
    pub candidate_verification: bool,
    /// 候选技能正式注册的分数阈值（0-10，默认 6）。
    pub candidate_score_threshold: u8,
    /// 技能去重阈值（0-1）：候选技能与已有技能摘要的字符 bigram Jaccard
    /// 相似度 ≥ 阈值时，不再新建技能，而是完善（覆盖更新）已有技能——
    /// 防止同类技能持续膨胀（如多个"项目探索"近似技能）。
    pub dedup_threshold: f64,
}

impl Default for SkillLearningConfig {
    fn default() -> Self {
        Self {
            enable_auto_learning: true,
            success_threshold: 2,
            min_history_length: 3,
            generation_model: String::new(),
            skill_storage_prefix: "skill/learned".to_string(),
            candidate_verification: true,
            candidate_score_threshold: 6,
            dedup_threshold: 0.45,
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
                        // G2b 候选验证门：注册前 LLM 质量审查，分级（正式/试验性）。
                        let verification = if self.config.candidate_verification {
                            self.verify_candidate(&skill).await.unwrap_or(None)
                        } else {
                            None
                        };
                        let experimental = verification
                            .as_ref()
                            .map(|v| v.score < self.config.candidate_score_threshold)
                            .unwrap_or(false);

                        // 去重门：候选技能与已有技能相似（摘要 bigram 相似度
                        // ≥ dedup_threshold）时完善已有技能，不再新建——防止
                        // 同类技能持续膨胀（如多个"项目探索"近似技能）。
                        let similar = self.find_similar_existing(&skill).await?;
                        let stored = match similar {
                            Some((existing_id, score)) => {
                                tracing::info!(
                                    candidate = %skill.id,
                                    existing = %existing_id,
                                    score,
                                    "检测到相似技能，完善已有技能而非新建"
                                );
                                self.update_existing_skill(&existing_id, &skill, experimental)
                                    .await
                                    .map(|_| true)
                            }
                            None => self.store_skill(&skill, experimental).await.map(|_| true),
                        };
                        match stored {
                            Ok(_) => {
                                if experimental {
                                    tracing::info!(
                                        skill_id = %skill.id,
                                        score = %verification.as_ref().map(|v| v.score).unwrap_or(0),
                                        issues = ?verification.as_ref().map(|v| v.issues.clone()).unwrap_or_default(),
                                        "技能标记为试验性（未达正式阈值）"
                                    );
                                }
                                generated_skills.push(skill);
                            }
                            Err(e) => {
                                tracing::warn!(skill_id = %skill.id, error = %e, "存储技能失败");
                            }
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

    /// 候选技能质量验证（G2b）：LLM 对新生成技能 0-10 打分 + 问题清单。
    ///
    /// 返回 `None` 表示验证不可用（LLM 错误 / 响应解析失败）——调用方
    /// 降级为正式注册，保证学习回路不被验证环节阻塞。
    pub async fn verify_candidate(
        &self,
        skill: &GeneratedSkill,
    ) -> Result<Option<SkillVerification>> {
        let prompt = CANDIDATE_VERIFICATION_PROMPT
            .replace("{name}", &skill.name)
            .replace("{description}", &skill.description)
            .replace("{scenarios}", &skill.applicable_scenarios.join("；"))
            .replace("{content}", &skill.content);

        let response = match self
            .model_service
            .chat(
                &self.config.generation_model,
                vec![crate::common::types::Message::user(prompt)],
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(skill_id = %skill.id, error = %e, "候选技能验证调用失败，降级为正式注册");
                return Ok(None);
            }
        };

        let json: serde_json::Value = match parse_llm_json(&response) {
            Some(v) => v,
            None => {
                tracing::warn!(skill_id = %skill.id, "候选技能验证响应解析失败，降级为正式注册");
                return Ok(None);
            }
        };

        let score = json.get("score").and_then(|v| v.as_u64()).map(|s| s as u8);
        let Some(score) = score else {
            tracing::warn!(skill_id = %skill.id, "候选技能验证缺少 score，降级为正式注册");
            return Ok(None);
        };

        let issues = json
            .get("issues")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|i| i.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        Ok(Some(SkillVerification { score, issues }))
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
            "file_operation",
            "code_operation",
            "search_operation",
            "test_operation",
            "deploy_operation",
            "analysis_operation",
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

    async fn store_skill(&self, skill: &GeneratedSkill, experimental: bool) -> Result<()> {
        let uri = TianyanUri::new(ContextNamespace::Skill, vec![skill.id.clone()]);

        if let Some(parent) = uri.parent() {
            if !self.vfs.exists(&parent).await? {
                self.vfs.create_directory(&parent).await?;
            }
        }

        let skill_md = self.format_skill_as_markdown(skill, experimental);
        self.vfs.write_content(&uri, &skill_md).await?;

        // 试验性技能在 L0 摘要中带 [试验性] 标记——渐进式披露时
        // LLM 可识别状态，谨慎选用（G2b 分级注册）。
        let mut abstract_content = format!(
            "{} | 适用场景: {}",
            skill.description,
            skill.applicable_scenarios.join(", ")
        );
        if experimental {
            abstract_content = format!("[试验性] {}", abstract_content);
        }
        self.vfs.write_abstract(&uri, &abstract_content).await?;

        tracing::info!(skill_id = %skill.id, name = %skill.name, experimental, "技能已存储");
        Ok(())
    }

    /// 查找与候选技能相似（摘要 bigram Jaccard ≥ dedup_threshold）的已有技能。
    ///
    /// 返回 `(已有技能 id, 相似度)`；无相似技能时返回 None。
    async fn find_similar_existing(
        &self,
        candidate: &GeneratedSkill,
    ) -> Result<Option<(String, f64)>> {
        if self.config.dedup_threshold <= 0.0 {
            return Ok(None);
        }
        let skill_root = TianyanUri::new(ContextNamespace::Skill, vec![]);
        let entries = self.vfs.list(&skill_root).await?;

        let candidate_text = format!(
            "{} {} {}",
            candidate.name,
            candidate.description,
            candidate.applicable_scenarios.join(" ")
        );

        let mut best: Option<(String, f64)> = None;
        for entry in entries {
            if !entry.is_directory() {
                continue;
            }
            let Some(id) = entry.uri().path().last().cloned() else {
                continue;
            };
            let abstract_text = self
                .vfs
                .read_abstract(entry.uri())
                .await
                .unwrap_or_default();
            if abstract_text.is_empty() {
                continue;
            }
            let score = text_similarity(&candidate_text, &abstract_text);
            if score >= self.config.dedup_threshold
                && best.as_ref().map(|(_, s)| score > *s).unwrap_or(true)
            {
                best = Some((id, score));
            }
        }
        Ok(best)
    }

    /// 完善已有技能：用候选技能的内容/摘要覆盖更新（保留原 id，刷新存储时间）。
    ///
    /// 与 [Self::store_skill] 同写路径（content.md + abstract.md），
    /// 仅目标 id 不同——技能数量不膨胀，且注册表引用不变。
    async fn update_existing_skill(
        &self,
        existing_id: &str,
        skill: &GeneratedSkill,
        experimental: bool,
    ) -> Result<()> {
        let uri = TianyanUri::new(ContextNamespace::Skill, vec![existing_id.to_string()]);

        let mut skill_md = self.format_skill_as_markdown(skill, experimental);
        // 内容头部的 ID 保留已有技能 id（注册表/引用不受影响）
        skill_md = skill_md.replace(
            &format!("**ID**: `{}`", skill.id),
            &format!("**ID**: `{}`", existing_id),
        );
        self.vfs.write_content(&uri, &skill_md).await?;

        let mut abstract_content = format!(
            "{} | 适用场景: {}",
            skill.description,
            skill.applicable_scenarios.join(", ")
        );
        if experimental {
            abstract_content = format!("[试验性] {}", abstract_content);
        }
        self.vfs.write_abstract(&uri, &abstract_content).await?;

        tracing::info!(
            existing_id = %existing_id,
            source = %skill.id,
            experimental,
            "技能已完善（去重合并）"
        );
        Ok(())
    }

    fn format_skill_as_markdown(&self, skill: &GeneratedSkill, experimental: bool) -> String {
        let mut md = String::new();

        md.push_str(&format!("# {}\n\n", skill.name));
        md.push_str(&format!("**ID**: `{}`\n\n", skill.id));
        md.push_str(&format!("**版本**: {}\n\n", skill.version));
        md.push_str(&format!("**描述**: {}\n\n", skill.description));
        md.push_str(&format!(
            "**状态**: {}\n\n",
            if experimental {
                "试验性（未达正式质量阈值，谨慎使用）"
            } else {
                "正式"
            }
        ));

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

/// 文本相似度（字符 bigram Jaccard，0-1）。
///
/// 用于技能去重：候选技能与已有技能摘要的相似度判断。
/// 空串或单字符文本无法构成 bigram，返回 0。
fn text_similarity(a: &str, b: &str) -> f64 {
    fn bigrams(s: &str) -> HashSet<String> {
        let chars: Vec<char> = s.chars().collect();
        chars
            .windows(2)
            .map(|w| w.iter().collect::<String>())
            .collect()
    }
    let ba = bigrams(a);
    let bb = bigrams(b);
    if ba.is_empty() || bb.is_empty() {
        return 0.0;
    }
    let intersection = ba.intersection(&bb).count();
    let union = ba.union(&bb).count();
    intersection as f64 / union as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_learning_config_default() {
        let config = SkillLearningConfig::default();
        assert!(config.enable_auto_learning);
        assert_eq!(config.success_threshold, 2);
        assert!(config.dedup_threshold > 0.0);
    }

    #[test]
    fn test_text_similarity() {
        // 完全相同 → 1.0
        assert!((text_similarity("项目结构探索", "项目结构探索") - 1.0).abs() < 1e-9);
        // 高度相似（共享"项目结构"等 bigram）
        assert!(
            text_similarity(
                "通过列目录和读文件了解项目结构",
                "快速了解项目目录和文件结构"
            ) > 0.3
        );
        // 完全不同
        assert!(text_similarity("获取当前工作目录路径", "天气晴朗适合出行") < 0.2);
        // 空串
        assert_eq!(text_similarity("", "项目结构"), 0.0);
    }

    #[tokio::test]
    async fn test_categorize_task() {
        let engine = SkillLearningEngine::new(
            mock_chat_empty(),
            Arc::new(MockVfs::new()),
            SkillLearningConfig::default(),
        );

        // mock LLM 返回无效类别 → 回退到关键词匹配
        assert_eq!(
            engine.categorize_task("读取文件内容").await,
            "file_operation"
        );
        assert_eq!(
            engine.categorize_task("搜索代码中的函数").await,
            "code_operation"
        );
        assert_eq!(
            engine.categorize_task("运行单元测试").await,
            "test_operation"
        );
        assert_eq!(
            engine.categorize_task("分析项目结构").await,
            "analysis_operation"
        );
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
    use crate::test_utils::MockChatService;

    /// 创建返回空 JSON 响应的 mock ChatService（用于学习引擎测试）。
    fn mock_chat_empty() -> Arc<dyn ChatService> {
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

    /// 构造固定响应文本的 mock ChatService。
    fn mock_chat_with(response: &str) -> Arc<dyn ChatService> {
        let response = response.to_string();
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().returning(move |_| {
            Ok(ChatCompletionResponse {
                id: "mock".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "mock".to_string(),
                choices: vec![ChatChoice {
                    index: 0,
                    message: crate::common::types::Message::assistant(response.clone()),
                    finish_reason: Some("stop".to_string()),
                }],
                usage: Default::default(),
            })
        });
        Arc::new(mock)
    }

    fn sample_skill() -> GeneratedSkill {
        GeneratedSkill {
            id: "skill-1".to_string(),
            name: "整理日志文件".to_string(),
            description: "按日期归档日志文件到子目录".to_string(),
            content: "1. 读取日志目录\n2. 按日期分组\n3. 移动归档".to_string(),
            applicable_scenarios: vec!["日志维护".to_string()],
            parameters: Vec::new(),
            version: "1.0.0".to_string(),
            source_history: vec!["exec-1".to_string()],
        }
    }

    // ── G2b 候选验证门 ────────────────────────────────────────

    #[tokio::test]
    async fn test_verify_candidate_parses_score_and_issues() {
        let engine = SkillLearningEngine::new(
            mock_chat_with(r#"{"score": 8, "issues": ["无"]}"#),
            Arc::new(MockVfs::new()),
            SkillLearningConfig::default(),
        );

        let verification = engine.verify_candidate(&sample_skill()).await.unwrap();
        let verification = verification.expect("应解析出验证结果");
        assert_eq!(verification.score, 8);
        assert_eq!(verification.issues, vec!["无".to_string()]);
    }

    #[tokio::test]
    async fn test_verify_candidate_unparseable_returns_none() {
        // 解析失败 → None（降级正式注册，不阻塞学习回路）
        let engine = SkillLearningEngine::new(
            mock_chat_with("这不是 JSON"),
            Arc::new(MockVfs::new()),
            SkillLearningConfig::default(),
        );
        assert!(engine
            .verify_candidate(&sample_skill())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_verify_candidate_llm_error_returns_none() {
        use crate::common::error::TianyanError;
        let mut mock = MockChatService::new();
        mock.expect_chat_completion()
            .returning(|_| Err(TianyanError::Custom("模型不可用".to_string())));
        let engine = SkillLearningEngine::new(
            Arc::new(mock),
            Arc::new(MockVfs::new()),
            SkillLearningConfig::default(),
        );
        assert!(engine
            .verify_candidate(&sample_skill())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_store_skill_experimental_marker() {
        // 试验性技能：L2 内容含状态标记，L0 摘要带 [试验性] 前缀
        let vfs = Arc::new(MockVfs::new());
        let engine = SkillLearningEngine::new(
            mock_chat_empty(),
            vfs.clone(),
            SkillLearningConfig::default(),
        );
        engine.store_skill(&sample_skill(), true).await.unwrap();

        let uri = TianyanUri::new(ContextNamespace::Skill, vec!["skill-1".to_string()]);
        let content = vfs
            .read_content(&uri, crate::common::types::ContentLevel::Detail)
            .await
            .unwrap();
        assert!(content.contains("**状态**: 试验性"), "L2 应标记试验性状态");
        let abstract_content = vfs
            .read_content(&uri, crate::common::types::ContentLevel::Abstract)
            .await
            .unwrap();
        assert!(
            abstract_content.starts_with("[试验性]"),
            "L0 摘要应带试验性前缀: {abstract_content}"
        );

        // 正式技能：状态为"正式"，摘要无前缀
        engine.store_skill(&sample_skill(), false).await.unwrap();
        let content = vfs
            .read_content(&uri, crate::common::types::ContentLevel::Detail)
            .await
            .unwrap();
        assert!(content.contains("**状态**: 正式"));
    }
}

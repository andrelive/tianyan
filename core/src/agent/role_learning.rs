//! 角色学习引擎（ADR-016：自演化分工——双管线共生的角色侧）。
//!
//! 与技能学习引擎（GEPA）**共享同一份执行轨迹采集、分开生成**：
//! - 技能引擎：产出「操作流程」技能（原样保留）；
//! - 本引擎：产出 `AgentRole`（系统提示 + 工具白名单）+ 编排技能（何时委托、怎么组合）。
//!
//! 学习信号：重复成功的任务类型（执行条数 ≥ 阈值）→ 角色候选；
//! `delegate_to_agent` 调用轨迹 → 已有角色使用统计（成功率门控输入，P1 仅日志）。
//! 防劣质闸门：G2b 候选验证门（<6 试验性）+ 同名完善（版本递增）+ 数量上限
//! （超出只完善不新建，防注册表膨胀）。

use std::collections::HashMap;
use std::sync::Arc;

use crate::agent::role_store::RoleStore;
use crate::agent::roles::{AgentRole, RoleSource, RoleStatus};
use crate::common::error::{Result, TianyanError};
use crate::common::llm_judge::parse_llm_json;
use crate::common::types::{ContextNamespace, Message, TianyanUri};
use crate::model::ChatService;
use crate::skills::learning::ExecutionHistory;
use crate::vfs::VirtualFileSystem;

/// 角色候选验证提示词（G2b 门，对齐技能引擎 `CANDIDATE_VERIFICATION_PROMPT`）。
pub const ROLE_CANDIDATE_VERIFICATION_PROMPT: &str = r#"你是角色质量审查员。评估以下自动生成的子智能体角色是否值得注册。

角色名：{name}
职责：{purpose}
系统提示：
{system_prompt}
工具白名单：{tools}

评分标准（0-10 分）：
- 职责边界清晰、系统提示可执行（3 分）
- 与内置角色（researcher/editor/reviewer）不重复，有独立价值（4 分）
- 工具白名单与职责匹配，内容不空洞不矛盾（3 分）

输出 JSON（不要输出其他内容）：
{"score": <0-10 整数>, "issues": ["问题1", "问题2"]}

score >= 6 为正式角色；score < 6 标记为试验性。"#;

/// 角色学习配置。
#[derive(Debug, Clone)]
pub struct RoleLearningConfig {
    /// 是否启用自动学习。
    pub enable_auto_learning: bool,
    /// 成功执行条数阈值。
    pub success_threshold: usize,
    /// 最小历史长度。
    pub min_history_length: usize,
    /// 生成模型名称。
    pub generation_model: String,
    /// G2b 候选验证门（默认开启）：新角色注册前经 LLM 质量审查（0-10 打分），
    /// 低于 [`RoleLearningConfig::candidate_score_threshold`] 标记为试验性。
    pub candidate_verification: bool,
    /// 候选角色正式注册的分数阈值（0-10，默认 6）。
    pub candidate_score_threshold: u8,
    /// 学习角色数量上限（防注册表膨胀；超出只完善不新建）。
    pub max_learned_roles: usize,
}

impl Default for RoleLearningConfig {
    fn default() -> Self {
        Self {
            enable_auto_learning: true,
            success_threshold: 2,
            min_history_length: 3,
            generation_model: String::new(),
            candidate_verification: true,
            candidate_score_threshold: 6,
            max_learned_roles: 16,
        }
    }
}

/// 生成的角色候选（LLM 输出解析产物）。
#[derive(Debug, Clone)]
struct GeneratedRoleCandidate {
    role_name: String,
    purpose: String,
    system_prompt: String,
    tools: Vec<String>,
    max_turns: usize,
    orchestration_skill_id: String,
    orchestration_skill_description: String,
    orchestration_skill_content: String,
}

/// 角色验证结果。
#[derive(Debug, Clone)]
struct RoleVerification {
    score: u8,
    issues: Vec<String>,
}

/// 角色学习产物（对外返回）。
#[derive(Debug, Clone)]
pub struct LearnedRole {
    /// 落盘后的角色定义。
    pub role: AgentRole,
    /// 同步产出的编排技能 ID（VFS skill/learned/ 下）。
    pub orchestration_skill_id: Option<String>,
    /// 是否试验性（G2b 门）。
    pub experimental: bool,
    /// 是否完善已有角色（true = 覆盖更新；false = 新建）。
    pub updated_existing: bool,
}

/// 角色学习引擎。
#[derive(Clone)]
pub struct RoleLearningEngine {
    model_service: Arc<dyn ChatService>,
    vfs: Arc<dyn VirtualFileSystem>,
    store: RoleStore,
    config: RoleLearningConfig,
}

impl RoleLearningEngine {
    /// 创建引擎。
    pub fn new(
        model_service: Arc<dyn ChatService>,
        vfs: Arc<dyn VirtualFileSystem>,
        store: RoleStore,
        config: RoleLearningConfig,
    ) -> Self {
        Self {
            model_service,
            vfs,
            store,
            config,
        }
    }

    /// 从执行历史学习角色（含角色使用统计）。
    pub async fn learn_from_history(
        &self,
        history: &[ExecutionHistory],
    ) -> Result<Vec<LearnedRole>> {
        if !self.config.enable_auto_learning || history.len() < self.config.min_history_length {
            return Ok(Vec::new());
        }
        let successful: Vec<_> = history.iter().filter(|h| h.success).collect();
        if successful.len() < self.config.success_threshold {
            return Ok(Vec::new());
        }

        // 已有角色的使用统计（delegate 轨迹；成功率门控输入，P1 仅日志）
        self.record_role_usage(history).await;

        // 聚类：任务类型 → 成功执行组
        let mut grouped: HashMap<String, Vec<&ExecutionHistory>> = HashMap::new();
        for exec in &successful {
            let key = self.categorize_task(&exec.task_description).await;
            grouped.entry(key).or_default().push(exec);
        }

        let mut learned = Vec::new();
        for (task_type, executions) in grouped {
            if executions.len() < self.config.success_threshold {
                continue;
            }
            match self.generate_role(&task_type, &executions).await {
                Ok(candidate) => {
                    // G2b 候选验证门：注册前 LLM 质量审查（验证不可用降级正式）
                    let verification = if self.config.candidate_verification {
                        self.verify_candidate(&candidate).await.unwrap_or(None)
                    } else {
                        None
                    };
                    let experimental = verification
                        .as_ref()
                        .map(|v| {
                            if v.score < self.config.candidate_score_threshold {
                                tracing::info!(
                                    role = %candidate.role_name,
                                    score = v.score,
                                    issues = ?v.issues,
                                    "角色标记为试验性（未达正式阈值）"
                                );
                            }
                            v.score < self.config.candidate_score_threshold
                        })
                        .unwrap_or(false);
                    match self.store_role(&candidate, experimental).await {
                        Ok(outcome) => learned.push(outcome),
                        Err(e) => {
                            tracing::warn!(role = %candidate.role_name, error = %e, "角色存储失败");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(task_type = %task_type, error = %e, "角色生成失败");
                }
            }
        }
        tracing::info!(count = learned.len(), "角色学习完成");
        Ok(learned)
    }

    /// 记录已有角色的使用统计（delegate 轨迹解析 role 参数；P1 仅日志）。
    async fn record_role_usage(&self, history: &[ExecutionHistory]) {
        let mut usage: HashMap<String, (usize, usize)> = HashMap::new();
        for exec in history {
            let desc = &exec.task_description;
            if !desc.starts_with("delegate_to_agent") {
                continue;
            }
            // task_description = "delegate_to_agent: {json}"
            let Some((_, json_part)) = desc.split_once(": ") else {
                continue;
            };
            let Some(role_name) = serde_json::from_str::<serde_json::Value>(json_part)
                .ok()
                .and_then(|v| v.get("role").and_then(|r| r.as_str()).map(str::to_string))
            else {
                continue;
            };
            let e = usage.entry(role_name).or_insert((0, 0));
            e.0 += 1;
            if exec.success {
                e.1 += 1;
            }
        }
        for (role, (total, ok)) in usage {
            tracing::info!(role = %role, total, success = ok, "角色使用统计");
        }
    }

    /// 任务分类（关键词匹配；与技能引擎同源简化版，分类失败不影响学习回路）。
    async fn categorize_task(&self, description: &str) -> String {
        let lower = description.to_lowercase();
        if lower.contains("delegate_to_agent") {
            return "delegation".to_string();
        }
        if lower.contains("web_search") || lower.contains("web_fetch") {
            return "web_research".to_string();
        }
        if lower.contains("search_code") || lower.contains("search_knowledge") {
            return "search".to_string();
        }
        if lower.contains("apply_edit")
            || lower.contains("apply_patch")
            || lower.contains("write_file")
        {
            return "code_edit".to_string();
        }
        if lower.contains("run_tests") || lower.contains("verify_build") {
            return "verify".to_string();
        }
        if lower.contains("execute_command") {
            return "command".to_string();
        }
        "general".to_string()
    }

    /// 生成角色候选（LLM）。
    async fn generate_role(
        &self,
        task_type: &str,
        executions: &[&ExecutionHistory],
    ) -> Result<GeneratedRoleCandidate> {
        let prompt = build_role_generation_prompt(task_type, executions);
        let response = self
            .model_service
            .chat(&self.config.generation_model, vec![Message::user(prompt)])
            .await?;
        parse_generated_role(&response, task_type)
    }

    /// G2b 候选角色质量验证（0-10 打分；不可用时降级正式注册）。
    async fn verify_candidate(
        &self,
        candidate: &GeneratedRoleCandidate,
    ) -> Result<Option<RoleVerification>> {
        let prompt = ROLE_CANDIDATE_VERIFICATION_PROMPT
            .replace("{name}", &candidate.role_name)
            .replace("{purpose}", &candidate.purpose)
            .replace("{system_prompt}", &candidate.system_prompt)
            .replace("{tools}", &candidate.tools.join(", "));

        let response = match self
            .model_service
            .chat(&self.config.generation_model, vec![Message::user(prompt)])
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(role = %candidate.role_name, error = %e, "角色验证调用失败，降级为正式注册");
                return Ok(None);
            }
        };

        let json: serde_json::Value = match parse_llm_json(&response) {
            Some(v) => v,
            None => {
                tracing::warn!(role = %candidate.role_name, "角色验证响应解析失败，降级为正式注册");
                return Ok(None);
            }
        };

        let Some(score) = json.get("score").and_then(|v| v.as_u64()).map(|s| s as u8) else {
            tracing::warn!(role = %candidate.role_name, "角色验证缺少 score，降级为正式注册");
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

        Ok(Some(RoleVerification { score, issues }))
    }

    /// 落盘角色：同名完善（version+1）/ 新建（数量上限内）+ 编排技能。
    async fn store_role(
        &self,
        candidate: &GeneratedRoleCandidate,
        experimental: bool,
    ) -> Result<LearnedRole> {
        let existing = self.store.load_role(&candidate.role_name).await?;
        let had_existing = existing.is_some();
        let role = match existing {
            Some(mut ex) => {
                // 同名完善：保留 name 与原始来源链，版本递增
                let lineage = ex.lineage.clone().or_else(|| {
                    if ex.source == RoleSource::Learned {
                        None
                    } else {
                        Some(ex.name.clone())
                    }
                });
                ex.system_prompt = Some(candidate.system_prompt.clone());
                ex.tools = Some(candidate.tools.clone());
                ex.max_turns = Some(candidate.max_turns);
                ex.source = RoleSource::Learned;
                ex.status = if experimental {
                    RoleStatus::Experimental
                } else {
                    RoleStatus::Active
                };
                ex.version += 1;
                ex.lineage = lineage;
                ex
            }
            None => {
                // 新建（受数量上限约束：超出只完善不新建）
                let learned_count = self
                    .store
                    .load_roles()
                    .await?
                    .iter()
                    .filter(|r| r.source == RoleSource::Learned)
                    .count();
                if learned_count >= self.config.max_learned_roles {
                    tracing::info!(
                        role = %candidate.role_name,
                        limit = self.config.max_learned_roles,
                        "学习角色已达上限，跳过新建"
                    );
                    return Ok(LearnedRole {
                        role: AgentRole::default(),
                        orchestration_skill_id: None,
                        experimental,
                        updated_existing: false,
                    });
                }
                AgentRole {
                    name: candidate.role_name.clone(),
                    system_prompt: Some(candidate.system_prompt.clone()),
                    tools: Some(candidate.tools.clone()),
                    max_turns: Some(candidate.max_turns),
                    source: RoleSource::Learned,
                    status: if experimental {
                        RoleStatus::Experimental
                    } else {
                        RoleStatus::Active
                    },
                    version: 1,
                    lineage: None,
                    ..Default::default()
                }
            }
        };
        self.store.save_role(&role).await?;

        // 编排技能同步落盘（skill/learned/ 命名空间）
        let skill_id = self
            .store_orchestration_skill(candidate, experimental)
            .await?;
        Ok(LearnedRole {
            role,
            orchestration_skill_id: Some(skill_id),
            experimental,
            updated_existing: had_existing,
        })
    }

    /// 编排技能落盘：内容 = 触发条件 / 建议工作流 / 注意事项 / 反模式。
    async fn store_orchestration_skill(
        &self,
        candidate: &GeneratedRoleCandidate,
        experimental: bool,
    ) -> Result<String> {
        let uri = TianyanUri::new(
            ContextNamespace::Skill,
            vec![candidate.orchestration_skill_id.clone()],
        );
        if let Some(parent) = uri.parent() {
            if !self.vfs.exists(&parent).await? {
                self.vfs.create_directory(&parent).await?;
            }
        }
        let md = format!(
            "# {}\n\n**ID**: `{}`\n\n**关联角色**: `{}`\n\n**适用场景**: {}\n\n{}",
            candidate.orchestration_skill_description,
            candidate.orchestration_skill_id,
            candidate.role_name,
            candidate.purpose,
            candidate.orchestration_skill_content
        );
        self.vfs.write_content(&uri, &md).await?;
        let mut abstract_content = format!(
            "{} | 关联角色: {}",
            candidate.orchestration_skill_description, candidate.role_name
        );
        if experimental {
            abstract_content = format!("[试验性] {}", abstract_content);
        }
        self.vfs.write_abstract(&uri, &abstract_content).await?;
        Ok(candidate.orchestration_skill_id.clone())
    }
}

/// 角色生成提示词：给定任务类型与成功执行，产出角色定义 + 编排技能。
fn build_role_generation_prompt(task_type: &str, executions: &[&ExecutionHistory]) -> String {
    let mut prompt = format!(
        "基于以下重复成功的执行历史，判断是否值得分化出一个专门的子智能体角色，
并生成角色定义与编排技能。\n\n任务类型: {}\n\n执行历史:\n",
        task_type
    );

    for (i, exec) in executions.iter().enumerate() {
        prompt.push_str(&format!("\n--- 执行 {} ---\n", i + 1));
        prompt.push_str(&format!("任务: {}\n", exec.task_description));
        prompt.push_str(&format!("结果: {}\n", exec.result));
    }

    prompt.push_str(
        r#"
请生成一个结构化的角色定义，格式如下：

```json
{
  "role_name": "角色名（小写英文和连字符）",
  "purpose": "一句话职责（≤50 字，用于角色列表展示）",
  "system_prompt": "完整系统提示（角色身份、职责边界、协作方式、只做什么不做什么）",
  "tools": ["工具名列表（从执行中实际用到的工具提炼，与职责匹配）"],
  "max_turns": 200,
  "orchestration_skill_id": "编排技能 ID（role-<name>-guide）",
  "orchestration_skill_description": "编排技能一句话描述",
  "orchestration_skill_content": "编排指南（Markdown：触发条件、建议工作流、注意事项、反模式）"
}
```

要求：
1. 仅当任务确实呈现稳定重复、值得独立分工时才生成；边界模糊时 role_name 用具体职责命名
2. 系统提示要写明职责边界（只做什么）与协作方式（什么情况汇报主任务）
3. 工具白名单要与职责匹配且尽量精简
4. 编排指南要告诉主智能体何时委托、怎么组合、注意什么
5. 不要与内置角色（researcher/editor/reviewer）职责重复
"#,
    );

    prompt
}

/// 解析 LLM 生成的角色候选。
fn parse_generated_role(response: &str, task_type: &str) -> Result<GeneratedRoleCandidate> {
    let json: serde_json::Value = parse_llm_json(response)
        .ok_or_else(|| TianyanError::Custom("内部错误：解析生成的角色失败".to_string()))?;

    let role_name = json
        .get("role_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("learned-{}", task_type));
    let purpose = json
        .get("purpose")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let system_prompt = json
        .get("system_prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tools = json
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let max_turns = json
        .get("max_turns")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(200)
        .clamp(1, 500);
    let orchestration_skill_id = json
        .get("orchestration_skill_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("role-{role_name}-guide"));
    let orchestration_skill_description = json
        .get("orchestration_skill_description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let orchestration_skill_content = json
        .get("orchestration_skill_content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(GeneratedRoleCandidate {
        role_name,
        purpose,
        system_prompt,
        tools,
        max_turns,
        orchestration_skill_id,
        orchestration_skill_description,
        orchestration_skill_content,
    })
}

#[cfg(test)]
#[path = "role_learning_tests.rs"]
mod tests;

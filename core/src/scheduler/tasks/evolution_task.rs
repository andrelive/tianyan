//! 自演化任务（ADR-017：每日演化智能体任务）。
//!
//! 三阶段编排（单实例、串行）：
//! - 阶段 0 采集：水位线（TaskStateStore）+ 当前注册表清单（记忆/技能/规则/角色）；
//! - 阶段 1 综述：注入的 EvolutionReviewExecutor（server 实现：委托/唤醒智能体）
//!   产出结构化 diff 计划（JSON）；
//! - 阶段 2 记账提交：应用增删改（软删除 _archive/ + 演化报告 + 水位线）。
//!
//! 提议/提交分离：综述智能体的工具白名单只读（执行统计/会话回忆/VFS 只读），
//! 无法直接修改注册表——增删改必须经本任务应用（架构上强制，ADR-017 决策 2）。

use std::sync::Arc;

use async_trait::async_trait;

use crate::common::error::{Result, TianyanError};
use crate::common::llm_judge::parse_llm_json;
use crate::common::types::{
    memory_paths, AgentPath, ContentLevel, ContextNamespace, MemoryCategory, MemoryEntry,
    TianyanUri,
};
use crate::memory::format_memory_as_markdown;
use crate::role_store::RoleStore;
use crate::roles::{AgentRole, RoleSource, RoleStatus};
use crate::scheduler::{TaskContext, TaskHandler, TaskResult};

/// 演化综述执行器（server 提供实现：委托 evolution_reviewer 角色 / 唤醒主 agent）。
#[async_trait]
pub trait EvolutionReviewExecutor: Send + Sync {
    /// 执行一次演化综述。
    ///
    /// input 为任务采集的输入包（上次运行时间 + 注册表清单 + 指令）；
    /// 返回智能体产出的 diff 计划（JSON 文本）。
    async fn review(&self, input: &str) -> std::result::Result<String, TianyanError>;
}

/// 记忆 diff 计划项。
#[derive(Debug, Clone, serde::Deserialize)]
struct MemoryPlan {
    action: String,
    category: String,
    content: String,
    #[serde(default)]
    importance: f32,
    #[serde(default)]
    id: Option<String>,
    /// `merge` 动作：被合并（归档）的其它同主题条目 id 列表。
    #[serde(default)]
    merge_from: Vec<String>,
}

/// 技能 diff 计划项。
#[derive(Debug, Clone, serde::Deserialize)]
struct SkillPlan {
    action: String,
    id: String,
    name: String,
    description: String,
    content: String,
    #[serde(default)]
    applicable_scenarios: Vec<String>,
}

/// 规则 diff 计划项。
#[derive(Debug, Clone, serde::Deserialize)]
struct RulePlan {
    action: String,
    #[serde(rename = "abstract", default)]
    abstract_text: String,
    content: String,
}

/// 角色 diff 计划项。
#[derive(Debug, Clone, serde::Deserialize)]
struct RolePlan {
    action: String,
    id: String,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    tools: Option<Vec<String>>,
}

/// 删除 diff 计划项（软删除：归档到 _archive/）。
#[derive(Debug, Clone, serde::Deserialize)]
struct DeletionPlan {
    kind: String,
    id: String,
    #[serde(default)]
    reason: String,
}

/// 综述智能体输出的完整 diff 计划。
#[derive(Debug, Clone, serde::Deserialize)]
struct EvolutionPlan {
    #[serde(default)]
    memories: Vec<MemoryPlan>,
    #[serde(default)]
    skills: Vec<SkillPlan>,
    #[serde(default)]
    rules: Vec<RulePlan>,
    #[serde(default)]
    roles: Vec<RolePlan>,
    #[serde(default)]
    deletions: Vec<DeletionPlan>,
    #[serde(default)]
    summary: String,
}

/// 自演化任务（ADR-017：每日演化综述 + 记账提交）。
pub struct EvolutionTask {
    executor: Arc<dyn EvolutionReviewExecutor>,
    max_items_per_run: usize,
}

impl EvolutionTask {
    /// 创建自演化任务。
    ///
    /// executor 为综述执行器（server 装配）；max_items_per_run 为单次
    /// 运行处理上限（注册表清单条目数，防 token 爆炸）。
    pub fn new(executor: Arc<dyn EvolutionReviewExecutor>, max_items_per_run: usize) -> Self {
        Self {
            executor,
            max_items_per_run,
        }
    }
}

#[async_trait]
impl TaskHandler for EvolutionTask {
    async fn execute(&self, ctx: &TaskContext) -> TaskResult {
        match self.run_evolution(ctx).await {
            Ok(applied) => TaskResult::success(applied),
            Err(e) => TaskResult::failed(e),
        }
    }

    fn name(&self) -> &str {
        "evolution"
    }
}

impl EvolutionTask {
    /// 三阶段编排：采集 → 综述 → 记账提交。
    async fn run_evolution(&self, ctx: &TaskContext) -> Result<usize> {
        tracing::info!("开始执行自演化任务（ADR-017）");

        // 阶段 0：采集（水位线 + 注册表清单）
        let (last_run_at, last_run_summary) = self.read_watermark(ctx).await;
        let inventory = self.collect_inventory(ctx).await?;
        let consolidate = ctx.config.memory.auto_consolidation;
        let input = self.build_review_input(&last_run_at, &inventory, consolidate);

        // 阶段 1：综述（智能体产出 diff 计划）
        let plan_text = self
            .executor
            .review(&input)
            .await
            .map_err(|e| TianyanError::Custom(format!("evolution: 综述失败：{e}")))?;
        // 解析失败降级为空计划：综述输出不可解析时不再让整个任务失败
        // （历史教训：模型偶尔以散文回复或把 JSON 写进剪贴板工具，导致
        // 自演化综述从未产出过任何报告/规则/记忆）。降级保证演化报告与
        // 水位线照常落盘，综述原文截断进摘要——失败可回溯、下周期可续跑。
        let plan = match parse_plan(&plan_text) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "evolution: 综述输出解析失败，按空计划降级（报告照常落盘）"
                );
                EvolutionPlan {
                    memories: Vec::new(),
                    skills: Vec::new(),
                    rules: Vec::new(),
                    roles: Vec::new(),
                    deletions: Vec::new(),
                    summary: first_line_truncated(&plan_text),
                }
            }
        };

        // 阶段 2：记账提交
        let applied = self.apply_plan(ctx, &plan).await?;
        self.write_report(ctx, &plan, applied, &last_run_summary)
            .await?;

        // 水位线更新（G5 跨运行状态）
        let summary = format!(
            "# 演化周期摘要
- 时间: {}
- 应用变更: {}
- 综述摘要: {}
",
            chrono::Utc::now().to_rfc3339(),
            applied,
            plan.summary
        );
        ctx.task_state.write("evolution", &summary).await?;

        tracing::info!("演化任务完成，应用了 {} 项变更", applied);
        Ok(applied)
    }

    /// 读取上次运行水位线（TaskStateStore）。
    async fn read_watermark(&self, ctx: &TaskContext) -> (Option<String>, Option<String>) {
        match ctx.task_state.read("evolution").await {
            Ok(Some(prev)) => {
                let first_line = prev.lines().next().unwrap_or("").to_string();
                let ts_line = prev
                    .lines()
                    .find(|l| l.starts_with("- 时间:"))
                    .map(|l| l.trim_start_matches("- 时间:").trim().to_string());
                (ts_line, Some(first_line))
            }
            Ok(None) => (None, None),
            Err(e) => {
                tracing::warn!(error = %e, "读取演化水位线失败");
                (None, None)
            }
        }
    }

    /// 采集当前注册表清单（记忆/技能/规则/角色 + 各自 L0 摘要首行）。
    ///
    /// memory 为嵌套结构（cases/events/facts 子目录）——递归收集叶子条目；
    /// 其余命名空间为单层文件列表——浅列即可。运维数据子域统一跳过
    /// （非记忆内容，不进综述清单）。
    async fn collect_inventory(&self, ctx: &TaskContext) -> Result<String> {
        let mut out = String::new();
        let mut count = 0usize;
        // memory：递归叶子清单
        let memory_root = TianyanUri::new(ContextNamespace::Memory, vec![]);
        collect_memory_inventory(
            ctx,
            &memory_root,
            &mut out,
            &mut count,
            self.max_items_per_run,
            0,
        )
        .await;

        let roots = [
            ("skill", TianyanUri::new(ContextNamespace::Skill, vec![])),
            (
                "agent/learned",
                TianyanUri::new(ContextNamespace::Agent, vec!["learned".to_string()]),
            ),
            (
                "agent/roles",
                TianyanUri::new(ContextNamespace::Agent, vec!["roles".to_string()]),
            ),
        ];
        for (label, root) in &roots {
            collect_inventory_ns(
                ctx,
                label,
                root,
                &mut out,
                &mut count,
                self.max_items_per_run,
            )
            .await;
        }

        Ok(out)
    }
}

impl EvolutionTask {
    /// 组装综述输入包（上次运行时间 + 注册表清单 + 综述指令）。
    ///
    /// `consolidate` 对应 `[memory] auto_consolidation`：启用时综述包含
    /// 记忆巩固职责（查重 / 合并归纳），关闭时保持最简写入原则。
    fn build_review_input(
        &self,
        last_run_at: &Option<String>,
        inventory: &str,
        consolidate: bool,
    ) -> String {
        let last = last_run_at.as_deref().unwrap_or("从未运行（首次演化综述）");
        let consolidation_principle = if consolidate {
            "- 记忆治理（巩固）：写记忆前先查同类——同主题已有条目时用 merge 归纳更新，\n  不要重复新增；发现同主题碎片（≥2 条旧条目）时归纳为一条主题条目，\n  被归纳的旧条目列入 merge_from 一并归档；\n"
        } else {
            ""
        };
        let merge_example = if consolidate {
            ",\n    {\"action\": \"merge\", \"id\": \"既有条目 id（先用 vfs_read 确认）\", \"category\": \"...\", \"content\": \"归纳后的完整内容\", \"importance\": 0.8, \"merge_from\": [\"被合并的其它条目 id\"]}"
        } else {
            ""
        };
        format!(
            r#"你是演化综述员。请完成一次天演自演化综述。

【上次演化运行时间】
{last}

【当前注册表清单】（L0 摘要首行；如需详情用 vfs_read 查看）
{inventory}

【本次任务】
4. 自行决定输出哪些内容作为新的记忆 / 技能 / 规则 / 组织形态，以及哪些应更新、合并或删除。
2. 用 session_recall 回忆近期会话内容，识别用户偏好、事实与重复工作模式；
3. 对照上面清单，自行决定查看哪些历史记忆、经验、组织形态；
4. 自行决定输出哪些内容作为新的记忆 / 技能 / 规则 / 组织形态，以及哪些应更新或删除。
  "memories": [
    {{"action": "add", "category": "preference|decision|fact|entity|pattern|successful_case|failed_case", "content": "...", "importance": 0.8, "id": "可选"}}{merge_example}
  ],
【输出格式】只输出以下 JSON（不要输出其他内容）。
不要使用任何工具（如 clipboard_write）输出综述结果——最终回复文本必须直接就是 JSON：
{{
  "memories": [{{"action": "add", "category": "preference|decision|fact|entity|pattern|successful_case|failed_case", "content": "...", "importance": 0.8, "id": "可选"}}],
  "skills": [{{"action": "add|update", "id": "kebab-case", "name": "...", "description": "...", "content": "使用说明（Markdown）", "applicable_scenarios": ["..."]}}],
  "rules": [{{"action": "add", "abstract": "规则摘要", "content": "规则详情"}}],
  "roles": [{{"action": "create|retire", "id": "role-name", "system_prompt": "...", "tools": ["..."]}}],
  "deletions": [{{"kind": "memory|skill|rule", "id": "...", "reason": "删除证据"}}],
{consolidation_principle}- 只把稳定、跨会话可复用的偏好 / 事实 / 教训 / 决策写入记忆；临时性、一次性内容
  与过程记录（版本发布、功能完成、例行里程碑）不写记忆——它们在演化报告与项目文档中留痕；
- 删除必须有证据（被取代 / 已过时 / 低分评审）——过时、被证伪、重复的条目（含记忆）
  应主动列入 deletions；

【写前原则】
- 写新技能前先查现有技能（语义重复则完善而非新建）；
- 只把稳定、跨会话可复用的偏好写入画像（临时性、一次性的不写）；
            consolidation_principle = consolidation_principle,
- 删除必须有证据（被取代 / 已过时 / 低分评审）；
- 不确定的内容宁可少写，不要污染注册表。
"#,
            last = last,
            inventory = inventory,
            merge_example = merge_example,
        )
    }

    /// 应用 diff 计划（记忆/技能/规则/角色/删除）。
    async fn apply_plan(&self, ctx: &TaskContext, plan: &EvolutionPlan) -> Result<usize> {
        let mut applied = 0usize;
        let consolidate = ctx.config.memory.auto_consolidation;
        for m in &plan.memories {
            match m.action.as_str() {
                "add" => match self.apply_memory(ctx, m).await {
                    Ok(()) => applied += 1,
                    Err(e) => tracing::warn!(content = %m.content, error = %e, "记忆应用失败"),
                },
                "merge" if consolidate => match self.apply_memory_merge(ctx, m).await {
                    Ok(()) => applied += 1,
                    Err(e) => tracing::warn!(content = %m.content, error = %e, "记忆合并应用失败"),
                },
                "merge" => {
                    tracing::debug!(id = ?m.id, "自动巩固已禁用（auto_consolidation=false），跳过 merge");
                }
                other => tracing::warn!(action = %other, "未知记忆动作，跳过"),
            }
        }
        for s in &plan.skills {
            if s.action != "add" && s.action != "update" {
                continue;
            }
            match self.apply_skill(ctx, s).await {
                Ok(()) => applied += 1,
                Err(e) => tracing::warn!(skill = %s.id, error = %e, "技能应用失败"),
            }
        }
        for r in &plan.rules {
            if r.action != "add" {
                continue;
            }
            match self.apply_rule(ctx, r).await {
                Ok(()) => applied += 1,
                Err(e) => tracing::warn!(error = %e, "规则应用失败"),
            }
        }
        for role in &plan.roles {
            match self.apply_role(ctx, role).await {
                Ok(()) => applied += 1,
                Err(e) => tracing::warn!(role = %role.id, error = %e, "角色应用失败"),
            }
        }
        for d in &plan.deletions {
            match self.apply_deletion(ctx, d).await {
                Ok(()) => applied += 1,
                Err(e) => tracing::warn!(id = %d.id, error = %e, "删除应用失败"),
            }
        }
        Ok(applied)
    }

    /// 应用记忆新增（写入 VFS memory 命名空间 + L0 摘要）。
    async fn apply_memory(&self, ctx: &TaskContext, m: &MemoryPlan) -> Result<()> {
        let category = parse_category(&m.category)?;
        let id =
            m.id.clone()
                .unwrap_or_else(|| format!("evo-{}", chrono::Utc::now().timestamp()));
        let mut entry = MemoryEntry::new(&id, &m.content, category);
        entry.importance = m.importance.clamp(0.0, 1.0);
        entry.source_session = Some("evolution".to_string());

        let uri = category.default_uri(&id);
        if let Some(parent) = uri.parent() {
            if !ctx.vfs.exists(&parent).await? {
                ctx.vfs.create_directory(&parent).await?;
            }
        }
        let content = format_memory_as_markdown(&entry);
        ctx.vfs.write_content(&uri, &content).await?;
        let abstract_content = format!(
            "{} | 重要性: {:.2} | 类别: {} | 来源: evolution",
            entry.content, entry.importance, entry.category
        );
        ctx.vfs.write_abstract(&uri, &abstract_content).await?;
        tracing::info!(uri = %uri, "演化记忆已写入");
        Ok(())
    }

    /// 应用记忆合并（巩固）：重写目标条目 + 归档被合并条目。
    ///
    /// - `id` 为目标条目：全树查找既有文件条目，命中则原地重写内容与摘要；
    ///   未命中则按 `category.default_uri` 新建（降级为 add——综述认知与现状
    ///   不一致时内容不丢）。
    /// - `merge_from` 列出的其它条目：走软删除通道归档到 `memory/archive/`
    ///   （摘要标注"已被合并到 {目标}"），从活跃记忆移除。
    async fn apply_memory_merge(&self, ctx: &TaskContext, m: &MemoryPlan) -> Result<()> {
        let Some(target_id) = m.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            return Err(TianyanError::invalid_input(
                "evolution: merge 记忆缺少目标 id",
            ));
        };
        let category = parse_category(&m.category)?;
        let memory_root = TianyanUri::new(ContextNamespace::Memory, vec![]);

        // 目标条目：全树查找；未命中降级为新建（保持与 apply_memory 相同的写路径）
        let target_uri = match find_entry(ctx, &memory_root, target_id, 0).await? {
            Some(uri) => uri,
            None => {
                let uri = category.default_uri(target_id);
                if let Some(parent) = uri.parent() {
                    if !ctx.vfs.exists(&parent).await? {
                        ctx.vfs.create_directory(&parent).await?;
                    }
                }
                uri
            }
        };

        let mut entry = MemoryEntry::new(target_id, &m.content, category);
        entry.importance = m.importance.clamp(0.0, 1.0);
        entry.source_session = Some("evolution".to_string());
        let content = format_memory_as_markdown(&entry);
        ctx.vfs.write_content(&target_uri, &content).await?;
        let abstract_content = format!(
            "{} | 重要性: {:.2} | 类别: {} | 来源: evolution",
            entry.content, entry.importance, entry.category
        );
        ctx.vfs
            .write_abstract(&target_uri, &abstract_content)
            .await?;
        tracing::info!(
            uri = %target_uri,
            merge_from = m.merge_from.len(),
            "记忆合并已应用"
        );

        // 归档被合并条目（复用软删除通道；防自吞与空 id）
        for from_id in &m.merge_from {
            let from_id = from_id.trim();
            if from_id.is_empty() || from_id == target_id {
                continue;
            }
            let d = DeletionPlan {
                kind: "memory".to_string(),
                id: from_id.to_string(),
                reason: format!("已被合并到 {target_id}"),
            };
            match self.apply_deletion(ctx, &d).await {
                Ok(()) => tracing::info!(id = %from_id, to = %target_id, "被合并条目已归档"),
                Err(e) => tracing::warn!(id = %from_id, error = %e, "合并归档失败"),
            }
        }

        Ok(())
    }

    /// 应用技能新增/更新（写入 skill/{id} content + abstract）。
    async fn apply_skill(&self, ctx: &TaskContext, s: &SkillPlan) -> Result<()> {
        let uri = TianyanUri::new(ContextNamespace::Skill, vec![s.id.clone()]);
        if let Some(parent) = uri.parent() {
            if !ctx.vfs.exists(&parent).await? {
                ctx.vfs.create_directory(&parent).await?;
            }
        }
        let mut md = String::new();
        md.push_str(&format!("# {}\n\n", s.name));
        md.push_str(&format!("**ID**: {}\n\n", s.id));
        md.push_str(&format!("**描述**: {}\n\n", s.description));
        md.push_str("**状态**: 正式（演化综述产物）\n\n");
        if !s.applicable_scenarios.is_empty() {
            md.push_str("## 适用场景\n\n");
            for scenario in &s.applicable_scenarios {
                md.push_str(&format!("- {}\n", scenario));
            }
            md.push('\n');
        }
        md.push_str("## 使用说明\n\n");
        md.push_str(&s.content);
        md.push('\n');
        ctx.vfs.write_content(&uri, &md).await?;
        let abstract_content = format!(
            "{} | 适用场景: {} | 来源: evolution",
            s.description,
            s.applicable_scenarios.join(", ")
        );
        ctx.vfs.write_abstract(&uri, &abstract_content).await?;
        tracing::info!(skill_id = %s.id, "技能已写入/更新");
        Ok(())
    }
}

impl EvolutionTask {
    /// 应用规则新增（agent/learned/{id}）。
    async fn apply_rule(&self, ctx: &TaskContext, r: &RulePlan) -> Result<()> {
        let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let rule_id = format!("rule-evo-{}", ts);
        let uri = AgentPath::Learned.uri().append(&rule_id);
        if !ctx.vfs.exists(&uri).await? {
            ctx.vfs.create_file(&uri).await?;
        }
        let abstract_content = format!("[演化] {} (来源: evolution)", r.abstract_text);
        ctx.vfs.write_abstract(&uri, &abstract_content).await?;
        let detail = format!(
            "来源: evolution（自演化综述）\n\n{}\n\n---\n## 规则元数据\n- 生成时间: {}\n",
            r.content,
            chrono::Utc::now().to_rfc3339()
        );
        ctx.vfs.write_content(&uri, &detail).await?;
        tracing::info!(rule_uri = %uri, "规则已写入");
        Ok(())
    }

    /// 应用角色新增/退役（RoleStore 持久化；退役软删除到 _archive/）。
    async fn apply_role(&self, ctx: &TaskContext, role: &RolePlan) -> Result<()> {
        let store = RoleStore::new(ctx.vfs.clone());
        match role.action.as_str() {
            "create" => {
                let r = AgentRole {
                    name: role.id.clone(),
                    model: None,
                    system_prompt: role.system_prompt.clone(),
                    tools: role.tools.clone(),
                    max_turns: None,
                    timeout_secs: None,
                    source: RoleSource::Learned,
                    status: RoleStatus::Experimental,
                    version: 1,
                    lineage: Some("evolution".to_string()),
                };
                store.save_role(&r).await?;
                tracing::info!(role = %role.id, "角色已创建（试验性）");
            }
            "retire" => {
                let uri = TianyanUri::new(
                    ContextNamespace::Agent,
                    vec!["roles".to_string(), role.id.clone()],
                );
                if ctx.vfs.exists(&uri).await? {
                    let archive = TianyanUri::new(
                        ContextNamespace::Agent,
                        vec!["roles".to_string(), "_archive".to_string(), role.id.clone()],
                    );
                    if let Some(parent) = archive.parent() {
                        if !ctx.vfs.exists(&parent).await? {
                            ctx.vfs.create_directory(&parent).await?;
                        }
                    }
                    if let Ok(content) = ctx.vfs.read_content(&uri, ContentLevel::Detail).await {
                        ctx.vfs.write_content(&archive, &content).await?;
                    }
                    if let Ok(abstract_text) = ctx.vfs.read_abstract(&uri).await {
                        ctx.vfs.write_abstract(&archive, &abstract_text).await?;
                    }
                    ctx.vfs.delete(&uri).await?;
                    tracing::info!(role = %role.id, "角色已软删除（归档）");
                }
            }
            _ => {
                tracing::warn!(action = %role.action, "未知角色动作，跳过");
            }
        }
        Ok(())
    }

    /// 应用删除（软删除：归档副本 + 删除原条目）。
    async fn apply_deletion(&self, ctx: &TaskContext, d: &DeletionPlan) -> Result<()> {
        let (root, archive_root) = match d.kind.as_str() {
            "memory" => (
                TianyanUri::new(ContextNamespace::Memory, vec![]),
                TianyanUri::new(ContextNamespace::Memory, vec!["archive".to_string()]),
            ),
            "skill" => (
                TianyanUri::new(ContextNamespace::Skill, vec![]),
                TianyanUri::new(ContextNamespace::Skill, vec!["_archive".to_string()]),
            ),
            "rule" => (
                TianyanUri::new(ContextNamespace::Agent, vec!["learned".to_string()]),
                TianyanUri::new(
                    ContextNamespace::Agent,
                    vec!["_archive".to_string(), "learned".to_string()],
                ),
            ),
            other => {
                tracing::warn!(kind = %other, "未知删除类型，跳过");
                return Ok(());
            }
        };

        let Some(original) = find_entry(ctx, &root, &d.id, 0).await? else {
            tracing::debug!(id = %d.id, "删除目标不存在，跳过");
            return Ok(());
        };

        // 归档副本（保留可回退）
        let archive = archive_root.append(&d.id);
        if let Some(parent) = archive.parent() {
            if !ctx.vfs.exists(&parent).await? {
                ctx.vfs.create_directory(&parent).await?;
            }
        }
        if let Ok(content) = ctx.vfs.read_content(&original, ContentLevel::Detail).await {
            ctx.vfs.write_content(&archive, &content).await?;
        }
        if let Ok(abstract_text) = ctx.vfs.read_abstract(&original).await {
            let marked = format!("[已归档: {}] {}", d.reason, abstract_text);
            ctx.vfs.write_abstract(&archive, &marked).await?;
        }
        ctx.vfs.delete(&original).await?;
        tracing::info!(original = %original, archive = %archive, "条目已软删除");
        Ok(())
    }

    /// 写演化报告（memory/events/evolution_reports/{ts}.md）。
    async fn write_report(
        &self,
        ctx: &TaskContext,
        plan: &EvolutionPlan,
        applied: usize,
        last_summary: &Option<String>,
    ) -> Result<()> {
        let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let uri = TianyanUri::new(
            ContextNamespace::Memory,
            vec![
                "events".to_string(),
                "evolution_reports".to_string(),
                format!("{ts}.md"),
            ],
        );
        if let Some(parent) = uri.parent() {
            if !ctx.vfs.exists(&parent).await? {
                ctx.vfs.create_directory(&parent).await?;
            }
        }
        let mut md = String::new();
        md.push_str(&format!(
            "# 演化报告 {}\n\n",
            chrono::Utc::now().to_rfc3339()
        ));
        md.push_str(&format!("- 应用变更: {}\n", applied));
        if let Some(prev) = last_summary {
            md.push_str(&format!(
                "- 上次周期: {}\n",
                prev.lines().next().unwrap_or("")
            ));
        }
        md.push_str(&format!("\n## 综述摘要\n\n{}\n", plan.summary));
        md.push_str(&format!("\n## 记忆变更（{}）\n", plan.memories.len()));
        for m in &plan.memories {
            md.push_str(&format!("- [{}] {}: {}\n", m.action, m.category, m.content));
        }
        md.push_str(&format!("\n## 技能变更（{}）\n", plan.skills.len()));
        for s in &plan.skills {
            md.push_str(&format!("- [{}] {}: {}\n", s.action, s.id, s.description));
        }
        md.push_str(&format!("\n## 规则变更（{}）\n", plan.rules.len()));
        for r in &plan.rules {
            md.push_str(&format!("- [{}] {}\n", r.action, r.abstract_text));
        }
        md.push_str(&format!("\n## 角色变更（{}）\n", plan.roles.len()));
        for role in &plan.roles {
            md.push_str(&format!("- [{}] {}\n", role.action, role.id));
        }
        md.push_str(&format!("\n## 删除（{}）\n", plan.deletions.len()));
        for d in &plan.deletions {
            md.push_str(&format!("- [{}] {}: {}\n", d.kind, d.id, d.reason));
        }
        ctx.vfs.write_content(&uri, &md).await?;
        Ok(())
    }
}

/// 解析综述输出（LLM JSON 提取 + 结构化）。
fn parse_plan(text: &str) -> Result<EvolutionPlan> {
    let json = parse_llm_json(text)
        .ok_or_else(|| TianyanError::Custom("evolution: 综述输出中未找到 JSON".to_string()))?;
    serde_json::from_value(json)
        .map_err(|e| TianyanError::Custom(format!("evolution: 综述 JSON 结构不合法：{e}")))
}

/// 降级路径的综述摘要：取输出首个非空行并截断（保证报告/水位线里可回溯）。
fn first_line_truncated(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut out: String = first.chars().take(120).collect();
    if first.chars().count() > 120 {
        out.push('…');
    }
    out
}

/// 解析记忆类别。
fn parse_category(category: &str) -> Result<MemoryCategory> {
    match category.to_lowercase().as_str() {
        "preference" => Ok(MemoryCategory::Preference),
        "decision" => Ok(MemoryCategory::Decision),
        "fact" => Ok(MemoryCategory::Fact),
        "entity" => Ok(MemoryCategory::Entity),
        "pattern" => Ok(MemoryCategory::Pattern),
        "successful_case" | "success" => Ok(MemoryCategory::SuccessfulCase),
        "failed_case" | "failure" => Ok(MemoryCategory::FailedCase),
        other => Err(TianyanError::Custom(format!(
            "evolution: 未知记忆类别：{other}"
        ))),
    }
}

/// 采集单个命名空间的清单（L0 摘要首行；受条目上限约束）。
async fn collect_inventory_ns(
    ctx: &TaskContext,
    label: &str,
    root: &TianyanUri,
    out: &mut String,
    count: &mut usize,
    max: usize,
) {
    let entries = match ctx.vfs.list(root).await {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries {
        if *count >= max {
            break;
        }
        if entry.is_directory() {
            // 清单只列条目（规则/技能文件）；跳过归档等子目录
            continue;
        }
        let name = entry.uri().path().last().cloned().unwrap_or_default();
        let abstract_text = ctx.vfs.read_abstract(entry.uri()).await.unwrap_or_default();
        let first_line = abstract_text.lines().next().unwrap_or("").to_string();
        out.push_str(&format!("{label}/{name}: {first_line}"));
        out.push('\n');
        *count += 1;
    }
}

/// 递归采集记忆清单（叶子条目 + L0 摘要首行；运维子域跳过；受条目上限约束）。
///
/// 记忆为嵌套结构（cases/events/facts 子目录）——旧实现对 memory 仅浅列
/// 目录名，综述看不到条目明细，无法执行"同主题合并"等巩固操作。
async fn collect_memory_inventory(
    ctx: &TaskContext,
    dir: &TianyanUri,
    out: &mut String,
    count: &mut usize,
    max: usize,
    depth: usize,
) {
    if depth > 3 || *count >= max {
        return;
    }
    let entries = match ctx.vfs.list(dir).await {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries {
        if *count >= max {
            break;
        }
        if memory_paths::is_operational_path(entry.uri()) {
            continue;
        }
        if entry.is_directory() {
            Box::pin(collect_memory_inventory(
                ctx,
                entry.uri(),
                out,
                count,
                max,
                depth + 1,
            ))
            .await;
            continue;
        }
        let relative = entry.uri().path().join("/");
        let abstract_text = ctx.vfs.read_abstract(entry.uri()).await.unwrap_or_default();
        let first_line = abstract_text.lines().next().unwrap_or("").to_string();
        out.push_str(&format!("memory/{relative}: {first_line}"));
        out.push('\n');
        *count += 1;
    }
}

/// 在命名空间下按名字递归查找条目（文件或目录均可；深度上限防失控）。
///
/// 历史缺陷：此前仅匹配**目录**条目（`if !entry.is_directory() { continue; }`），
/// 而规则/技能/记忆条目在 VFS 中均为文件——演化删除通道自上线起从未命中过
/// 任何目标（"删除目标不存在，跳过"被静默吞掉），三个软删归档目录恒为空。
/// 旧测试以目录形态的 mock 条目掩盖了该缺陷（与生产数据形态不一致）。
async fn find_entry(
    ctx: &TaskContext,
    uri: &TianyanUri,
    name: &str,
    depth: usize,
) -> Result<Option<TianyanUri>> {
    if depth > 4 {
        return Ok(None);
    }
    let entries = match ctx.vfs.list(uri).await {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };
    for entry in entries {
        let entry_name = entry.uri().path().last().cloned().unwrap_or_default();
        if entry_name == name {
            return Ok(Some(entry.uri().clone()));
        }
        if entry.is_directory() {
            if let Some(found) = Box::pin(find_entry(ctx, entry.uri(), name, depth + 1)).await? {
                return Ok(Some(found));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::MockVfs;
    use crate::vfs::{ContentStore, SummaryEngine, VfsCore};

    struct FakeExecutor {
        response: String,
    }

    #[async_trait]
    impl EvolutionReviewExecutor for FakeExecutor {
        async fn review(&self, _input: &str) -> Result<String> {
            Ok(self.response.clone())
        }
    }

    fn make_context(vfs: Arc<MockVfs>) -> TaskContext {
        make_context_with_config(vfs, crate::config::TianyanConfig::default())
    }

    fn make_context_with_config(
        vfs: Arc<MockVfs>,
        config: crate::config::TianyanConfig,
    ) -> TaskContext {
        let chat: Arc<dyn crate::model::ChatService> =
            Arc::new(crate::test_utils::MockChatService::new());
        let summary_engine = Arc::new(SummaryEngine::new(chat.clone(), "test-model"));
        let memory_extractor = Arc::new(crate::memory::MemoryExtractor::new(
            chat.clone(),
            crate::memory::ExtractionConfig::default(),
        ));
        let skill_reviewer = Arc::new(crate::skills::SkillReviewer::new(
            chat,
            vfs.clone(),
            "test-model".to_string(),
        ));
        let config = Arc::new(config);
        TaskContext::new(
            vfs,
            summary_engine,
            memory_extractor,
            skill_reviewer,
            config,
        )
    }

    #[test]
    fn test_parse_plan_valid() {
        let text = r#"{"memories":[{"action":"add","category":"fact","content":"用户喜欢 Rust","importance":0.8}],"skills":[{"action":"add","id":"rust-lint","name":"Rust 检查","description":"运行 clippy","content":"步骤 1","applicable_scenarios":["Rust 项目"]}],"rules":[{"action":"add","abstract":"先测试再提交","content":"细节"}],"deletions":[{"kind":"skill","id":"old-skill","reason":"被取代"}],"summary":"本周收敛"}"#;
        let plan = parse_plan(text).unwrap();
        assert_eq!(plan.memories.len(), 1);
        assert_eq!(plan.memories[0].category, "fact");
        assert_eq!(plan.skills.len(), 1);
        assert_eq!(plan.skills[0].id, "rust-lint");
        assert_eq!(plan.rules.len(), 1);
        assert_eq!(plan.rules[0].abstract_text, "先测试再提交");
        assert_eq!(plan.deletions.len(), 1);
        assert_eq!(plan.deletions[0].kind, "skill");
        assert_eq!(plan.summary, "本周收敛");
    }

    #[test]
    fn test_parse_plan_invalid() {
        assert!(parse_plan("不是 JSON").is_err());
        assert!(parse_plan("").is_err());
    }
    #[test]
    fn test_parse_plan_merge_memory() {
        let text = r#"{"memories":[{"action":"merge","id":"theme-1","category":"failed_case","content":"归纳内容","importance":0.8,"merge_from":["a","b"]}],"summary":"巩固"}"#;
        let plan = parse_plan(text).unwrap();
        assert_eq!(plan.memories[0].action, "merge");
        assert_eq!(plan.memories[0].id.as_deref(), Some("theme-1"));
        assert_eq!(
            plan.memories[0].merge_from,
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn test_parse_category() {
        assert_eq!(
            parse_category("preference").unwrap(),
            MemoryCategory::Preference
        );
        assert_eq!(parse_category("FACT").unwrap(), MemoryCategory::Fact);
        assert!(parse_category("unknown")
            .unwrap_err()
            .to_string()
            .contains("未知"));
    }

    #[tokio::test]
    async fn test_apply_memory_writes_entry() {
        let vfs = Arc::new(MockVfs::new());
        let ctx = make_context(vfs.clone());
        let task = EvolutionTask::new(
            Arc::new(FakeExecutor {
                response: "{}".to_string(),
            }),
            10,
        );
        let m = MemoryPlan {
            action: "add".to_string(),
            category: "fact".to_string(),
            content: "用户使用 Windows 与 PowerShell".to_string(),
            importance: 0.8,
            id: Some("evo-test-fact".to_string()),
            merge_from: Vec::new(),
        };
        task.apply_memory(&ctx, &m).await.unwrap();
        let uri = MemoryCategory::Fact.default_uri("evo-test-fact");
        let content = ctx
            .vfs
            .read_content(&uri, ContentLevel::Detail)
            .await
            .unwrap();
        assert!(content.contains("用户使用 Windows 与 PowerShell"));
        let abstract_text = ctx.vfs.read_abstract(&uri).await.unwrap();
        assert!(abstract_text.contains("来源: evolution"));
    }

    #[tokio::test]
    async fn test_apply_deletion_soft_delete() {
        let vfs = Arc::new(MockVfs::new());
        let ctx = make_context(vfs.clone());
        let task = EvolutionTask::new(
            Arc::new(FakeExecutor {
                response: "{}".to_string(),
            }),
            10,
        );
        // 建技能 skill/rust-lint——**文件形态**（与生产一致：技能/规则/记忆
        // 条目在 VFS 中均为文件；旧测试用目录形态掩盖了 find_entry 缺陷）
        let skill_root = TianyanUri::new(ContextNamespace::Skill, vec![]);
        let uri = TianyanUri::new(ContextNamespace::Skill, vec!["rust-lint".to_string()]);
        vfs.create_directory(&skill_root).await.unwrap();
        vfs.add_entry(&skill_root, &uri);
        vfs.write_content(&uri, "content").await.unwrap();
        vfs.write_abstract(&uri, "abstract").await.unwrap();
        // 删除
        let d = DeletionPlan {
            kind: "skill".to_string(),
            id: "rust-lint".to_string(),
            reason: "被取代".to_string(),
        };
        task.apply_deletion(&ctx, &d).await.unwrap();
        // 原条目删除
        assert!(!ctx.vfs.exists(&uri).await.unwrap());
        // 归档副本存在
        let archive = TianyanUri::new(
            ContextNamespace::Skill,
            vec!["_archive".to_string(), "rust-lint".to_string()],
        );
        assert!(ctx.vfs.exists(&archive).await.unwrap());
    }
    #[tokio::test]
    async fn test_apply_deletion_memory_nested_file() {
        // 记忆条目为深层文件（cases/failed_tasks/{id}）——删除通道必须命中
        // 文件条目并归档到 memory/archive（回归：find_entry 仅匹配目录时必红）
        let vfs = Arc::new(MockVfs::new());
        let ctx = make_context(vfs.clone());
        let task = EvolutionTask::new(
            Arc::new(FakeExecutor {
                response: "{}".to_string(),
            }),
            10,
        );
        let mem_root = TianyanUri::new(ContextNamespace::Memory, vec![]);
        let cases = mem_root.append("cases");
        let failed = cases.append("failed_tasks");
        vfs.create_directory(&mem_root).await.unwrap();
        vfs.add_directory(&mem_root, &cases);
        vfs.add_directory(&cases, &failed);
        let entry = failed.append("old-case");
        vfs.add_entry(&failed, &entry);
        vfs.write_content(&entry, "旧案例内容").await.unwrap();
        vfs.write_abstract(&entry, "旧摘要").await.unwrap();

        let d = DeletionPlan {
            kind: "memory".to_string(),
            id: "old-case".to_string(),
            reason: "已过时".to_string(),
        };
        task.apply_deletion(&ctx, &d).await.unwrap();

        assert!(!ctx.vfs.exists(&entry).await.unwrap(), "原条目应删除");
        let archive = mem_root.append("archive").append("old-case");
        let archived = ctx
            .vfs
            .read_content(&archive, ContentLevel::Detail)
            .await
            .unwrap();
        assert_eq!(archived, "旧案例内容", "归档副本应保留内容");
    }
    #[tokio::test]
    async fn test_apply_memory_merge_updates_target_and_archives_sources() {
        let vfs = Arc::new(MockVfs::new());
        let ctx = make_context(vfs.clone());
        let task = EvolutionTask::new(
            Arc::new(FakeExecutor {
                response: "{}".to_string(),
            }),
            10,
        );
        // 两条同主题旧记忆（cases/failed_tasks 下）
        let mem_root = TianyanUri::new(ContextNamespace::Memory, vec![]);
        let cases = mem_root.append("cases");
        let failed = cases.append("failed_tasks");
        vfs.create_directory(&mem_root).await.unwrap();
        vfs.add_directory(&mem_root, &cases);
        vfs.add_directory(&cases, &failed);
        let a = failed.append("theme-a");
        let b = failed.append("theme-b");
        vfs.add_entry(&failed, &a);
        vfs.add_entry(&failed, &b);
        vfs.write_content(&a, "旧内容 A").await.unwrap();
        vfs.write_abstract(&a, "旧摘要 A").await.unwrap();
        vfs.write_content(&b, "旧内容 B").await.unwrap();
        vfs.write_abstract(&b, "旧摘要 B").await.unwrap();

        let m = MemoryPlan {
            action: "merge".to_string(),
            category: "failed_case".to_string(),
            content: "归纳后的主题内容".to_string(),
            importance: 0.9,
            id: Some("theme-a".to_string()),
            merge_from: vec!["theme-b".to_string()],
        };
        task.apply_memory_merge(&ctx, &m).await.unwrap();

        // 目标条目原地重写（不新建）
        let updated = ctx
            .vfs
            .read_content(&a, ContentLevel::Detail)
            .await
            .unwrap();
        assert!(
            updated.contains("归纳后的主题内容"),
            "目标条目应更新: {updated}"
        );
        // 被合并条目已归档 + 原条目删除
        assert!(!ctx.vfs.exists(&b).await.unwrap(), "被合并条目应移除");
        let archive = mem_root.append("archive").append("theme-b");
        let archived = ctx
            .vfs
            .read_content(&archive, ContentLevel::Detail)
            .await
            .unwrap();
        assert_eq!(archived, "旧内容 B", "归档副本应保留原内容");
    }

    #[tokio::test]
    async fn test_apply_memory_merge_missing_target_falls_back_to_add() {
        let vfs = Arc::new(MockVfs::new());
        let ctx = make_context(vfs.clone());
        let task = EvolutionTask::new(
            Arc::new(FakeExecutor {
                response: "{}".to_string(),
            }),
            10,
        );
        let m = MemoryPlan {
            action: "merge".to_string(),
            category: "fact".to_string(),
            content: "新建的归纳内容".to_string(),
            importance: 0.7,
            id: Some("brand-new".to_string()),
            merge_from: Vec::new(),
        };
        task.apply_memory_merge(&ctx, &m).await.unwrap();

        // 目标不存在 → 按类别默认路径新建（内容不丢）
        let uri = MemoryCategory::Fact.default_uri("brand-new");
        let content = ctx
            .vfs
            .read_content(&uri, ContentLevel::Detail)
            .await
            .unwrap();
        assert!(content.contains("新建的归纳内容"));
    }

    #[tokio::test]
    async fn test_merge_skipped_when_consolidation_disabled() {
        let vfs = Arc::new(MockVfs::new());
        let mut config = crate::config::TianyanConfig::default();
        config.memory.auto_consolidation = false;
        let ctx = make_context_with_config(vfs.clone(), config);
        let task = EvolutionTask::new(
            Arc::new(FakeExecutor {
                response: "{}".to_string(),
            }),
            10,
        );
        // 目标条目存在
        let mem_root = TianyanUri::new(ContextNamespace::Memory, vec![]);
        let cases = mem_root.append("cases");
        let failed = cases.append("failed_tasks");
        vfs.create_directory(&mem_root).await.unwrap();
        vfs.add_directory(&mem_root, &cases);
        vfs.add_directory(&cases, &failed);
        let a = failed.append("theme-a");
        vfs.add_entry(&failed, &a);
        vfs.write_content(&a, "旧内容 A").await.unwrap();
        vfs.write_abstract(&a, "旧摘要 A").await.unwrap();

        let plan = EvolutionPlan {
            memories: vec![MemoryPlan {
                action: "merge".to_string(),
                category: "failed_case".to_string(),
                content: "不应被写入".to_string(),
                importance: 0.9,
                id: Some("theme-a".to_string()),
                merge_from: Vec::new(),
            }],
            skills: Vec::new(),
            rules: Vec::new(),
            roles: Vec::new(),
            deletions: Vec::new(),
            summary: "test".to_string(),
        };
        let applied = task.apply_plan(&ctx, &plan).await.unwrap();
        assert_eq!(applied, 0, "auto_consolidation=false 时 merge 不应被应用");
        let content = ctx
            .vfs
            .read_content(&a, ContentLevel::Detail)
            .await
            .unwrap();
        assert!(
            content.contains("旧内容 A"),
            "目标条目不应被改动: {content}"
        );
    }

    #[tokio::test]
    async fn test_memory_inventory_recursive_and_excludes_operational() {
        let vfs = Arc::new(MockVfs::new());
        let ctx = make_context(vfs.clone());
        let mem_root = TianyanUri::new(ContextNamespace::Memory, vec![]);
        let cases = mem_root.append("cases");
        let failed = cases.append("failed_tasks");
        let events = mem_root.append("events");
        let reports = events.append("evolution_reports");
        vfs.create_directory(&mem_root).await.unwrap();
        vfs.add_directory(&mem_root, &cases);
        vfs.add_directory(&cases, &failed);
        vfs.add_directory(&mem_root, &events);
        vfs.add_directory(&events, &reports);
        let leaf = failed.append("deep-case");
        vfs.add_entry(&failed, &leaf);
        vfs.write_abstract(&leaf, "深层案例摘要首行").await.unwrap();
        let report = reports.append("r.md");
        vfs.add_entry(&reports, &report);
        vfs.write_abstract(&report, "报告摘要").await.unwrap();

        let mut out = String::new();
        let mut count = 0usize;
        collect_memory_inventory(&ctx, &mem_root, &mut out, &mut count, 200, 0).await;

        assert!(
            out.contains("memory/cases/failed_tasks/deep-case"),
            "递归应含深层叶子: {out}"
        );
        assert!(out.contains("深层案例摘要首行"));
        assert!(!out.contains("evolution_reports"), "运维子域应跳过: {out}");
        assert_eq!(count, 1);
    }

    #[test]
    fn test_build_review_input_consolidation_toggle() {
        let task = EvolutionTask::new(
            Arc::new(FakeExecutor {
                response: "{}".to_string(),
            }),
            10,
        );
        let on = task.build_review_input(&None, "memory/x: y", true);
        assert!(on.contains("merge_from"), "启用巩固时输出格式含 merge 示例");
        assert!(on.contains("记忆治理（巩固）"));
        assert!(on.contains("过程记录"), "写前原则含过程记录约束");
        let off = task.build_review_input(&None, "memory/x: y", false);
        assert!(!off.contains("merge_from"), "关闭巩固时不含 merge 指导");
        assert!(!off.contains("记忆治理（巩固）"));
    }

    #[tokio::test]
    async fn test_full_flow_with_fake_executor() {
        let vfs = Arc::new(MockVfs::new());
        let ctx = make_context(vfs.clone());
        let executor = FakeExecutor {
            response: r#"{"memories":[{"action":"add","category":"fact","content":"测试事实","importance":0.9}],"summary":"测试运行"}"#
                .to_string(),
        };
        let task = EvolutionTask::new(Arc::new(executor), 10);
        let applied = task.run_evolution(&ctx).await.unwrap();
        assert_eq!(applied, 1);
        // 演化报告目录已创建（MockVfs 文件不入 list，用 exists 验证）
        let report_root = TianyanUri::new(
            ContextNamespace::Memory,
            vec!["events".to_string(), "evolution_reports".to_string()],
        );
        assert!(ctx.vfs.exists(&report_root).await.unwrap());
        // 水位线已写
        let state = ctx.task_state.read("evolution").await.unwrap().unwrap();
        assert!(state.contains("应用变更: 1"));
    }

    #[tokio::test]
    async fn test_unparseable_review_degrades_to_empty_plan_not_failure() {
        // 回归（真实事故 2026-08-31）：综述智能体以散文回复（无可解析 JSON），
        // 旧实现整个任务失败 → 演化报告/水位线全不落盘。
        // 降级契约：空计划 + 报告照常写 + 任务成功（applied=0）+ 摘要可回溯原文。
        let vfs = Arc::new(MockVfs::new());
        let ctx = make_context(vfs.clone());
        let executor = FakeExecutor {
            response: "本周期经审查无需新增记忆或规则，现有注册表已覆盖。".to_string(),
        };
        let task = EvolutionTask::new(Arc::new(executor), 10);
        let applied = task.run_evolution(&ctx).await.unwrap();
        assert_eq!(applied, 0, "降级空计划应 0 变更且任务不失败");
        // 演化报告仍落盘
        let report_root = TianyanUri::new(
            ContextNamespace::Memory,
            vec!["events".to_string(), "evolution_reports".to_string()],
        );
        assert!(ctx.vfs.exists(&report_root).await.unwrap());
        // 水位线写入且摘要回溯了综述原文
        let state = ctx.task_state.read("evolution").await.unwrap().unwrap();
        assert!(state.contains("应用变更: 0"));
        assert!(state.contains("无需新增记忆或规则"));
    }

    #[test]
    fn test_first_line_truncated() {
        assert_eq!(
            first_line_truncated("\n  \n本周期无变更。\n尾行"),
            "本周期无变更。"
        );
        let long = "字".repeat(200);
        assert_eq!(first_line_truncated(&long).chars().count(), 121);
        assert!(first_line_truncated(&long).ends_with('…'));
        assert_eq!(first_line_truncated(""), "");
    }
}

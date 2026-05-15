//! 规则记录器。
//!
//! Harness Engineering 核心：每次智能体失败时，追加一条规则到 agent/learned/，
//! 确保同一失败永不发生第二次。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{AgentPath, ContentLevel};
use crate::storage::VirtualFileSystem;

/// 失败类型分类。
///
/// 不同失败应有不同的处理策略：
/// - Transient: 不记录（网络超时等瞬态错误）
/// - Logic: 记录为 learned rule（Planner/Executor 逻辑错误）
/// - Input: 轻量记录（用户输入模糊，标记来源）
/// - System: 记录并告警（系统级故障）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// 瞬态错误（网络超时、临时服务不可用），不应记录为规则。
    Transient,
    /// 逻辑错误（Planner 误判、Executor 执行失败），值得记录为规则。
    Logic,
    /// 用户输入错误（模糊指令、信息不足），记录但不作为失败规则。
    Input,
    /// 系统级错误（配置缺失、存储损坏），记录并升级告警。
    System,
}

impl FailureKind {
    /// 是否应该记录为 learned rule。
    pub fn should_record(&self) -> bool {
        matches!(self, FailureKind::Logic | FailureKind::System)
    }

    /// 获取失败类型的中文标签。
    pub fn label(&self) -> &'static str {
        match self {
            FailureKind::Transient => "瞬态",
            FailureKind::Logic => "逻辑",
            FailureKind::Input => "输入",
            FailureKind::System => "系统",
        }
    }
}

/// 规则记录器。
#[derive(Clone)]
pub struct RuleRecorder {
    vfs: Arc<dyn VirtualFileSystem>,
    /// 生成此规则的模型版本。
    model_version: String,
    /// 生成此规则时的 Pipeline 版本。
    pipeline_version: String,
}

/// 规则记录器默认模型版本标识。
const DEFAULT_MODEL_VERSION: &str = "unknown";
/// 规则记录器默认 Pipeline 版本标识。
const DEFAULT_PIPELINE_VERSION: &str = "1.0";

impl RuleRecorder {
    /// 创建新的规则记录器。
    pub fn new(vfs: Arc<dyn VirtualFileSystem>) -> Self {
        Self {
            vfs,
            model_version: DEFAULT_MODEL_VERSION.to_string(),
            pipeline_version: DEFAULT_PIPELINE_VERSION.to_string(),
        }
    }

    /// 设置模型版本（用于记录规则生成来源）。
    pub fn with_model_version(mut self, version: impl Into<String>) -> Self {
        self.model_version = version.into();
        self
    }

    /// 设置 Pipeline 版本（用于记录规则生成来源）。
    pub fn with_pipeline_version(mut self, version: impl Into<String>) -> Self {
        self.pipeline_version = version.into();
        self
    }

    /// 追加一条学习规则。
    ///
    /// - `abstract_text` - 规则摘要（~100 tokens），注入 system_prompt
    /// - `detail_text` - 规则详情（含溯源信息），存储在 Detail 层
    /// - `source_session` - 触发此规则的会话 ID
    pub async fn record(
        &self,
        abstract_text: &str,
        detail_text: &str,
        source_session: &str,
    ) -> Result<()> {
        self.record_with_kind(
            abstract_text,
            detail_text,
            source_session,
            FailureKind::Logic,
        )
        .await
    }

    /// 带失败类型分类的记录。
    ///
    /// Transient 失败不记录。Logic/System 失败记录为 learned rule。
    /// 写入前检查语义重复，避免相同规则多次写入。
    pub async fn record_with_kind(
        &self,
        abstract_text: &str,
        detail_text: &str,
        source_session: &str,
        kind: FailureKind,
    ) -> Result<()> {
        if !kind.should_record() {
            tracing::debug!(failure_kind = kind.label(), "跳过记录（非持久失败类型）");
            return Ok(());
        }

        // 去重检查：避免语义重复的规则重复写入
        if self.has_similar_rule(abstract_text).await? {
            tracing::info!(
                abstract_text = abstract_text,
                failure_kind = kind.label(),
                "跳过记录（已存在相似规则）"
            );
            return Ok(());
        }

        let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let rule_id = format!("rule-{}", ts);

        let uri = AgentPath::Learned.uri().append(&rule_id);

        self.vfs.create_file(&uri).await?;

        let abstract_content = format!(
            "[{}] {} (来源会话: {})",
            kind.label(),
            abstract_text,
            source_session
        );
        self.vfs.write_abstract(&uri, &abstract_content).await?;

        let full_detail = format!(
            "失败类型: {}\n来源会话: {}\n\n详情:\n{}\n\n---\n## 规则元数据\n- 生成模型: {}\n- Pipeline 版本: {}\n- 生成时间: {}\n",
            kind.label(),
            source_session,
            detail_text,
            self.model_version,
            self.pipeline_version,
            chrono::Utc::now().to_rfc3339()
        );
        self.vfs.write_content(&uri, &full_detail).await?;

        tracing::info!(
            rule_id = %uri.to_string(),
            session_id = %source_session,
            failure_kind = kind.label(),
            "已追加学习规则"
        );

        Ok(())
    }

    /// 检查 VFS 中是否已存在与 `abstract_text` 语义相似的规则。
    ///
    /// 通过比较已有规则的 Abstract 摘要与候选摘要的文本重叠度来判断。
    async fn has_similar_rule(&self, candidate_abstract: &str) -> Result<bool> {
        let learned_uri = AgentPath::Learned.uri();
        let entries = match self.vfs.list(&learned_uri).await {
            Ok(e) => e,
            Err(_) => return Ok(false),
        };

        let candidate_lower = candidate_abstract.to_lowercase();

        for entry in &entries {
            if let Ok(existing) = self
                .vfs
                .read_content(entry.uri(), ContentLevel::Abstract)
                .await
            {
                // 去掉类型标签前缀 "[逻辑]" / "[系统]"
                let existing_clean = existing
                    .trim_start_matches("[逻辑] ")
                    .trim_start_matches("[系统] ")
                    .trim_start_matches("[输入] ")
                    .to_lowercase();

                // 简单相似性检查：任一方向的包含关系或高重叠度
                if existing_clean.contains(&candidate_lower)
                    || candidate_lower.contains(&existing_clean)
                {
                    return Ok(true);
                }

                // 字符级 Jaccard 相似度近似
                let overlap = candidate_lower
                    .chars()
                    .filter(|c| existing_clean.contains(*c))
                    .count();
                let total = candidate_lower
                    .chars()
                    .count()
                    .max(existing_clean.chars().count());
                if total > 0 && (overlap as f32 / total as f32) > 0.85 {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }
}

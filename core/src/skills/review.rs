//! 技能使用复审**契约**（记录形状 + 只读端口）。
//!
//! 技能 = 方法论文档，本身无成败——评审的是**使用效果**（被采纳 / 推动任务 /
//! 用户满意），与面板的「调用频率」互补；低分技能是退役/改进信号。
//!
//! 评审**实现**（会话证据提取、评分提示词、JSON 解析、JSONL 追加与裁剪）已迁
//! `plugins/policy-review`（ADR-049 期 3-T3）。本模块只留内核与通道层共用的两样东西：
//!
//! - [`SkillReview`]：评审记录形状。它出现在 [`SkillReviewPort`] 的签名里 ⇒ 留内核
//!   （同一把尺子：`vfs::StorageBackend` / `ApprovalGate` / `ContextPort`）。
//! - [`SkillReviewPort`]：**唯一**真实调用点是
//!   `server/src/api/skills/handlers.rs`（技能统计面板读「每个技能的最新一条评审」）。
//!   方法面按调用点证据最小化到 **1 个**：只读的 `latest_review`。
//! - [`REVIEWS_PREFIX`]：评审记录目录名。它既是插件的落盘路径（
//!   `tianyan://skill/_reviews/<id>.jsonl`），也是内核技能发现时的过滤条件
//!   （`SkillManager` 不得把 `_reviews` 当成一个技能列出来）——两处必须同源，
//!   否则「评审记录出现在技能列表里」或「插件写到别处、管理器又看不到」会静默发生。
//!
//! **产生评审的路径没有进契约**：`review_conversation`（会话证据 → LLM 评分 → 落盘）
//! 今天**零生产调用者**（只有自身测试），属「未接线」而非「已废弃」，因此保留在插件的
//! `SkillReviewer` 具体类型上，不进 trait、不删、不改 mock（ADR-049 期 3-T3 遗留）。

use async_trait::async_trait;

use crate::common::error::Result;

/// 评审记录存储目录名（技能发现时过滤）。
pub const REVIEWS_PREFIX: &str = "_reviews";

/// 单次技能使用评审记录。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillReview {
    /// 评审时间（epoch 毫秒）。
    pub ts: i64,
    /// 技能 ID。
    pub skill_id: String,
    /// 来源会话。
    pub session_id: String,
    /// 质量分（0-10）。
    pub score: u8,
    /// 使用效果判定（positive / neutral / negative）。
    pub verdict: String,
    /// 用户反馈倾向（positive / negative / mixed / none）。
    pub user_feedback: String,
    /// 理由（截断）。
    pub reason: String,
    /// 证据摘要（截断）。
    pub evidence: String,
}

/// 技能使用复审端口（**只读** seam）。
///
/// 内核与通道层只需要「读某技能的最新一条评审」，因此接口只有一个方法；产生评审的
/// 完整逻辑（`review_conversation`）留在插件的具体类型上（见模块文档的「未接线」说明）。
///
/// 实现：`tianyan-policy-review` 的 `SkillReviewer`（读取
/// `tianyan://skill/_reviews/<id>.jsonl` 的最后一行），由装配层的
/// `SkillReviewFactory` 构造注入。
#[async_trait]
pub trait SkillReviewPort: Send + Sync {
    /// 某技能的最新评审（无记录 → `None`）。
    async fn latest_review(&self, skill_id: &str) -> Result<Option<SkillReview>>;
}

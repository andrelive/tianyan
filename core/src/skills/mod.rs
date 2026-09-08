//! 技能模块。
//!
//! 技能 = **方法论文档**（VFS `skill/` 命名空间），不是可执行单元：
//!
//! - **写**：GEPA 进化引擎（[`SkillLearningEngine`]）从执行历史自动生成技能，
//!   写入 VFS `skill/{id}/`（content.md + abstract.md）；`planning` 为预置技能
//!   （bootstrap 时写入），与学习技能同构。
//! - **读**：[`SkillManager`] 从 VFS 发现技能（L0 摘要）与读取内容（L2 详情）；
//!   `call_skill` 工具直接读 VFS 返回内容，由 LLM 参考后自行用工具执行。
//! - **复审**：[`SkillReviewer`] 基于会话证据对技能使用效果打分，落 VFS
//!   `skill/_reviews/<id>.jsonl`。
//!
//! 技能没有 handler、没有执行语义——文件/命令/网络等能力由 Agent 内置工具
//! 直接覆盖，技能只承载"怎么做"的方法论。

pub(crate) mod learning;
mod manager;
mod reviewer;

pub use learning::{
    ExecutionHistory, ExecutionStep, GeneratedSkill, SkillAction, SkillEvaluation,
    SkillLearningConfig, SkillLearningEngine, SkillParameter, SkillVerification,
};
pub use manager::{SkillManager, SkillSummary};
pub use reviewer::{SkillReview, SkillReviewer, REVIEWS_PREFIX};

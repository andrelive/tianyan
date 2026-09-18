//! 技能模块。
//!
//! 技能 = **方法论文档**（VFS `skill/` 命名空间），不是可执行单元：
//!
//! - **写**：技能由 bootstrap 预置（`planning`）与外部导入写入 VFS
//!   `skill/{id}/`（content.md + abstract.md）。旧“GEPA 技能自学习引擎”
//!   （`skills/learning`，~1100 行）已删除：全仓无生产消费方，且其
//!   `apply_evaluation` 的 Deprecate 分支用 `write_content` **覆盖写**——
//!   会把技能正文替换成弃用标记（销毁内容）。
//! - **读**：[`SkillManager`] 从 VFS 发现技能（L0 摘要）与读取内容（L2 详情）；
//!   `call_skill` 工具直接读 VFS 返回内容，由 LLM 参考后自行用工具执行。
//! - **复审**：[`SkillReviewer`] 基于会话证据对技能使用效果打分，落 VFS
//!   `skill/_reviews/<id>.jsonl`。
//!
//! 技能没有 handler、没有执行语义——文件/命令/网络等能力由 Agent 内置工具
//! 直接覆盖，技能只承载"怎么做"的方法论。

mod manager;
mod reviewer;

pub use manager::{SkillManager, SkillSummary};
pub use reviewer::{SkillReview, SkillReviewer, REVIEWS_PREFIX};

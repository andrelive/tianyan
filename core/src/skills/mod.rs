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
//! - **质量评审：旧范式已退役**（ADR-049 期 4c）。历史上有「技能使用复审」契约
//!   （`SkillReviewPort` 只读端口 + `SkillReview` 记录形状，评审实现曾在
//!   `plugins/policy-review`），已随「LLM 打分 + score/verdict」范式整体删除。技能
//!   质量判断改由**演化智能体**（`plugins/task-evolution`）依据「技能使用统计 +
//!   用户原话材料 + 技能清单」自主完成，不再需要预先算一个分数。残留只剩
//!   [`REVIEWS_PREFIX`]（见其文档）。
//!
//! 技能没有 handler、没有执行语义——文件/命令/网络等能力由 Agent 内置工具
//! 直接覆盖，技能只承载"怎么做"的方法论。

mod manager;

pub use manager::{SkillManager, SkillSummary};

/// 评审记录目录名（技能发现时过滤）。
///
/// **契约退役后仍保留**（ADR-049 期 4c）：历史上的评审记录曾写在这里
/// （`tianyan://skill/_reviews/<id>.jsonl`），`SkillManager` 必须继续把它排除在技能列表外
/// ——否则存量数据会在某天被当成一个叫 `_reviews` 的技能列出来。
pub const REVIEWS_PREFIX: &str = "_reviews";

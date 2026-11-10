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
//!
//! # abstract 写法（L0 契约）
//!
//! 技能清单**全量**进 system 前缀（渐进式披露的「发现」环节：模型必须知道
//! 有哪些方法论可调，否则无从查起），所以每条 abstract 的前缀成本是**常驻**的。
//! 因此 abstract 只写**一句话**：
//!
//! - **什么场景用 + 它给什么**（`场景 → 方法`），不写"本文介绍…""该指南说明…"
//!   这类元描述——那种写法既不告诉模型何时该用，还把前缀成本翻几倍；
//! - 目标 **≤ 60 字符**（一行清单）。展开说明、步骤、反模式、来源都在 L2 正文里，
//!   模型调 [`crate::skills::SkillManager::read_skill`]（`call_skill`）时才付那份成本；
//! - 例：`排查 bug / 改动没生效：先造红得起来的反馈回路再谈假设`
//!
//! **不接受"读取侧截断"来省前缀**（试过：按首句切 + 上限截断，现网 34/60 条被
//! 切在引号或括号中间，读起来比没有更难判断）。截断是把写入侧的毛病转嫁给每一次
//! 请求——源头写短才是根治。

mod manager;

pub use manager::{SkillManager, SkillSummary};
/// 技能 abstract 的字符上限（契约值，用于写入侧告警；**不做读取侧截断**）。
///
/// 技能清单**全量**进 system 前缀，每条 abstract 的前缀成本是**每轮都付**的。
/// 实测代价：现网 60 条按「描述 | 适用场景: … | 来源: evolution」拼接，合计
/// 12,526 字符（均值 208 字）≈ 6.6K tokens，占固定前缀四成——而其中真正被用到
/// 的只有 28 条。一句话（场景 + 用途）+ 来源标记 ≈ 60 字符，60 条合计约 3.6K
/// 字符 ≈ 1K tokens。
pub const SKILL_ABSTRACT_MAX_CHARS: usize = 60;

/// 评审记录目录名（技能发现时过滤）。
///
/// **契约退役后仍保留**（ADR-049 期 4c）：历史上的评审记录曾写在这里
/// （`tianyan://skill/_reviews/<id>.jsonl`），`SkillManager` 必须继续把它排除在技能列表外
/// ——否则存量数据会在某天被当成一个叫 `_reviews` 的技能列出来。
pub const REVIEWS_PREFIX: &str = "_reviews";

/// 归档目录名（技能软删除）。
///
/// 归档 = 把 `skill/{id}` 移到 `skill/_archive/{id}`：内容与向量点随
/// `move_entry` 一起迁移（可恢复），但归档子路径被
/// `system_paths::is_system_path` 排除，因此归档后不再出现在技能列表、
/// 检索结果与上下文前缀中。
pub const ARCHIVE_PREFIX: &str = "_archive";

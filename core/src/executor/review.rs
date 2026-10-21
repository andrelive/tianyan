//! 任务结果自审**契约**（LLM-as-Judge 的判定形状 + 自审器 seam）。
//!
//! 评审**实现**（判定提示词、`VERDICT:` 解析口径、故障兜底）已迁
//! `plugins/policy-review`（ADR-049 期 3-T3）。本模块只留内核必须认的那条缝：
//!
//! - [`TaskReviewer`]：后台任务完成通知（ADR-013）注入父会话前的自审入口。内核侧
//!   以 `Arc<dyn TaskReviewer>` 消费——`agent/background.rs` 的任务管理器字段与
//!   `attach_reviewer`、`agent/tool_registry/mod.rs` 的注册表注入点、`AgentBuilder`
//!   的 `with_task_reviewer`。
//! - [`Judgment`] / [`Verdict`]：出现在 trait 签名里 ⇒ 必须留内核（与
//!   `vfs::StorageBackend` / `executor::approval::ApprovalGate` 同一条边界：
//!   契约留内核、实现进插件。搬进插件会让 `tianyan-core` 反向
//!   `use tianyan_policy_review::…`，破 D2-1/D2-3）。
//!
//! **没有** `LlmTaskReviewer`：提示词文案、判定解析、故障兜底都是「可自由改动的
//! B 级内容」（ADR-049 D3）——调一句提示词、换一次评审模型不该重编内核。

use async_trait::async_trait;

/// 判断结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// 通过 — 输出表明操作成功完成。
    Pass,
    /// 失败 — 输出表明操作失败。
    Fail,
    /// 需要修改 — 输出表明操作部分成功但有问题需要修复。
    NeedsChanges {
        /// LLM 给出的修复建议。
        suggestion: String,
    },
}

impl Verdict {
    /// 是否表示成功。
    pub fn is_pass(&self) -> bool {
        matches!(self, Verdict::Pass)
    }
}

/// LLM 判断结果。
#[derive(Debug, Clone)]
pub struct Judgment {
    /// 判断结论。
    pub verdict: Verdict,
    /// 判断理由。
    pub reason: String,
}

/// 后台任务结果自审器（G4 循环内自审门）。
///
/// 后台任务完成通知（ADR-013）注入父会话前，对任务结果做轻量 LLM 自审：
/// 未通过（`NeedsChanges`/`Fail`）时在结果前标记 `[自审未通过] {reason}`，
/// 由主 agent 下一轮看到后决策（复核/重新委托）——**不自动重跑**
/// （重跑语义与成本不可控，决策权留在 LLM）。
///
/// 实现：`tianyan-policy-review` 的 `LlmTaskReviewer`（复用
/// `VERDICT: PASS|FAIL|NEEDS_CHANGES` 判定格式），由装配层的 `TaskReviewerFactory`
/// 构造注入。契约因此只有「审什么、回什么」——判据与提示词全在插件里。
#[async_trait]
pub trait TaskReviewer: Send + Sync {
    /// 自审任务结果；返回 [`Judgment`]（`verdict.is_pass()` 判定是否通过）。
    async fn review(&self, description: &str, result: &str) -> Judgment;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verdict_is_pass() {
        assert!(Verdict::Pass.is_pass());
        assert!(!Verdict::Fail.is_pass());
        assert!(!Verdict::NeedsChanges {
            suggestion: String::new()
        }
        .is_pass());
    }
}

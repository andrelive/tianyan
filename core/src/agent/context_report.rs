//! 上下文状态快照：`context_status` 工具与「低余量尾部提示」共用的**唯一**数据面。
//!
//! 为什么需要这个中间层（而不是让工具自己去读会话状态）：工具层
//! （[`crate::agent::tool_registry::ToolRegistry`]）**拿不到** Agent 的会话状态——
//! 那是 `Agent` 私有的 `Arc<RwLock<SessionState>>`。于是由 Agent 在每轮组装前算好
//! 一份快照、写进 [`crate::agent::tool_context::ToolContext`] 的共享槽位，工具只读快照。
//!
//! 这样取数口径只有一处（`Agent::build_context_report`）：占用 = 最近一次请求的
//! prompt 侧 token（与压缩判定同源，见 `StructuredMessage::prompt_side_tokens`），
//! 窗口/阈值来自策略侧的 [`CompressionBudget`]。工具不会另造第二份算法。

use crate::context::CompressionBudget;

/// 一份会话上下文快照（Agent 每轮刷新，工具只读）。
#[derive(Debug, Clone)]
pub struct ContextReport {
    /// 当前会话使用的模型名。
    pub model: String,
    /// 会话消息总条数（含压缩点）。
    pub message_count: usize,
    /// 距最近一次压缩点之后的消息条数——压缩的前置条件之一。
    pub messages_since_marker: usize,
    /// 压缩前置的**最少**消息条数（不足时"现在压也没用"）。
    pub min_messages_before_compression: usize,
    /// 策略侧给出的预算（窗口 / 占用 / 阈值 / 临界阈值）。
    pub budget: CompressionBudget,
}

impl ContextReport {
    /// 距自动压缩阈值还剩多少 token（已越过则为 0）。
    pub fn remaining_to_threshold(&self) -> u64 {
        self.budget.threshold_tokens.saturating_sub(self.budget.used_tokens)
    }

    /// 占用百分比（四舍五入；窗口为 0 时按 0 处理）。
    pub fn used_pct(&self) -> u64 {
        if self.budget.context_window == 0 {
            return 0;
        }
        ((self.budget.used_tokens as f64 / self.budget.context_window as f64) * 100.0).round() as u64
    }

    /// 现在是否可以发起压缩（消息数够了）。
    pub fn compressible(&self) -> bool {
        self.messages_since_marker >= self.min_messages_before_compression
    }

    /// 给模型看的一句结论（数字之外的人话，模型读这句比读 12 个字段快）。
    fn hint(&self) -> &'static str {
        if !self.compressible() {
            "上下文还很新（距上次压缩的消息不足），现在压缩没有可压的内容"
        } else if self.budget.is_critical {
            "已越过临界阈值：建议尽快压缩，否则下一次请求可能被上游拒绝"
        } else if self.budget.should_compress {
            "已越过自动压缩阈值：可以压缩后继续，也可以先做完当前步骤再压"
        } else {
            "上下文充裕，继续工作即可"
        }
    }

    /// 工具返回值（`context_status` 的 JSON 形态）。
    ///
    /// 字段名用 `snake_case`（与其它工具一致）；`used_tokens` / `threshold_tokens`
    /// 都是**绝对 token 数**，`used_pct` / `remaining_to_threshold` 是对应的人话版本。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "context_window": self.budget.context_window,
            "used_tokens": self.budget.used_tokens,
            "used_pct": self.used_pct(),
            "threshold_tokens": self.budget.threshold_tokens,
            "critical_tokens": self.budget.critical_tokens,
            "remaining_to_threshold": self.remaining_to_threshold(),
            "should_compress": self.budget.should_compress,
            "is_critical": self.budget.is_critical,
            "message_count": self.message_count,
            "messages_since_marker": self.messages_since_marker,
            "min_messages_before_compression": self.min_messages_before_compression,
            "compressible": self.compressible(),
            "hint": self.hint(),
        })
    }
}

#[cfg(test)]
mod tests {
    //! 口径单测：占用百分比、余量、可压缩前置、hint 分档。
    use super::*;

    fn report(used: u64, window: u64, threshold: u64, critical: u64, since: usize) -> ContextReport {
        ContextReport {
            model: "test-model".to_string(),
            message_count: since,
            messages_since_marker: since,
            min_messages_before_compression: 6,
            budget: CompressionBudget {
                context_window: window,
                used_tokens: used,
                threshold_tokens: threshold,
                critical_tokens: critical,
                should_compress: used > threshold,
                is_critical: used > critical,
            },
        }
    }

    #[test]
    fn pct_and_remaining_are_consistent() {
        let r = report(200_000, 1_000_000, 600_000, 800_000, 20);
        assert_eq!(r.used_pct(), 20);
        assert_eq!(r.remaining_to_threshold(), 400_000);
        assert!(!r.budget.should_compress);
    }

    #[test]
    fn remaining_saturates_after_threshold() {
        let r = report(700_000, 1_000_000, 600_000, 800_000, 20);
        assert_eq!(r.remaining_to_threshold(), 0, "越过阈值后余量归零，不给负数");
        assert!(r.budget.should_compress);
        assert!(!r.budget.is_critical);
    }

    #[test]
    fn zero_window_does_not_divide_by_zero() {
        let r = report(0, 0, 0, 0, 0);
        assert_eq!(r.used_pct(), 0);
    }

    #[test]
    fn compressible_needs_enough_messages() {
        assert!(!report(0, 1000, 600, 800, 5).compressible());
        assert!(report(0, 1000, 600, 800, 6).compressible());
    }

    #[test]
    fn hint_ranks_by_state() {
        // 消息太少优先（此时压缩没内容可压）
        assert!(report(900_000, 1_000_000, 600_000, 800_000, 2).hint().contains("很新"));
        // 临界优先于普通阈值
        assert!(report(900_000, 1_000_000, 600_000, 800_000, 20).hint().contains("临界"));
        assert!(report(700_000, 1_000_000, 600_000, 800_000, 20).hint().contains("自动压缩阈值"));
        assert!(report(1000, 1_000_000, 600_000, 800_000, 20).hint().contains("充裕"));
    }

    #[test]
    fn json_carries_both_numbers_and_hint() {
        let v = report(200_000, 1_000_000, 600_000, 800_000, 20).to_json();
        assert_eq!(v["used_pct"], 20);
        assert_eq!(v["remaining_to_threshold"], 400_000);
        assert_eq!(v["compressible"], true);
        assert!(v["hint"].as_str().is_some());
        assert!(v["model"].as_str().is_some());
    }
}

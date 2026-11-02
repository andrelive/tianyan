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
    /// 生成这份快照的时刻（本地时区，`YYYY-MM-DD HH:MM:SS +08:00`）。
    ///
    /// 为什么塞在这里：模型对「现在几点」没有内建概念（系统提示里不写时间——
    /// 那会让**每轮前缀都变**，直接打掉 DeepSeek 的前缀缓存），而它做时间相关判断
    /// （今天几号 / 这个文件的时间戳算不算旧 / 任务跑了多久）时又需要它。
    /// 挂在工具返回上：按需取、零缓存代价。
    pub now: String,
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
        self.budget
            .threshold_tokens
            .saturating_sub(self.budget.used_tokens)
    }

    /// 距**窗口上限**还剩多少 token（已越过则为 0）。
    ///
    /// 与 [`Self::remaining_to_threshold`] 的区别：阈值是**系统自动兜底**的策略线
    /// （且只在轮收尾时生效），窗口上限是**物理约束**（撞上去请求就发不出去）。
    /// 模型在轮内（连续多步工具）能依靠的只有后者，所以两个数字都给、含义分开写。
    pub fn remaining_window(&self) -> u64 {
        self.budget
            .context_window
            .saturating_sub(self.budget.used_tokens)
    }

    /// 占用百分比（四舍五入；窗口为 0 时按 0 处理）。
    pub fn used_pct(&self) -> u64 {
        if self.budget.context_window == 0 {
            return 0;
        }
        ((self.budget.used_tokens as f64 / self.budget.context_window as f64) * 100.0).round()
            as u64
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
            "已越过临界阈值：建议尽快压缩（系统只在本轮收尾兜底，轮内不会触发）"
        } else if self.budget.should_compress {
            "已越过自动压缩阈值：系统会在本轮收尾时兜底压缩；你先做完当前步骤再压也可以"
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
            "now": self.now,
            "context_window": self.budget.context_window,
            "used_tokens": self.budget.used_tokens,
            "used_pct": self.used_pct(),
            "remaining_window": self.remaining_window(),
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

/// 「低余量」尾部提示的文案（`Agent::low_remaining_note` 用）。
///
/// ## 只给两个数：窗口占比 + 距窗口上限的绝对余量
///
/// **不再提「自动压缩阈值」**。它是**系统自动兜底**的策略参数，而且**只在轮收尾时
/// 生效**——一轮里连续跑很多步（工具链）时它根本不会触发。把它报给模型有两个坏处：
///
/// 1. **口径打架**：早先的文案是「已用到自动压缩阈值的 {pct}%」（分母 = 阈值 = 窗口
///    的 60%），模型转述时把「阈值」这个限定词丢掉、说成「上下文已用到 89%」，而界面
///    上下文环显示 54%（分母 = 窗口）——同一份占用两个数（实测 used 536,891：
///    54% vs 89%），用户直接质疑「才 54% 怎么就到 89%」。
/// 2. **给出错误的依赖**：模型看到「距阈值还差 63K」会以为「还有系统兜底」，而在
///    轮内（它真正会撞墙的时段）兜底并不存在。
///
/// 所以这里只报**物理事实**（窗口占用 + 余量），并明确告知兜底的时机；要不要提前
/// 压缩由模型自己判断（ADR-053：判断权归模型，系统只兜底）。
pub fn low_remaining_note_text(
    budget: &CompressionBudget,
    since_marker: usize,
    min_messages: usize,
) -> String {
    let window = budget.context_window.max(1);
    let pct = (budget.used_tokens as f64 / window as f64 * 100.0).round() as u64;
    let remaining = budget.context_window.saturating_sub(budget.used_tokens);
    let mut note = format!(
        "【上下文提示】上下文窗口已用 {pct}%（约 {} / {} token），距窗口上限还剩 {}。\
         系统的自动压缩**只在本轮收尾时**兜底（轮内不会触发）——这一轮如果还要跑很多步，\
         余量得你自己盯。任务还长可以调 compress_context 压缩并继续（handoff 写清接下来\
         要做什么）；不要为了省上下文而缩小工作范围。",
        budget.used_tokens, budget.context_window, remaining
    );
    if since_marker < min_messages {
        note.push_str(&format!(
            "（注意：距上次压缩只有 {since_marker} 条消息，压缩可能因内容太少而被跳过。）"
        ));
    }
    note
}

#[cfg(test)]
mod tests {
    //! 口径单测：占用百分比、余量、可压缩前置、hint 分档。
    use super::*;

    fn report(
        used: u64,
        window: u64,
        threshold: u64,
        critical: u64,
        since: usize,
    ) -> ContextReport {
        ContextReport {
            model: "test-model".to_string(),
            now: "2026-11-01 20:00:00 +08:00".to_string(),
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
        assert_eq!(
            r.remaining_to_threshold(),
            0,
            "越过阈值后余量归零，不给负数"
        );
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
        assert!(report(900_000, 1_000_000, 600_000, 800_000, 2)
            .hint()
            .contains("很新"));
        // 临界优先于普通阈值
        assert!(report(900_000, 1_000_000, 600_000, 800_000, 20)
            .hint()
            .contains("临界"));
        assert!(report(700_000, 1_000_000, 600_000, 800_000, 20)
            .hint()
            .contains("自动压缩阈值"));
        assert!(report(1000, 1_000_000, 600_000, 800_000, 20)
            .hint()
            .contains("充裕"));
    }

    #[test]
    fn json_carries_both_numbers_and_hint() {
        let v = report(200_000, 1_000_000, 600_000, 800_000, 20).to_json();
        assert_eq!(v["used_pct"], 20);
        assert_eq!(v["remaining_to_threshold"], 400_000);
        assert_eq!(
            v["remaining_window"], 800_000,
            "窗口余量是物理约束（1,000,000 − 200,000）——轮内唯一能依靠的数字"
        );
        assert_eq!(v["compressible"], true);
        assert_eq!(v["now"], "2026-11-01 20:00:00 +08:00", "当前时间随快照带出");
        assert!(v["hint"].as_str().is_some());
        assert!(v["model"].as_str().is_some());
    }

    /// 回归（用户实测：「这个会话才 54% 的上下文，怎么就到了 89%？」）。
    ///
    /// 事故数字：窗口 1,000,000 / 阈值 600,000 / 实测占用 536,891。
    /// 窗口占比 54%（界面环同源）、阈值占比 89%（旧文案的分母）—— 旧文案让模型
    /// 说成「上下文已用到 89%」，与环矛盾。现在文案只给窗口口径与窗口余量。
    #[test]
    fn low_remaining_note_uses_window_pct_only() {
        let b = CompressionBudget {
            context_window: 1_000_000,
            used_tokens: 536_891,
            threshold_tokens: 600_000,
            critical_tokens: 800_000,
            should_compress: false,
            is_critical: false,
        };
        let note = low_remaining_note_text(&b, 20, 6);
        assert!(
            note.contains("窗口已用 54%"),
            "百分比必须是窗口口径（与界面环一致）：{note}"
        );
        assert!(
            !note.contains("89%"),
            "不能再出现「阈值的 89%」——模型转述时会丢掉限定词：{note}"
        );
        assert!(
            note.contains("还剩 463109"),
            "余量按**窗口**算（1,000,000 − 536,891）：{note}"
        );
        assert!(
            !note.contains("自动压缩阈值 600000"),
            "阈值是系统兜底参数（且只在轮收尾生效），不该报给模型当依据：{note}"
        );
        assert!(
            note.contains("只在本轮收尾时"),
            "必须告知兜底时机——轮内不会触发，模型不能依赖它：{note}"
        );
    }

    /// 余量吃紧时口径不变（仍是窗口占比 + 绝对余量），不给第二个百分比、也不给阈值。
    #[test]
    fn low_remaining_note_keeps_single_pct_when_tight() {
        let b = CompressionBudget {
            context_window: 1_000_000,
            used_tokens: 960_000,
            threshold_tokens: 600_000,
            critical_tokens: 800_000,
            should_compress: true,
            is_critical: true,
        };
        let note = low_remaining_note_text(&b, 20, 6);
        assert!(note.contains("窗口已用 96%"), "{note}");
        assert!(note.contains("还剩 40000"), "{note}");
        assert!(
            !note.contains("阈值 600000"),
            "余量吃紧也不报阈值（口径仍是窗口）：{note}"
        );
    }
}

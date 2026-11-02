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
    /// 轮内**实时**占用（每步 LLM 返回后由 `AgentLoop` 写入；`None` = 还没有本轮实测）。
    ///
    /// 为什么需要它：这份快照本身只在**轮边界**刷新（用户轮组装后 / 唤醒轮前 / 压缩后），
    /// 而占用几乎全部增长在轮内（多步工具）。不带上实时值，模型在轮内问
    /// `context_status` 拿到的永远是本轮起点的数字——它据此判断"还很宽裕"，
    /// 然后一路涨到请求发不出去。
    pub live_used_tokens: Option<u64>,
}

impl ContextReport {
    /// 本次请求实际应报给模型的占用：优先轮内实时值，没有则用轮边界快照。
    pub fn effective_used(&self) -> u64 {
        self.live_used_tokens.unwrap_or(self.budget.used_tokens)
    }

    /// 距自动压缩阈值还剩多少 token（已越过则为 0）。
    pub fn remaining_to_threshold(&self) -> u64 {
        self.budget
            .threshold_tokens
            .saturating_sub(self.effective_used())
    }

    /// 距**窗口上限**还剩多少 token（已越过则为 0）。
    ///
    /// 与 [`Self::remaining_to_threshold`] 的区别：阈值是**系统自动兜底**的策略线
    /// （且只在轮收尾时生效），窗口上限是**物理约束**（撞上去请求就发不出去）。
    /// 模型在轮内（连续多步工具）能依靠的只有后者，所以两个数字都给、含义分开写。
    pub fn remaining_window(&self) -> u64 {
        self.budget
            .context_window
            .saturating_sub(self.effective_used())
    }

    /// 占用百分比（四舍五入；窗口为 0 时按 0 处理）。
    pub fn used_pct(&self) -> u64 {
        if self.budget.context_window == 0 {
            return 0;
        }
        ((self.effective_used() as f64 / self.budget.context_window as f64) * 100.0).round() as u64
    }

    /// 是否已越过自动压缩阈值（按**实时**占用判定）。
    pub fn should_compress(&self) -> bool {
        self.effective_used() > self.budget.threshold_tokens
    }

    /// 是否已越过临界阈值（按**实时**占用判定）。
    pub fn is_critical(&self) -> bool {
        self.effective_used() > self.budget.critical_tokens
    }

    /// 现在是否可以发起压缩（消息数够了）。
    pub fn compressible(&self) -> bool {
        self.messages_since_marker >= self.min_messages_before_compression
    }

    /// 给模型看的一句结论（数字之外的人话，模型读这句比读 12 个字段快）。
    fn hint(&self) -> &'static str {
        if !self.compressible() {
            "上下文还很新（距上次压缩的消息不足），现在压缩没有可压的内容"
        } else if self.is_critical() {
            "已越过临界阈值：建议尽快压缩（系统只在本轮收尾兜底，轮内不会触发）"
        } else if self.should_compress() {
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
            "used_tokens": self.effective_used(),
            "used_pct": self.used_pct(),
            "remaining_window": self.remaining_window(),
            "threshold_tokens": self.budget.threshold_tokens,
            "critical_tokens": self.budget.critical_tokens,
            "remaining_to_threshold": self.remaining_to_threshold(),
            "should_compress": self.should_compress(),
            "is_critical": self.is_critical(),
            "message_count": self.message_count,
            "messages_since_marker": self.messages_since_marker,
            "min_messages_before_compression": self.min_messages_before_compression,
            "compressible": self.compressible(),
            "hint": self.hint(),
        })
    }
}

/// 轮内跨过提示线时追加给模型的那条提示的文案。
///
/// ## 为什么是「跨线时一次」而不是「每步刷新数字」
///
/// 占用的增长全部发生在**轮内**（多步工具），而轮内既不触发自动压缩、轮起点组装的
/// 那份尾部提示也不再更新。模型需要的不是连续数值，而是「现在该决定要不要压缩了」这个
/// **信号**；每步灌一遍数字既占 token，也会把历史刷成一堆重复文本。
///
/// ## 为什么只讲窗口口径、不提「自动压缩阈值」
///
/// 1. **口径打架**：早先的文案是「已用到自动压缩阈值的 {pct}%」（分母 = 阈值 = 窗口的
///    60%），模型转述时把限定词丢掉、说成「上下文已用到 89%」，而界面环显示 54%
///    （分母 = 窗口）——同一份占用两个数（实测 used 536,891：54% vs 89%）。
/// 2. **错误的依赖**：阈值只在轮收尾生效；轮内报它，模型会以为「还有系统兜底」，
///    而这正是它真正会撞墙的时段。
///
/// 文案里保留「兜底只在本轮收尾」这句：它是**能力边界**（不是数字），让模型知道
/// 轮内只有它自己能管。
pub fn threshold_notice_text(used: u64, window: u64, remaining: u64) -> String {
    let pct = if window == 0 {
        0
    } else {
        (used as f64 / window as f64 * 100.0).round() as u64
    };
    format!(
        "【上下文提示】上下文窗口已用 {pct}%（约 {used} / {window} token），距窗口上限还剩 {remaining}。\
         建议现在调用 compress_context 压缩并继续（handoff 写清接下来要做什么）——\
         系统的自动压缩只在本轮收尾时兜底，轮内不会触发。"
    )
}

/// 跨线判定（`lines` = 升序的**绝对 token** 提示线；`notified` 与之一一对应）。
///
/// 返回首个「已越过且尚未提示」的**最高档**下标，并把该档及以下一并标记为已提示
/// （一步跨两档时只补一条提示，不连发两条）。`None` = 本次不用提示。
pub fn crossed_notice_line(used: u64, lines: &[u64], notified: &mut [bool]) -> Option<usize> {
    for ix in (0..lines.len()).rev() {
        if used >= lines[ix] && !notified[ix] {
            for slot in notified.iter_mut().take(ix + 1) {
                *slot = true;
            }
            return Some(ix);
        }
    }
    None
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
            live_used_tokens: None,
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
    /// 事故数字：窗口 1,000,000 / 实测占用 536,891。窗口占比 54%（界面环同源）；
    /// 旧文案用「阈值 600,000」作分母 → 89%，模型转述成「上下文已用到 89%」。
    /// 现在文案只给窗口口径 + 距上限余量，并点明兜底时机。
    #[test]
    fn threshold_notice_uses_window_pct_only() {
        let note = threshold_notice_text(536_891, 1_000_000, 463_109);
        assert!(
            note.contains("窗口已用 54%"),
            "百分比必须是窗口口径（与界面环一致）：{note}"
        );
        assert!(
            !note.contains("89%"),
            "不能再出现「阈值的 89%」——模型转述时会丢掉限定词：{note}"
        );
        assert!(note.contains("还剩 463109"), "余量按窗口算：{note}");
        assert!(
            !note.contains("自动压缩阈值 600000"),
            "阈值是系统兜底参数（且只在轮收尾生效），不该报给模型当依据：{note}"
        );
        assert!(
            note.contains("只在本轮收尾时"),
            "必须告知兜底时机——轮内不会触发，模型不能依赖它：{note}"
        );
    }

    /// 跨线判定：每档每轮最多提示一次；一步跨两档只补一条（最高档）。
    #[test]
    fn crossed_notice_line_fires_each_tier_once() {
        let lines = [600_000u64, 800_000u64];
        let mut notified = [false, false];
        // 未跨线
        assert_eq!(crossed_notice_line(500_000, &lines, &mut notified), None);
        // 跨第一档 → 提示一次
        assert_eq!(crossed_notice_line(620_000, &lines, &mut notified), Some(0));
        // 同档重复 → 不再提示
        assert_eq!(crossed_notice_line(640_000, &lines, &mut notified), None);
        // 跨第二档 → 提示一次
        assert_eq!(crossed_notice_line(810_000, &lines, &mut notified), Some(1));
        assert_eq!(crossed_notice_line(900_000, &lines, &mut notified), None);

        // 一步从线下直接跨两档：只提示最高档，低档一并标记（不连发两条）
        let mut fresh = [false, false];
        assert_eq!(crossed_notice_line(850_000, &lines, &mut fresh), Some(1));
        assert_eq!(
            fresh,
            [true, true],
            "一步跨两档时低档也要标记，否则下一步回落到低档区间会补发一条"
        );
        assert_eq!(crossed_notice_line(700_000, &lines, &mut fresh), None);
    }
}

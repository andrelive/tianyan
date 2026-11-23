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

    /// 是否已**接近**自动压缩线（距阈值不足窗口的 5%，且尚未越线）。
    ///
    /// 为什么需要这一档：`used_pct` 是四舍五入的，`used = 597584 / 窗口 1,000,000`
    /// 会显示成 **60**，而 `should_compress` 用的是严格大于（59.76% < 60% ⇒ 未越线）。
    /// 于是同一份返回值同时给出「显示 60%」与「上下文充裕」，模型只能自己编一句话
    /// ——实测它编的是「距压缩阈值仅剩 2.4k token」（2026-11-23 会话
    /// `session-18e63d113a5aab90`：`remaining_to_threshold` 2416）。这一档的作用
    /// 就是把那段状态如实说出来：还没越线，但马上就碰。
    pub fn approaching_threshold(&self) -> bool {
        let margin = self.budget.context_window / 20; // 窗口的 5%
        let left = self
            .budget
            .threshold_tokens
            .saturating_sub(self.effective_used());
        !self.should_compress() && left <= margin
    }

    /// 给模型看的一句结论（数字之外的人话，模型读这句比读 12 个字段快）。
    ///
    /// **不复述百分比**：`used_pct` 已经在上面的字段里，两处各说一遍必然打架
    /// （四舍五入的 60 vs 严格判定的未越线，见 [`Self::approaching_threshold`]）。
    fn hint(&self) -> String {
        if !self.compressible() {
            format!(
                "距上次压缩的消息不足 {} 条，现在压也没有可压的内容",
                self.min_messages_before_compression
            )
        } else if self.is_critical() {
            "窗口余量已不足 20%：尽快压缩".to_string()
        } else if self.should_compress() {
            "已越过自动压缩线：系统会在本轮收尾兜底，你先收尾手上这步再压也可以".to_string()
        } else if self.approaching_threshold() {
            "即将触及自动压缩线：不必现在压，但别开新的长任务".to_string()
        } else {
            "上下文充裕，继续工作即可".to_string()
        }
    }

    /// 工具返回值（`context_status` 的 JSON 形态）。
    ///
    /// 字段名用 `snake_case`（与其它工具一致）。
    ///
    /// ## 只留「模型能据以行动」的量
    ///
    /// 刻意**不再**返回 `threshold_tokens` / `critical_tokens` /
    /// `remaining_to_threshold` / `should_compress` / `is_critical`：
    ///
    /// 1. 模型对它们**无法行动**——阈值是系统在**轮收尾**兜底的判据，模型既不能
    ///    触发它、也不知道收尾何时到，读到只能推出「还有系统兜底」，而这正是
    ///    [`context_notice_text`] 注释里写过的**错误依赖**
    ///    （「轮内报它，模型会以为还有系统兜底，而这正是它真正会撞墙的时段」）。
    ///    那段论证当初只落在提示文案上，工具返回值一直把五个字段全给模型 ——
    ///    同一个坑一半修了、一半留着。
    /// 2. `remaining_to_threshold` 天生会被转述成「总余量」：实测模型说的是
    ///    「距压缩阈值仅剩 2.4k token」（而 `remaining_window` 是 402416），
    ///    读的人以为上下文快没了。同一个 `54% / 89%` 事故的翻版。
    ///
    /// 留下的都是行动依据：窗口/占用/占比（与界面上下文环同口径）、距**窗口上限**
    /// 的物理余量、以及「现在压有没有意义」（消息条数 + `compressible`）。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "now": self.now,
            "context_window": self.budget.context_window,
            "used_tokens": self.effective_used(),
            "used_pct": self.used_pct(),
            "remaining_window": self.remaining_window(),
            "message_count": self.message_count,
            "messages_since_marker": self.messages_since_marker,
            "min_messages_before_compression": self.min_messages_before_compression,
            "compressible": self.compressible(),
            "hint": self.hint(),
        })
    }
}

/// 提示档位：60% 线（余量信息）与 80% 线（紧张区）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeTier {
    /// 越过自动压缩线（窗口 × 0.6）：只报余量，并明确「不是收尾信号」。
    Approaching,
    /// 越过临界线（窗口 × 0.8）：要求在**本步之后**先压一次，并说清撞上限的后果。
    Critical,
}

/// 轮内跨过提示线时追加给模型的那条提示的文案（两档措辞不同）。
///
/// ## 为什么是「跨线时一次」而不是「每步刷新数字」
///
/// 占用的增长全部发生在**轮内**（多步工具），而轮内既不触发自动压缩、轮起点组装的
/// 那份尾部提示也不再更新。模型需要的不是连续数值，而是「现在该决定要不要压缩了」这个
/// **信号**；每步灌一遍数字既占 token，也会把历史刷成一堆重复文本。
///
/// ## 两档只差三件事
///
/// 1. 60% 档带一句免责：**这不是收尾信号**，手上这一步照常做完；
/// 2. 80% 档要求**本步工具结果出来后先压一次**，并明确别开新的长任务；
/// 3. 80% 档说清后果：撞上窗口上限，请求会直接发不出去。
///
/// ## 刻意不写的两样
///
/// - **怎么压**（调 `compress_context` / handoff 写什么）：那是 `context_status` 与
///   `compress_context` 两个工具描述里已有的内容，提示里再说一遍只占 token，还会把
///   「一段信息」变成「一道指令」—— 用户报的「像在催着赶紧干完」，原话正是旧文案
///   里的「建议现在调用 compress_context 压缩并继续」。
/// - **阈值口径的数字**：理由见 [`ContextReport::to_json`] 的字段面说明（同一份占用
///   不许有两个分母，`54% / 89%` 事故有单测锁着）。
///
/// 保留「系统只在本轮收尾兜底」这句：它是**能力边界**（轮内没人替你压），不是数字，
/// 模型据此知道轮内只有它自己能管。
pub fn context_notice_text(tier: NoticeTier, used: u64, window: u64, remaining: u64) -> String {
    let pct = if window == 0 {
        0
    } else {
        (used as f64 / window as f64 * 100.0).round() as u64
    };
    match tier {
        NoticeTier::Approaching => format!(
            "【上下文提示】已用 {used} / {window}（{pct}%），距上限还剩 {remaining}。\
             这是余量信息，不是收尾信号：手上这一步照常做完。\
             系统的自动压缩只在本轮收尾兜底，轮内不会触发。"
        ),
        NoticeTier::Critical => format!(
            "【上下文提示】已用 {used} / {window}（{pct}%），距上限只剩 {remaining}，\
             已进入紧张区：本步工具结果出来后先压一次再继续，不要再开新的长任务——\
             撞上上限请求会直接发不出去。系统的自动压缩只在本轮收尾兜底，轮内不会触发。"
        ),
    }
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
        // ① 消息太少优先（此时压缩没内容可压）
        assert!(report(900_000, 1_000_000, 600_000, 800_000, 2)
            .hint()
            .contains("不足 6 条"));
        // ② 临界（> 80%）
        assert!(report(900_000, 1_000_000, 600_000, 800_000, 20)
            .hint()
            .contains("不足 20%"));
        // ③ 已越自动压缩线（> 60%）
        assert!(report(700_000, 1_000_000, 600_000, 800_000, 20)
            .hint()
            .contains("已越过自动压缩线"));
        // ④ 即将触及（未越线，但距阈值不足窗口 5%）——本档专门解决
        //    「used_pct 四舍五入显示 60 / 判定未越线」那一段的措辞真空
        let near = report(597_584, 1_000_000, 600_000, 800_000, 20);
        assert!(!near.should_compress(), "597584 未越 60 万阈值");
        assert_eq!(near.used_pct(), 60, "四舍五入后显示 60%");
        assert!(near.approaching_threshold(), "距阈值 2416 = 窗口 0.24%");
        assert!(
            near.hint().contains("即将触及自动压缩线"),
            "不能说「充裕」——实测模型据此说出「只剩 2.4k token」：{}",
            near.hint()
        );
        // ⑤ 充裕（远离阈值）
        assert!(report(1000, 1_000_000, 600_000, 800_000, 20)
            .hint()
            .contains("充裕"));
    }

    /// 「接近」这一档的边界：距阈值 ≤ 窗口 5% 才算；越线后归「已越过」档。
    #[test]
    fn approaching_threshold_boundary() {
        // 距阈值恰为窗口 5%（600000 − 550000 = 50000）：算接近
        assert!(report(550_000, 1_000_000, 600_000, 800_000, 20).approaching_threshold());
        // 距阈值 6%（540000）：不算
        assert!(!report(540_000, 1_000_000, 600_000, 800_000, 20).approaching_threshold());
        // 已越线：不算「接近」（它属于更强的「已越过」档）
        assert!(!report(620_000, 1_000_000, 600_000, 800_000, 20).approaching_threshold());
    }

    #[test]
    fn json_keeps_only_actionable_fields() {
        let v = report(200_000, 1_000_000, 600_000, 800_000, 20).to_json();
        assert_eq!(v["used_pct"], 20);
        assert_eq!(
            v["remaining_window"], 800_000,
            "窗口余量是物理约束（1,000,000 − 200,000）——轮内唯一能依靠的数字"
        );
        assert_eq!(v["compressible"], true);
        assert_eq!(v["now"], "2026-11-01 20:00:00 +08:00", "当前时间随快照带出");
        assert!(v["hint"].as_str().is_some());
        assert!(v["model"].as_str().is_some());
        // 反向断言：五个「系统策略量」必须**不再出现**。
        //
        // 判据来自实测：模型把 `remaining_to_threshold`（2416）转述成「距压缩阈值仅剩
        // 2.4k token」，而窗口余量其实是 402416 —— 读的人以为上下文快没了。
        for gone in [
            "threshold_tokens",
            "critical_tokens",
            "remaining_to_threshold",
            "should_compress",
            "is_critical",
        ] {
            assert!(
                v.get(gone).is_none(),
                "`{gone}` 不该出现在给模型的返回值里（模型无法据此行动，且会被误读成总余量）"
            );
        }
    }

    /// 回归（用户实测：「这个会话才 54% 的上下文，怎么就到了 89%？」）。
    ///
    /// 事故数字：窗口 1,000,000 / 实测占用 536,891。窗口占比 54%（界面环同源）；
    /// 旧文案用「阈值 600,000」作分母 → 89%，模型转述成「上下文已用到 89%」。
    /// 现在文案只给窗口口径 + 距上限余量，并点明兜底时机。
    #[test]
    fn approaching_notice_reports_window_pct_only() {
        let note = context_notice_text(NoticeTier::Approaching, 536_891, 1_000_000, 463_109);
        assert!(
            note.contains("（54%）"),
            "百分比必须是窗口口径（与界面环一致）：{note}"
        );
        assert!(
            !note.contains("89%"),
            "不能再出现「阈值的 89%」——模型转述时会丢掉限定词：{note}"
        );
        assert!(note.contains("还剩 463109"), "余量按窗口算：{note}");
        assert!(
            !note.contains("600000"),
            "阈值是系统兜底参数（且只在轮收尾生效），不该报给模型当依据：{note}"
        );
        assert!(
            note.contains("只在本轮收尾兜底"),
            "必须告知兜底时机——轮内不会触发，模型不能依赖它：{note}"
        );
        // 60% 档的免责句 + **不写怎么压**（工具描述里已有，写进来就成了催促）
        assert!(
            note.contains("不是收尾信号"),
            "60% 档必须明确「不是让你收尾」：{note}"
        );
        assert!(
            !note.contains("compress_context") && !note.contains("handoff"),
            "「调什么工具 / handoff 写什么」属于工具描述，提示里不重复：{note}"
        );
    }

    /// 80% 档：动作升级为「本步之后先压一次」，并说清撞上限的后果。
    #[test]
    fn critical_notice_asks_to_compress_after_this_step() {
        let note = context_notice_text(NoticeTier::Critical, 820_000, 1_000_000, 180_000);
        assert!(note.contains("（82%）"), "窗口口径：{note}");
        assert!(note.contains("只剩 180000"), "余量：{note}");
        assert!(note.contains("紧张区"), "要说明已进入紧张区：{note}");
        assert!(
            note.contains("本步工具结果出来后先压一次"),
            "80% 档要给出明确的先后顺序（做完这一步再压，不是立刻中断）：{note}"
        );
        assert!(
            note.contains("发不出去"),
            "要说清撞上限的后果，否则模型不知道有多紧：{note}"
        );
        assert!(
            !note.contains("不是收尾信号"),
            "80% 档不再需要免责句（这时确实该先腾空间）：{note}"
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

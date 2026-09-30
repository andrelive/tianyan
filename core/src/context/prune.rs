//! 会话内 prune（ADR-048）：老工具输出降级。
//!
//! **问题**：工具结果进入会话链后长期占位——即便"结论已经被后续 assistant
//! 消息消化"，原文仍每轮随请求发送，直到触发（昂贵且更有损的）LLM 摘要压缩。
//!
//! **做法**：组装视图变换（与 [`super::assembler`] 的 `normalize_tool_pairs`
//! 同层、同纪律：只作用于请求视图，库与缓存不动）——保留**最近**
//! [`PruneConfig::protect_tokens`] 的工具输出完整，更老的裁到
//! [`PruneConfig::max_chars`] 字符 + 裁剪标记。
//!
//! **与 ①（ADR-047 工具输出 spill）的分工**：
//! - spill 管"**单次**结果过大"（落盘 + 路径，可回读完整内容）；
//! - prune 管"**老**结果长期占位"（就地降级；原文仍在存储与落盘文件里，
//!   需要时重新调用工具即可）。
//!
//! **确定性**：判定完全由链内容决定（token 预算），同一链 → 同一视图 →
//! 前缀缓存可预测。代价是"跨过受保护边界的那一轮"会更新前缀（一次缓存
//! 未命中），因此只在**收益足够**（省下 ≥ [`PruneConfig::min_tokens`]）时
//! 才改写——参照实现同款纪律（opencode `PRUNE_MINIMUM`）。

use std::borrow::Cow;
use std::collections::HashMap;

use crate::common::token_estimator::estimate_tokens;
use crate::common::types::{Part, StructuredMessage};

/// 裁剪标记（幂等判据：已含标记的内容不再裁）。
pub const PRUNE_MARKER: &str = "\n\n…[该工具输出较早，已裁剪；如需完整内容请重新调用该工具]";

/// 受保护工具（其结果不裁）。
///
/// `call_skill`：技能方法论文档——调用后可能被反复参考（步骤/纪律），
/// 裁掉会直接导致行为退化（对齐 opencode 的 `PRUNE_PROTECTED_TOOLS = ["skill"]`）。
pub const PROTECTED_TOOLS: &[&str] = &["call_skill"];

/// prune 配置（由 `[tool_output]` 的 `prune_*` 字段映射，见 `agent::builder`）。
#[derive(Debug, Clone)]
pub struct PruneConfig {
    /// 总开关。
    pub enabled: bool,
    /// 保留最近 N token 的工具输出完整（更老的进入可裁区）。
    pub protect_tokens: usize,
    /// 修剪收益阈值：省下的 token 少于此值则不改写（保持前缀稳定）。
    pub min_tokens: usize,
    /// 单条被裁的老工具输出保留字符数。
    pub max_chars: usize,
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            protect_tokens: 40_000,
            min_tokens: 20_000,
            max_chars: 2000,
        }
    }
}

impl PruneConfig {
    /// 完全关闭（测试与"不启用 prune"路径的显式构造点）。
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }
}

/// 对链上的**老工具输出**做降级。
///
/// 返回 `None` = 无需改动（未启用 / 无超长处 / 收益不足）——调用方直接走原链，
/// 零拷贝、零改动。返回 `Some` = 改写后的链（仅被裁的消息为新对象）。
pub fn prune_tool_outputs(
    chain: &[StructuredMessage],
    cfg: &PruneConfig,
) -> Option<Vec<StructuredMessage>> {
    if !cfg.enabled || cfg.max_chars == 0 {
        return None;
    }

    // call_id → 工具名：受保护工具判定（ToolResult part 自身不含工具名）。
    let mut tool_names: HashMap<&str, &str> = HashMap::new();
    for sm in chain {
        for part in &sm.parts {
            if let Part::ToolCall { id, name, .. } = part {
                tool_names.insert(id.as_str(), name.as_str());
            }
        }
    }

    // 从链尾向前累计工具输出 token：超出保护额度的（更老的）进入可裁区。
    let mut acc = 0usize;
    let mut prunable = vec![false; chain.len()];
    for (i, sm) in chain.iter().enumerate().rev() {
        for part in &sm.parts {
            if let Part::ToolResult {
                tool_call_id,
                content,
                ..
            } = part
            {
                if acc > cfg.protect_tokens {
                    let protected = tool_names
                        .get(tool_call_id.as_str())
                        .is_some_and(|n| PROTECTED_TOOLS.contains(n));
                    if !protected {
                        prunable[i] = true;
                    }
                }
                acc += estimate_tokens(content);
            }
        }
    }

    // 执行裁剪（惰性克隆：只有真正被裁的消息才产生新对象）。
    let mut saved = 0usize;
    let mut out: Option<Vec<StructuredMessage>> = None;
    for (i, sm) in chain.iter().enumerate() {
        if !prunable[i] {
            continue;
        }
        let mut replaced: Option<StructuredMessage> = None;
        for (pi, part) in sm.parts.iter().enumerate() {
            let Part::ToolResult { content, .. } = part else {
                continue;
            };
            // 幂等：已裁过的内容不再裁（标记已在）。
            if content.contains(PRUNE_MARKER) || content.chars().count() <= cfg.max_chars {
                continue;
            }
            let kept: String = content.chars().take(cfg.max_chars).collect();
            let pruned = format!("{kept}{PRUNE_MARKER}");
            saved += estimate_tokens(content).saturating_sub(estimate_tokens(&pruned));
            let target = replaced.get_or_insert_with(|| sm.clone());
            if let Part::ToolResult { content: c, .. } = &mut target.parts[pi] {
                *c = pruned;
            }
        }
        if let Some(s) = replaced {
            out.get_or_insert_with(|| chain.to_vec())[i] = s;
        }
    }

    let out = out?;
    // 收益不足：不改写（改写会换掉前缀、付一次缓存未命中，不值）。
    if saved < cfg.min_tokens {
        return None;
    }
    Some(out)
}

/// 诊断：链上工具输出的估算 token 总量（测试与观测用）。
pub fn tool_output_tokens(chain: &[StructuredMessage]) -> usize {
    chain
        .iter()
        .flat_map(|sm| sm.parts.iter())
        .filter_map(|p| match p {
            Part::ToolResult { content, .. } => Some(estimate_tokens(content)),
            _ => None,
        })
        .sum()
}

/// 借用版本的便捷包装（无需改动时零克隆——供调用方直接拿 `&[StructuredMessage]`）。
pub fn prune_or_borrow<'a>(
    chain: &'a [StructuredMessage],
    cfg: &PruneConfig,
) -> Cow<'a, [StructuredMessage]> {
    match prune_tool_outputs(chain, cfg) {
        Some(v) => Cow::Owned(v),
        None => Cow::Borrowed(chain),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::{DetailedTokenUsage, MessageRole, MessageTime, PartTime};

    /// 取字符串前 `n` 个字符（测试断言辅助）。
    fn original_head(s: &str, n: usize) -> String {
        s.chars().take(n).collect()
    }

    /// 工具结果消息（`content` 为给定文本）。
    fn tool_result(sid: &str, call_id: &str, content: &str) -> StructuredMessage {
        let mut sm = StructuredMessage::user(sid, "");
        sm.role = MessageRole::Tool;
        sm.parts = vec![Part::ToolResult {
            tool_call_id: call_id.to_string(),
            content: content.to_string(),
            error: None,
            time: PartTime::default(),
        }];
        sm
    }

    /// 带工具调用的 assistant 消息（提供 call_id → 工具名映射）。
    fn assistant_with_call(sid: &str, call_id: &str, tool: &str) -> StructuredMessage {
        let mut sm = StructuredMessage::assistant(sid, "调用");
        sm.parts.push(Part::ToolCall {
            id: call_id.to_string(),
            name: tool.to_string(),
            arguments: "{}".to_string(),
            time: PartTime::default(),
        });
        sm
    }

    fn user_msg(sid: &str, text: &str) -> StructuredMessage {
        StructuredMessage::user(sid, text)
    }

    /// 小阈值配置（便于构造可裁场景）。
    fn cfg(protect_tokens: usize, min_tokens: usize, max_chars: usize) -> PruneConfig {
        PruneConfig {
            enabled: true,
            protect_tokens,
            min_tokens,
            max_chars,
        }
    }

    /// 老工具输出被裁到 max_chars + 标记；最近的保留完整。
    #[test]
    fn test_prunes_old_output_keeps_recent_intact() {
        let old = "old-".to_string() + &"x".repeat(9000);
        let recent = "recent-".to_string() + &"y".repeat(9000);
        let chain = vec![
            assistant_with_call("s1", "c1", "grep"),
            tool_result("s1", "c1", &old),
            user_msg("s1", "继续"),
            assistant_with_call("s1", "c2", "grep"),
            tool_result("s1", "c2", &recent),
        ];
        // 保护额度只够最近一条（recent 的 token 数 > 0）；min_tokens 设低使其生效
        let out = prune_tool_outputs(&chain, &cfg(100, 10, 100)).expect("应裁");
        let old_after = match &out[1].parts[0] {
            Part::ToolResult { content, .. } => content.clone(),
            _ => panic!("expected tool result"),
        };
        assert!(old_after.contains(PRUNE_MARKER), "老输出应带裁剪标记");
        let expected_head: String = original_head(&old, 100);
        assert!(
            old_after.starts_with(&expected_head),
            "老输出应保留头部 max_chars 个字符"
        );
        assert_eq!(
            old_after.chars().count(),
            100 + PRUNE_MARKER.chars().count(),
            "保留字数 = max_chars + 标记长度"
        );
        let recent_after = match &out[4].parts[0] {
            Part::ToolResult { content, .. } => content.clone(),
            _ => panic!("expected tool result"),
        };
        assert_eq!(recent_after, recent, "受保护区内（最近）的输出必须完整");
    }

    /// 收益不足（min_tokens 未达）→ 不改写（返回 None，前缀不动）。
    #[test]
    fn test_no_change_when_savings_below_min() {
        let chain = vec![
            assistant_with_call("s1", "c1", "grep"),
            tool_result("s1", "c1", &"x".repeat(1000)),
        ];
        // min_tokens 巨大 → 即便可裁也不动手
        assert!(
            prune_tool_outputs(&chain, &cfg(0, 10_000_000, 100)).is_none(),
            "收益不足必须返回 None（不改写视图）"
        );
    }

    /// 全部在保护额度内 → 不动。
    #[test]
    fn test_all_within_protect_budget_untouched() {
        let chain = vec![
            assistant_with_call("s1", "c1", "grep"),
            tool_result("s1", "c1", &"x".repeat(9000)),
        ];
        assert!(
            prune_tool_outputs(&chain, &cfg(1_000_000, 1, 100)).is_none(),
            "保护额度充足时不得裁剪"
        );
    }

    /// 幂等：对已裁结果再裁一次，结果完全一致（不再叠加标记）。
    #[test]
    fn test_idempotent_on_already_pruned() {
        let chain = vec![
            assistant_with_call("s1", "c1", "grep"),
            tool_result("s1", "c1", &"x".repeat(9000)),
            user_msg("s1", "继续"),
            assistant_with_call("s1", "c2", "grep"),
            tool_result("s1", "c2", &"y".repeat(9000)),
        ];
        let c = cfg(100, 10, 100);
        let once = prune_tool_outputs(&chain, &c).expect("首轮应裁");
        let twice = prune_tool_outputs(&once, &c);
        // 二次运行：已裁内容不再可裁 → 可能整体无需改动（None），或结果与首轮一致
        match twice {
            None => {}
            Some(v) => {
                for (a, b) in v.iter().zip(once.iter()) {
                    assert_eq!(a.parts.len(), b.parts.len());
                    for (pa, pb) in a.parts.iter().zip(b.parts.iter()) {
                        if let (
                            Part::ToolResult { content: ca, .. },
                            Part::ToolResult { content: cb, .. },
                        ) = (pa, pb)
                        {
                            assert_eq!(ca, cb, "幂等：二次裁剪不得改变内容（含标记叠加）");
                        }
                    }
                }
            }
        }
    }

    /// 受保护工具（call_skill）的输出不被裁（同链上非受保护的老输出仍会被裁）。
    #[test]
    fn test_protected_tool_not_pruned() {
        let skill = "skill-".to_string() + &"z".repeat(9000);
        let chain = vec![
            assistant_with_call("s1", "c1", "call_skill"),
            tool_result("s1", "c1", &skill),
            assistant_with_call("s1", "c2", "grep"),
            tool_result("s1", "c2", &"m".repeat(9000)),
            assistant_with_call("s1", "c3", "grep"),
            tool_result("s1", "c3", &"y".repeat(9000)),
        ];
        // 保护额度 0 + 收益阈值 0：除最近一条外都可裁，受保护工具跳过。
        let out = prune_tool_outputs(&chain, &cfg(0, 0, 100)).expect("应裁非受保护输出");
        match &out[1].parts[0] {
            Part::ToolResult { content, .. } => {
                assert_eq!(content, &skill, "call_skill 结果必须完整保留");
            }
            _ => panic!("expected tool result"),
        }
        match &out[3].parts[0] {
            Part::ToolResult { content, .. } => {
                assert!(content.contains(PRUNE_MARKER), "非受保护的老输出应被裁");
            }
            _ => panic!("expected tool result"),
        }
    }

    /// 非工具消息（user/assistant 文本）永不被裁。
    #[test]
    fn test_non_tool_messages_untouched() {
        let long_user = "u".repeat(9000);
        let chain = vec![
            user_msg("s1", &long_user),
            assistant_with_call("s1", "c1", "grep"),
            tool_result("s1", "c1", &"x".repeat(9000)),
            assistant_with_call("s1", "c2", "grep"),
            tool_result("s1", "c2", &"y".repeat(9000)),
        ];
        // 老工具输出（c1）被裁 → 断言同链上的长用户消息不受影响。
        let out = prune_tool_outputs(&chain, &cfg(0, 0, 100)).expect("应裁老工具输出");
        match &out[0].parts[0] {
            Part::Text { text, .. } => assert_eq!(text, &long_user, "用户消息不得被裁"),
            _ => panic!("expected text"),
        }
    }

    /// 关闭时零改动；借用包装在无需改动时走 Borrowed（零克隆）。
    #[test]
    fn test_disabled_and_borrow_path() {
        let chain = vec![
            assistant_with_call("s1", "c1", "grep"),
            tool_result("s1", "c1", &"x".repeat(9000)),
        ];
        assert!(prune_tool_outputs(&chain, &PruneConfig::disabled()).is_none());
        let borrowed = prune_or_borrow(&chain, &PruneConfig::disabled());
        assert!(matches!(borrowed, Cow::Borrowed(_)), "关闭时应借用原链");
        assert_eq!(
            tool_output_tokens(&chain),
            estimate_tokens(&"x".repeat(9000)),
            "诊断函数应统计工具输出 token"
        );
    }

    /// 无 ToolResult 的链（纯文本）→ 恒不改动。
    #[test]
    fn test_chain_without_tool_results() {
        let chain = vec![user_msg("s1", &"a".repeat(9000)), user_msg("s1", "b")];
        assert!(prune_tool_outputs(&chain, &cfg(0, 1, 10)).is_none());
    }

    /// 空 parts 与默认 token 结构不 panic（健壮性）。
    #[test]
    fn test_empty_chain_and_defaults() {
        assert!(prune_tool_outputs(&[], &cfg(0, 1, 10)).is_none());
        let mut sm = StructuredMessage::user("s1", "");
        sm.tokens = DetailedTokenUsage::default();
        sm.time = MessageTime::default();
        assert!(prune_tool_outputs(&[sm], &cfg(0, 1, 10)).is_none());
    }
}

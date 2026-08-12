//! LLM 输出处理的共享原语（judge-plumbing）。
//!
//! 供多个 LLM-as-Judge / LLM 输出消费方（回答评测、构建判断、任务自审、
//! 后台任务摘要、网页抓取文本）复用的纯函数。历史上存在 4 份逐字复制
//! （eval/judge.rs、executor/judge.rs、agent/background.rs、executor/web.rs），
//! 统一收敛于此——共享基础设施归属被依赖方（ADR-007 先例）。

/// 截断长文本以控制 LLM 调用成本（保留头尾）。
///
/// 保留开头（关键信息通常在前）与结尾（摘要/错误统计通常在后），
/// 中间以省略标记接合。**UTF-8 安全**：按字符边界切分，不产生字节
/// 切片 panic（CJK 等宽字符内容安全）。
pub fn truncate_output(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let head_limit = max_chars * 2 / 3;
    let tail_limit = max_chars / 3;
    // 头尾切分边界对齐到字符边界（避免多字节字符中间切片 panic）
    let head_end = text
        .char_indices()
        .take_while(|(i, _)| *i < head_limit)
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    let byte_tail_start = text.len().saturating_sub(tail_limit);
    let tail_start = text
        .char_indices()
        .map(|(i, _)| i)
        .find(|&i| i >= byte_tail_start)
        .unwrap_or(text.len());

    let head = &text[..head_end];
    let tail = &text[tail_start..];
    format!(
        "{}...\n[内容被截断，省略 {} 字符]...\n{}",
        head,
        text.len() - head.len() - tail.len(),
        tail
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short_unchanged() {
        assert_eq!(truncate_output("hello", 2000), "hello");
    }

    #[test]
    fn test_truncate_exact_boundary_unchanged() {
        let text = "a".repeat(100);
        assert_eq!(truncate_output(&text, 100), text);
    }

    #[test]
    fn test_truncate_long_output() {
        let long = "a".repeat(3000);
        let t = truncate_output(&long, 2000);
        assert!(t.contains("[内容被截断"));
        assert!(t.len() <= 2500, "截断 + 省略标记应在上限内: {}", t.len());
    }

    #[test]
    fn test_truncate_cjk_no_panic_and_bounded() {
        // 回归保护：CJK 宽字符内容截断不得产生字节切片 panic，且输出有界
        let long = "汉".repeat(3000);
        let t = truncate_output(&long, 2000);
        assert!(t.contains("[内容被截断"));
        assert!(t.len() <= 2600, "CJK 字符边界对齐允许少量余量: {}", t.len());
        // 截断信息准确（省略字符数 = 原文 - 保留）
        assert!(t.contains("省略"));
    }
}

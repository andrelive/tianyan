//! LLM 输出处理的共享原语（judge-plumbing）。
//!
//! 供多个 LLM-as-Judge / LLM 输出消费方（构建判断、任务自审、
//! 后台任务摘要、网页抓取文本）复用的纯函数。历史上存在 3 份逐字复制
//! （executor/judge.rs、agent/background.rs、executor/web.rs），
//! 统一收敛于此——共享基础设施归属被依赖方（ADR-007 先例）。

/// 剥离 LLM 输出的代码围栏并解析为 JSON 值。
///
/// 兼容三种形态：```json 围栏、无语言 ``` 围栏、无围栏裸 JSON；
/// 直接解析失败时回退为**平衡花括号提取**——从混排文本（前言/后记/围栏夹杂）
/// 中定位首个平衡的 JSON 对象再解析（字符串内转义与花括号均正确跳过）。
/// 全部失败返回 `None`（调用方自行决定降级策略，如告警、空结果或默认值）。
pub fn parse_llm_json(response: &str) -> Option<serde_json::Value> {
    let cleaned = response
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if let Ok(v) = serde_json::from_str(cleaned) {
        return Some(v);
    }
    extract_balanced_json(response)
}

/// 从混排文本中提取首个平衡的 JSON 对象并解析。
///
/// 扫描规则：从首个 `{` 起，正确跳过字符串字面量内的花括号与转义序列，
/// 深度归零处截取；截取片段再走一次严格解析（片段非法仍返回 None）。
fn extract_balanced_json(text: &str) -> Option<serde_json::Value> {
    let start = text.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, ch) in text[start..].char_indices() {
        let abs = start + i;
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&text[start..=abs]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

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
    fn test_parse_llm_json_fenced_with_lang() {
        let v = parse_llm_json("```json\n{\"a\": 1}\n```").unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn test_parse_llm_json_fenced_no_lang() {
        let v = parse_llm_json("```\n{\"a\": 1}\n```").unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn test_parse_llm_json_unfenced() {
        let v = parse_llm_json("{\"a\": 1}").unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn test_parse_llm_json_preamble_extracted() {
        // 前置说明文字 + JSON：平衡提取回退应成功取出 JSON 对象
        // （旧行为返回 None——这正是自演化综述"输出中未找到 JSON"失败的形态之一）
        let v = parse_llm_json("结果如下：{\"a\": 1}，请查收。").unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn test_parse_llm_json_braces_inside_strings_ignored() {
        // 字符串字面量内的花括号不参与平衡计算
        let v = parse_llm_json("说明：{\"text\": \"包含}花括号{\"}").unwrap();
        assert_eq!(v["text"], "包含}花括号{");
    }

    #[test]
    fn test_parse_llm_json_escaped_quotes_in_strings() {
        // 字符串内的转义引号正确跳过
        let v = parse_llm_json("前言\n{\"q\": \"a\\\"b\"}").unwrap();
        assert_eq!(v["q"], "a\"b");
    }

    #[test]
    fn test_parse_llm_json_first_object_wins() {
        // 多个对象时取首个平衡对象
        let v = parse_llm_json("{\"a\": 1}{\"b\": 2}").unwrap();
        assert_eq!(v["a"], 1);
        assert!(v.get("b").is_none());
    }

    #[test]
    fn test_parse_llm_json_unbalanced_returns_none() {
        // 不平衡的花括号：无法截取完整对象
        assert!(parse_llm_json("残缺输出 {\"a\": 1").is_none());
        assert!(parse_llm_json("}").is_none());
    }

    #[test]
    fn test_parse_llm_json_invalid_returns_none() {
        assert!(parse_llm_json("不是 JSON").is_none());
        assert!(parse_llm_json("").is_none());
    }

    #[test]
    fn test_parse_llm_json_whitespace_variants() {
        // 围栏内外的空白变体
        let v = parse_llm_json("  ```json  \n  {\"a\": 1}  \n  ```  ").unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn test_parse_llm_json_nested_fence_content() {
        // 围栏内容含 ``` 字符串时仍应正确解析（只剥首尾围栏）
        let v = parse_llm_json("```json\n{\"code\": \"a ``` b\"}\n```").unwrap();
        assert_eq!(v["code"], "a ``` b");
    }

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

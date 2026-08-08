//! 内置黄金评测用例（离线质量回归用）。
//!
//! 覆盖：概念解释 / 代码诊断 / 开放建议 / 事实问答。
//! 与 [`run_eval_suite`] 搭配：接入真实模型后运行即得基准分，
//! 集成测试用 MockChatService 冒烟验证评分链路可用。

use super::runner::EvalCase;

/// 返回内置黄金用例集。
pub fn golden_cases() -> Vec<EvalCase> {
    vec![
        EvalCase::new(
            "rust-ownership",
            "为什么这段 Rust 代码无法编译？\n```rust\nlet s = String::from(\"hi\");\nlet t = s;\nprintln!(\"{}\", s);\n```",
        )
        .with_reference("String 的所有权在 let t = s 时移动给了 t，s 已失效，println! 使用 s 会触发 use-after-move 编译错误。修复方式：使用 s.clone() 或 &s 借用。"),
        EvalCase::new(
            "http-stateless",
            "HTTP 是无状态协议，这句话是什么意思？",
        )
        .with_reference("服务器不在请求之间保存客户端状态；每个请求独立处理。会话状态需通过 Cookie/Token 等机制显式维持。"),
        EvalCase::new(
            "sql-debug",
            "下面这个 SQL 查询为什么慢？\nSELECT * FROM orders WHERE customer_id = 42;\n表有 1000 万行。",
        )
        .with_reference("customer_id 无索引导致全表扫描。应在 customer_id 上建索引，或改为只 SELECT 需要的列。"),
        EvalCase::new(
            "optimize-api",
            "我写的 REST API 响应时间从 200ms 涨到了 2s，可能是什么原因？如何排查？",
        ),
        EvalCase::new(
            "factual-capital",
            "法国的首都是哪里？",
        )
        .with_reference("巴黎。"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_golden_cases_non_empty() {
        let cases = golden_cases();
        assert!(cases.len() >= 4, "黄金用例应覆盖多个场景");
        for case in &cases {
            assert!(!case.name.is_empty());
            assert!(!case.question.is_empty());
        }
    }
}

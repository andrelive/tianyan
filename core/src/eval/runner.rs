//! 评测批处理入口：对一组黄金用例逐个评分。

use super::judge::AnswerJudge;
use super::{AnswerEvaluation, AnswerVerdict};

/// 单个评测用例。
#[derive(Debug, Clone)]
pub struct EvalCase {
    /// 用例名称（如 "rust-ownership"）。
    pub name: String,
    /// 用户问题。
    pub question: String,
    /// 参考答案（可选，用于人工对比）。
    pub reference_answer: Option<String>,
    /// 参考上下文（可选，用于正确性判断）。
    pub context: Option<String>,
}

impl EvalCase {
    /// 创建评测用例。
    pub fn new(name: impl Into<String>, question: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            question: question.into(),
            reference_answer: None,
            context: None,
        }
    }

    /// 设置参考答案。
    pub fn with_reference(mut self, answer: impl Into<String>) -> Self {
        self.reference_answer = Some(answer.into());
        self
    }

    /// 设置参考上下文。
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }
}

/// 单个用例的评测结果。
#[derive(Debug, Clone)]
pub struct EvalResult {
    /// 用例。
    pub case: EvalCase,
    /// 评测结果。
    pub evaluation: AnswerEvaluation,
}

impl EvalResult {
    /// 是否达到及格线（≥ 5 且成功解析）。
    pub fn passed(&self) -> bool {
        self.evaluation.parsed && self.evaluation.overall_score >= 5.0
    }
}

/// 批量评测用例。
///
/// 逐条顺序执行（共享同一模型服务；并发评分会争抢限速）。
/// 单条失败不阻断后续。
pub async fn run_eval_suite(judge: &AnswerJudge, cases: &[EvalCase]) -> Vec<EvalResult> {
    let mut results = Vec::with_capacity(cases.len());
    for case in cases {
        let evaluation = judge
            .evaluate(
                &case.question,
                case.reference_answer.as_deref().unwrap_or(""),
                case.context.as_deref(),
            )
            .await;
        results.push(EvalResult {
            case: case.clone(),
            evaluation,
        });
    }
    results
}

/// 汇总评测套件为简洁报告（文本形式）。
pub fn format_report(results: &[EvalResult]) -> String {
    let mut out = String::from("回答质量评测报告\n==================\n");
    let mut total = 0.0;
    for r in results {
        let e = &r.evaluation;
        total += e.overall_score;
        out.push_str(&format!(
            "- {}: {:.1}/10 [{}] (相关 {:.0} / 正确 {:.0} / 完整 {:.0} / 清晰 {:.0}){}",
            r.case.name,
            e.overall_score,
            e.verdict,
            e.dimensions.relevance,
            e.dimensions.correctness,
            e.dimensions.completeness,
            e.dimensions.clarity,
            if e.parsed { "" } else { " [解析回退]" },
        ));
        out.push('\n');
        for imp in &e.improvements {
            out.push_str(&format!("    - 建议: {imp}\n"));
        }
    }
    if !results.is_empty() {
        let avg = total / results.len() as f64;
        out.push_str(&format!(
            "\n平均分: {:.1}/10 → {}",
            avg,
            AnswerVerdict::from_score(avg)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eval_result_passed() {
        let case = EvalCase::new("c1", "问题");
        let ok = EvalResult {
            case: case.clone(),
            evaluation: AnswerEvaluation {
                overall_score: 7.0,
                dimensions: crate::eval::DimensionScores {
                    relevance: 7.0,
                    correctness: 7.0,
                    completeness: 7.0,
                    clarity: 7.0,
                },
                strengths: vec![],
                improvements: vec![],
                verdict: AnswerVerdict::Good,
                parsed: true,
            },
        };
        assert!(ok.passed());

        // 解析回退（parsed = false）不算通过
        let fallback = EvalResult {
            case,
            evaluation: AnswerEvaluation::fallback("无法解析"),
        };
        assert!(!fallback.passed());
    }

    #[test]
    fn test_format_report_empty() {
        let report = format_report(&[]);
        assert!(report.contains("平均分") == false || report.trim().is_empty() == false);
        assert!(!report.contains("平均分: "), "空套件不输出平均分");
    }

    #[test]
    fn test_format_report_contains_cases() {
        let results = vec![EvalResult {
            case: EvalCase::new("c1", "q"),
            evaluation: AnswerEvaluation {
                overall_score: 8.0,
                dimensions: crate::eval::DimensionScores {
                    relevance: 8.0,
                    correctness: 8.0,
                    completeness: 8.0,
                    clarity: 8.0,
                },
                strengths: vec![],
                improvements: vec!["更详细".to_string()],
                verdict: AnswerVerdict::Good,
                parsed: true,
            },
        }];
        let report = format_report(&results);
        assert!(report.contains("c1"));
        assert!(report.contains("平均分: 8.0"));
        assert!(report.contains("更详细"));
    }
}

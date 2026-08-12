//! 回答质量评测器（LLM-as-Judge 评分式）。
//!
//! 输入：用户问题 + Agent 回答（可选参考上下文）；
//! 输出：[`AnswerEvaluation`]（四维度 1-10 分 + 加权总分 + 优点/改进建议）。
//!
//! 解析回退链：JSON 块 → 行格式（SCORE/维度行）→ 中性默认分（5 分，
//! `parsed = false` 标记不可靠），与 [`crate::executor::judge`] 的
//! VERDICT 回退思路一致，保证评测调用永不因解析失败而崩溃。

use std::sync::Arc;

use serde::Deserialize;

use crate::common::llm_judge::truncate_output;
use crate::common::types::Message;
use crate::model::types::ChatCompletionRequest;
use crate::model::ChatService;

use super::{AnswerEvaluation, AnswerVerdict, DimensionScores};

/// LLM-as-Judge 回答质量评测器。
#[derive(Clone)]
pub struct AnswerJudge {
    model_service: Arc<dyn ChatService>,
    model: String,
}

impl AnswerJudge {
    /// 创建评测器。
    ///
    /// - `model_service` — 共享模型服务（与聊天链路同一实例）
    /// - `model` — 用于评测的模型名（应选择判断力强的模型）
    pub fn new(model_service: Arc<dyn ChatService>, model: impl Into<String>) -> Self {
        Self {
            model_service,
            model: model.into(),
        }
    }

    /// 评测单条问答。
    ///
    /// - `question` — 用户问题
    /// - `answer` — Agent 回答
    /// - `context` — 可选参考上下文（知识库片段等，帮助评测正确性）
    pub async fn evaluate(
        &self,
        question: &str,
        answer: &str,
        context: Option<&str>,
    ) -> AnswerEvaluation {
        // 快速路径：空回答直接低分，省一次 LLM 调用
        if answer.trim().is_empty() {
            return AnswerEvaluation::fallback("回答为空");
        }

        let prompt = build_eval_prompt(question, answer, context);
        let request = ChatCompletionRequest::new(&self.model, vec![Message::user(&prompt)]);

        let response = match self.model_service.chat_completion(request).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "AnswerJudge LLM 调用失败，回退到默认分");
                return AnswerEvaluation::fallback(&format!("LLM 调用失败：{e}"));
            }
        };

        let content = response.first_choice_content().unwrap_or_default();

        parse_evaluation(&content)
    }
}

/// 构建评测提示词。
fn build_eval_prompt(question: &str, answer: &str, context: Option<&str>) -> String {
    let mut prompt = String::from(
        "你是一个严格、客观的回答质量评审员。请评测以下 AI 回答，按四个维度各打 1-10 分：\n\
         \n\
         - relevance（相关性）：回答是否切题、聚焦用户真实意图\n\
         - correctness（正确性）：事实是否准确，有无错误或误导\n\
         - completeness（完整性）：是否覆盖用户需求的所有关键点\n\
         - clarity（清晰度）：结构、表达、可读性\n\
         \n\
         评分指南：\n\
         - 9-10：优秀，准确且完整，几乎无可挑剔\n\
         - 7-8：良好，基本准确完整，有小瑕疵\n\
         - 5-6：及格，方向对但明显不足\n\
         - 1-4：差，错误、偏题或严重不完整\n\
         \n\
         请只输出以下 JSON（不要输出其他内容）：\n\
         {\n\
           \"relevance\": <1-10 数字>,\n\
           \"correctness\": <1-10 数字>,\n\
           \"completeness\": <1-10 数字>,\n\
           \"clarity\": <1-10 数字>,\n\
           \"strengths\": [\"<优点1>\", \"<优点2>\"],\n\
           \"improvements\": [\"<改进建议1>\"]\n\
         }\n",
    );

    if let Some(ctx) = context {
        if !ctx.is_empty() {
            prompt.push_str("\n参考上下文（用于判断正确性）：\n");
            prompt.push_str(&truncate_output(ctx, 3000));
            prompt.push('\n');
        }
    }

    prompt.push_str("\n用户问题：\n");
    prompt.push_str(&truncate_output(question, 2000));
    prompt.push_str("\n\nAI 回答：\n");
    prompt.push_str(&truncate_output(answer, 4000));
    prompt.push('\n');
    prompt
}

/// LLM 输出的结构化评分（JSON 解析目标）。
#[derive(Debug, Deserialize)]
struct EvalJson {
    relevance: f64,
    correctness: f64,
    completeness: f64,
    clarity: f64,
    #[serde(default)]
    strengths: Vec<String>,
    #[serde(default)]
    improvements: Vec<String>,
}

/// 解析 LLM 评测回复：JSON 块 → 行格式 → 中性回退。
fn parse_evaluation(content: &str) -> AnswerEvaluation {
    // 1. 优先尝试完整 JSON 解析
    if let Ok(eval) = serde_json::from_str::<EvalJson>(content.trim()) {
        return from_json(eval, true);
    }

    // 2. 尝试从回复中提取 JSON 块（LLM 常夹杂 ```json 围栏或前言）
    if let Some(block) = extract_json_block(content) {
        if let Ok(eval) = serde_json::from_str::<EvalJson>(&block) {
            return from_json(eval, true);
        }
    }

    // 3. 行格式回退：逐行解析 SCORE/维度标记
    if let Some(eval) = parse_line_format(content) {
        return eval;
    }

    // 4. 中性默认分（标记 parsed = false）
    AnswerEvaluation::fallback("评测回复无法解析")
}

fn from_json(eval: EvalJson, parsed: bool) -> AnswerEvaluation {
    let clamp = |v: f64| v.clamp(1.0, 10.0);
    let dims = DimensionScores {
        relevance: clamp(eval.relevance),
        correctness: clamp(eval.correctness),
        completeness: clamp(eval.completeness),
        clarity: clamp(eval.clarity),
    };
    let overall = dims.weighted_overall();
    AnswerEvaluation {
        overall_score: (overall * 10.0).round() / 10.0,
        dimensions: dims,
        strengths: eval.strengths,
        improvements: eval.improvements,
        verdict: AnswerVerdict::from_score(overall),
        parsed,
    }
}

/// 提取回复中的第一个 JSON 对象块（支持 ```json 围栏）。
fn extract_json_block(content: &str) -> Option<String> {
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(content[start..=end].to_string())
}

/// 行格式回退：兼容 VERDICT 风格的行标记输出。
fn parse_line_format(content: &str) -> Option<AnswerEvaluation> {
    let mut relevance = None;
    let mut correctness = None;
    let mut completeness = None;
    let mut clarity = None;
    for line in content.lines() {
        let line = line.trim();
        for (key, slot) in [
            ("RELEVANCE:", &mut relevance),
            ("CORRECTNESS:", &mut correctness),
            ("COMPLETENESS:", &mut completeness),
            ("CLARITY:", &mut clarity),
        ] {
            if let Some(rest) = line.strip_prefix(key) {
                *slot = rest.trim().parse::<f64>().ok();
            }
        }
    }
    let dims = DimensionScores {
        relevance: relevance?,
        correctness: correctness?,
        completeness: completeness?,
        clarity: clarity?,
    };
    Some(AnswerEvaluation {
        overall_score: (dims.weighted_overall() * 10.0).round() / 10.0,
        dimensions: dims,
        strengths: Vec::new(),
        improvements: Vec::new(),
        verdict: AnswerVerdict::from_score(dims.weighted_overall()),
        parsed: false, // 行格式缺少优点/建议，视为部分解析
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short_unchanged() {
        assert_eq!(truncate_output("hello", 2000), "hello");
    }

    #[test]
    fn test_truncate_long_output() {
        let long = "a".repeat(3000);
        let t = truncate_output(&long, 2000);
        assert!(t.contains("[内容被截断"));
    }

    #[test]
    fn test_parse_json_evaluation() {
        let content = r#"{"relevance": 9, "correctness": 8, "completeness": 7, "clarity": 8, "strengths": ["准确"], "improvements": ["可更详细"]}"#;
        let eval = parse_evaluation(content);
        assert!(eval.parsed);
        assert!((eval.dimensions.relevance - 9.0).abs() < 0.01);
        assert_eq!(eval.strengths, vec!["准确"]);
        assert_eq!(eval.verdict, AnswerVerdict::Good);
        // 加权：8*0.4+9*0.25+7*0.25+8*0.1 = 3.2+2.25+1.75+0.8 = 8.0
        assert!((eval.overall_score - 8.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_json_with_fences() {
        let content = "好的，评测如下：\n```json\n{\"relevance\": 10, \"correctness\": 10, \"completeness\": 10, \"clarity\": 10}\n```\n";
        let eval = parse_evaluation(content);
        assert!(eval.parsed);
        assert_eq!(eval.verdict, AnswerVerdict::Excellent);
    }

    #[test]
    fn test_parse_line_format_fallback() {
        let content = "RELEVANCE: 6\nCORRECTNESS: 5\nCOMPLETENESS: 7\nCLARITY: 6";
        let eval = parse_evaluation(content);
        assert!(!eval.parsed, "行格式视为部分解析");
        assert!((eval.dimensions.correctness - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_garbage_falls_back_to_neutral() {
        let eval = parse_evaluation("完全无法理解的内容！！！");
        assert!(!eval.parsed);
        assert_eq!(eval.overall_score, 5.0);
        assert_eq!(eval.verdict, AnswerVerdict::Fair);
    }

    #[test]
    fn test_parse_clamps_out_of_range_scores() {
        let content = r#"{"relevance": 99, "correctness": -5, "completeness": 7, "clarity": 6}"#;
        let eval = parse_evaluation(content);
        assert!(eval.parsed);
        assert!(
            (eval.dimensions.relevance - 10.0).abs() < 0.01,
            "高分应被钳制到 10"
        );
        assert!(
            (eval.dimensions.correctness - 1.0).abs() < 0.01,
            "低分应被钳制到 1"
        );
    }

    #[test]
    fn test_verdict_mapping() {
        assert_eq!(AnswerVerdict::from_score(9.0), AnswerVerdict::Excellent);
        assert_eq!(AnswerVerdict::from_score(8.0), AnswerVerdict::Good);
        assert_eq!(AnswerVerdict::from_score(6.0), AnswerVerdict::Fair);
        assert_eq!(AnswerVerdict::from_score(3.0), AnswerVerdict::Poor);
    }

    #[test]
    fn test_empty_answer_short_circuit() {
        // 空回答走 fallback 快速路径（不调用 LLM）——通过 fallback 结构验证
        let eval = AnswerEvaluation::fallback("回答为空");
        assert!(!eval.parsed);
        assert_eq!(eval.overall_score, 5.0);
    }

    // ── 集成测试：MockChatService 全链路 ──────────────────────────────

    fn chat_response(content: &str) -> crate::model::types::ChatCompletionResponse {
        use crate::common::types::{Message, TokenUsage};
        use crate::model::types::ChatChoice;
        crate::model::types::ChatCompletionResponse {
            id: "test".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test-model".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: Message::assistant(content),
                finish_reason: Some("stop".to_string()),
            }],
            usage: TokenUsage::default(),
        }
    }

    #[tokio::test]
    async fn test_evaluate_success_with_mock() {
        use crate::model::MockChatService;

        let mut mock = MockChatService::new();
        mock.expect_chat_completion()
            .times(1)
            .returning(|_| {
                Ok(chat_response(
                    r#"{"relevance": 9, "correctness": 8, "completeness": 7, "clarity": 8, "strengths": ["准确"], "improvements": ["更详细"]}"#,
                ))
            });
        let judge = AnswerJudge::new(Arc::new(mock), "test-model");

        let eval = judge.evaluate("问题", "回答", None).await;
        assert!(eval.parsed);
        assert!((eval.dimensions.relevance - 9.0).abs() < 0.01);
        assert_eq!(eval.strengths, vec!["准确"]);
        assert_eq!(eval.verdict, AnswerVerdict::Good);
    }

    #[tokio::test]
    async fn test_evaluate_empty_answer_skips_llm() {
        use crate::model::MockChatService;

        let mut mock = MockChatService::new();
        // 空回答应走快速路径，不调用 LLM
        mock.expect_chat_completion().times(0);
        let judge = AnswerJudge::new(Arc::new(mock), "test-model");

        let eval = judge.evaluate("问题", "   ", None).await;
        assert!(!eval.parsed);
        assert_eq!(eval.overall_score, 5.0);
    }

    #[tokio::test]
    async fn test_evaluate_llm_error_falls_back_to_neutral() {
        use crate::model::MockChatService;

        let mut mock = MockChatService::new();
        mock.expect_chat_completion().times(1).returning(|_| {
            Err(crate::common::error::TianyanError::Custom(
                "模拟调用失败".to_string(),
            ))
        });
        let judge = AnswerJudge::new(Arc::new(mock), "test-model");

        let eval = judge.evaluate("问题", "回答", None).await;
        assert!(!eval.parsed, "LLM 失败应回退到中性分");
        assert_eq!(eval.overall_score, 5.0);
    }
}

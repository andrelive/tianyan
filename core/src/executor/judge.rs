//! LLM-as-Judge：在 Agent 返回最终答案前进行独立质量评判。

use crate::common::error::Result;
use crate::model::ModelService;
use std::sync::Arc;

/// Judge 验证结果。
#[derive(Debug, Clone)]
pub struct JudgeVerdict {
    pub passed: bool,
    pub confidence: f32,
    pub issues: Vec<String>,
    pub suggestion: Option<String>,
}

/// LLM-as-Judge。
///
/// 默认关闭，通过 AgentConfig 显式启用。
#[derive(Clone)]
pub struct LlmJudge {
    model_service: Arc<dyn ModelService>,
    model_name: String,
    enabled: bool,
}

const JUDGE_PROMPT: &str = r#"评估以下 AI 助手对用户任务的完成情况，以 JSON 格式返回评判结果：

## 用户任务
{task}

## 执行结果
{execution_results}

## 拟返回的答案
{proposed_answer}

## 评判标准（返回 JSON，不要其他内容）
{
  "passed": true 或 false,
  "confidence": 0.0 到 1.0 之间的分数,
  "issues": ["发现的问题"],
  "suggestion": "改进建议（通过则为 null）"
}
"#;

impl LlmJudge {
    pub fn new(model_service: Arc<dyn ModelService>, model_name: impl Into<String>) -> Self {
        Self {
            model_service,
            model_name: model_name.into(),
            enabled: false,
        }
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// 评判 Agent 输出的完整性、正确性。
    ///
    /// - `task`: 用户原始任务
    /// - `execution_results`: 执行结果摘要
    /// - `proposed_answer`: 拟返回的答案
    /// - returns: 评判结果
    pub async fn evaluate(
        &self,
        task: &str,
        execution_results: &str,
        proposed_answer: &str,
    ) -> Result<JudgeVerdict> {
        if !self.enabled {
            return Ok(JudgeVerdict {
                passed: true,
                confidence: 1.0,
                issues: vec![],
                suggestion: None,
            });
        }

        let prompt = JUDGE_PROMPT
            .replace("{task}", task)
            .replace("{execution_results}", execution_results)
            .replace("{proposed_answer}", proposed_answer);

        let response = self
            .model_service
            .chat(
                &self.model_name,
                vec![crate::common::types::Message::user(prompt)],
            )
            .await?;

        self.parse_verdict(&response)
    }

    fn parse_verdict(&self, response: &str) -> Result<JudgeVerdict> {
        let cleaned = response
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        let json: serde_json::Value = serde_json::from_str(cleaned).unwrap_or(serde_json::json!({
            "passed": true,
            "confidence": 0.5,
            "issues": [],
            "suggestion": null
        }));

        let passed = json.get("passed").and_then(|v| v.as_bool()).unwrap_or(true);

        let confidence = json
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5) as f32;

        let issues: Vec<String> = json
            .get("issues")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let suggestion = json
            .get("suggestion")
            .and_then(|v| v.as_str())
            .filter(|s| *s != "null")
            .map(|s| s.to_string());

        Ok(JudgeVerdict {
            passed,
            confidence,
            issues,
            suggestion,
        })
    }
}

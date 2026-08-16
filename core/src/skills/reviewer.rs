//! 技能使用复审（基于会话证据：执行结果 + 用户反馈）。
//!
//! 与记忆提取同周期运行（`MemoryTask` 顺路调用）：对会话中**实际调用过**的
//! 技能，依据「调用参数 + 工具执行结果 + 后续用户反馈」由 LLM 评审，
//! 产出质量分（0-10）/ 倾向 / 理由，落 VFS `skill/_reviews/<id>.jsonl`。
//!
//! 语义：方法论技能本身无成败——评审的是**使用效果**（被采纳/推动任务/用户满意），
//! 与面板的「调用频率」互补；低分技能是退役/改进信号。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::llm_judge::{parse_llm_json, truncate_output};
use crate::common::types::{
    ContentLevel, ContextNamespace, Message, Part, StructuredMessage, TianyanUri,
};
use crate::model::ChatService;
use crate::session::parse_message_lines;
use crate::vfs::VirtualFileSystem;

/// 评审记录存储目录名（技能发现时过滤）。
pub const REVIEWS_PREFIX: &str = "_reviews";

/// 单次技能使用评审记录。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillReview {
    /// 评审时间（epoch 毫秒）。
    pub ts: i64,
    /// 技能 ID。
    pub skill_id: String,
    /// 来源会话。
    pub session_id: String,
    /// 质量分（0-10）。
    pub score: u8,
    /// 使用效果判定（positive / neutral / negative）。
    pub verdict: String,
    /// 用户反馈倾向（positive / negative / mixed / none）。
    pub user_feedback: String,
    /// 理由（截断）。
    pub reason: String,
    /// 证据摘要（截断）。
    pub evidence: String,
}

/// 技能使用评审器。
#[derive(Clone)]
pub struct SkillReviewer {
    chat: Arc<dyn ChatService>,
    vfs: Arc<dyn VirtualFileSystem>,
    model: String,
    /// 单个会话最多评审次数（防刷屏）。
    max_reviews_per_conversation: usize,
}

/// 评审提示词：基于会话证据评估一次技能调用的实际效果。
pub const SKILL_REVIEW_PROMPT: &str = r#"你是技能质量评审员。评估一次技能调用的**实际使用效果**（不是文档质量——
技能是方法论文档，重点看它是否真的帮到了任务）。

技能：{skill_id}
技能说明：{abstract}

本次调用的会话证据（调用参数、工具执行结果、后续用户反馈）：
{evidence}

评分标准（0-10）：
- 技能被正确调用且输出有用（4 分）
- 执行结果被采纳、推动任务完成（3 分）
- 用户反馈正面、无纠正（3 分）

输出 JSON（不要输出其他内容）：
{"score": <0-10 整数>, "verdict": "positive|neutral|negative", "user_feedback": "positive|negative|mixed|none", "reason": "一句话理由"}
"#;

impl SkillReviewer {
    /// 创建评审器。`model` 为评审模型名。
    pub fn new(chat: Arc<dyn ChatService>, vfs: Arc<dyn VirtualFileSystem>, model: String) -> Self {
        Self {
            chat,
            vfs,
            model,
            max_reviews_per_conversation: 3,
        }
    }

    /// 从会话文本提取技能调用证据并评审（记忆任务顺路调用；失败不影响调用方）。
    pub async fn review_conversation(
        &self,
        session_id: &str,
        conversation: &str,
    ) -> Result<Vec<SkillReview>> {
        let messages = parse_message_lines(conversation);
        // 1. 提取技能调用样本（skill_id, 证据窗口）
        let mut samples: Vec<(String, String)> = Vec::new();
        for (i, msg) in messages.iter().enumerate() {
            for part in &msg.parts {
                if let Part::ToolCall {
                    name, arguments, ..
                } = part
                {
                    if name != "call_skill" {
                        continue;
                    }
                    // 不依赖 agent 模块：直接解析 arguments JSON 取 skill_id
                    let Some(skill_id) = serde_json::from_str::<serde_json::Value>(arguments)
                        .ok()
                        .and_then(|v| {
                            v.get("skill_id")
                                .and_then(|s| s.as_str())
                                .map(str::to_string)
                        })
                    else {
                        continue;
                    };
                    let evidence = build_evidence(&messages, i);
                    if samples.len() < self.max_reviews_per_conversation {
                        samples.push((skill_id, evidence));
                    }
                }
            }
        }
        if samples.is_empty() {
            return Ok(Vec::new());
        }

        // 2. 逐样本评审 + 落盘
        let mut reviews = Vec::new();
        for (skill_id, evidence) in samples {
            let abstract_text = self
                .vfs
                .read_abstract(&TianyanUri::new(
                    ContextNamespace::Skill,
                    vec![skill_id.clone()],
                ))
                .await
                .unwrap_or_default();
            match self.review_one(&skill_id, &abstract_text, &evidence).await {
                Ok(Some(mut review)) => {
                    review.session_id = session_id.to_string();
                    if let Err(e) = self.store_review(&review).await {
                        tracing::warn!(skill_id = %skill_id, error = %e, "技能评审记录存储失败");
                    }
                    tracing::info!(
                        skill_id = %skill_id,
                        score = review.score,
                        verdict = %review.verdict,
                        "技能使用评审完成"
                    );
                    reviews.push(review);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(skill_id = %skill_id, error = %e, "技能评审失败");
                }
            }
        }
        Ok(reviews)
    }

    /// 单次评审（LLM）。
    async fn review_one(
        &self,
        skill_id: &str,
        abstract_text: &str,
        evidence: &str,
    ) -> Result<Option<SkillReview>> {
        let prompt = SKILL_REVIEW_PROMPT
            .replace("{skill_id}", skill_id)
            .replace("{abstract}", abstract_text)
            .replace("{evidence}", evidence);
        let response = match self
            .chat
            .chat(&self.model, vec![Message::user(prompt)])
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(skill_id = %skill_id, error = %e, "技能评审调用失败");
                return Ok(None);
            }
        };
        let json: serde_json::Value = match parse_llm_json(&response) {
            Some(v) => v,
            None => {
                tracing::warn!(skill_id = %skill_id, "技能评审响应解析失败");
                return Ok(None);
            }
        };
        let Some(score) = json
            .get("score")
            .and_then(|v| v.as_u64())
            .map(|s| s.min(10) as u8)
        else {
            tracing::warn!(skill_id = %skill_id, "技能评审缺少 score");
            return Ok(None);
        };
        let verdict = json
            .get("verdict")
            .and_then(|v| v.as_str())
            .unwrap_or("neutral")
            .to_string();
        let user_feedback = json
            .get("user_feedback")
            .and_then(|v| v.as_str())
            .unwrap_or("none")
            .to_string();
        let reason = json
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok(Some(SkillReview {
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            skill_id: skill_id.to_string(),
            session_id: String::new(),
            score,
            verdict,
            user_feedback,
            reason: truncate_output(&reason, 300),
            evidence: truncate_output(evidence, 400),
        }))
    }

    /// 追加评审记录（JSONL，保留最近 50 条）。
    async fn store_review(&self, review: &SkillReview) -> Result<()> {
        let uri = reviews_uri(&review.skill_id);
        if let Some(parent) = uri.parent() {
            if !self.vfs.exists(&parent).await? {
                self.vfs.create_directory(&parent).await?;
            }
        }
        let mut reviews = self.load_reviews(&review.skill_id).await?;
        reviews.push(review.clone());
        if reviews.len() > 50 {
            let drop = reviews.len() - 50;
            reviews.drain(..drop);
        }
        let jsonl = reviews
            .iter()
            .filter_map(|r| serde_json::to_string(r).ok())
            .collect::<Vec<_>>()
            .join("\n");
        self.vfs.write_content(&uri, &jsonl).await
    }

    /// 读取某技能的全部评审记录（时间升序）。
    pub async fn load_reviews(&self, skill_id: &str) -> Result<Vec<SkillReview>> {
        let uri = reviews_uri(skill_id);
        if !self.vfs.exists(&uri).await? {
            return Ok(Vec::new());
        }
        let content = self.vfs.read_content(&uri, ContentLevel::Detail).await?;
        Ok(content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<SkillReview>(l.trim()).ok())
            .collect())
    }

    /// 某技能的最新评审。
    pub async fn latest_review(&self, skill_id: &str) -> Result<Option<SkillReview>> {
        Ok(self.load_reviews(skill_id).await?.pop())
    }
}

/// 评审记录 URI（`tianyan://skill/_reviews/<id>.jsonl`）。
fn reviews_uri(skill_id: &str) -> TianyanUri {
    TianyanUri::new(
        ContextNamespace::Skill,
        vec![REVIEWS_PREFIX.to_string(), format!("{skill_id}.jsonl")],
    )
}

#[cfg(test)]
#[path = "reviewer_tests.rs"]
mod tests;

/// 构建证据窗口：调用消息前后各若干条（文本/结果/用户反馈），截断防膨胀。
fn build_evidence(messages: &[StructuredMessage], idx: usize) -> String {
    let start = idx.saturating_sub(1);
    let end = (idx + 4).min(messages.len());
    let mut parts_text: Vec<String> = Vec::new();
    for msg in &messages[start..end] {
        for part in &msg.parts {
            match part {
                Part::Text { text, .. } => parts_text.push(text.clone()),
                Part::ToolCall {
                    name, arguments, ..
                } => {
                    parts_text.push(format!("[调用 {name}] {arguments}"));
                }
                Part::ToolResult { content, .. } => {
                    parts_text.push(format!("[工具结果] {content}"));
                }
                Part::Reasoning { .. } => {}
                Part::Image { .. } => {}
            }
        }
    }
    truncate_output(&parts_text.join("\n"), 2000)
}

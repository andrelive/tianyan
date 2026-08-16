use std::sync::Arc;

use super::SkillReviewer;
use crate::common::types::{
    ContentLevel, ContextNamespace, MessageRole, MessageTime, Part, PartTime, StructuredMessage,
    TianyanUri,
};
use crate::model::types::{ChatChoice, ChatCompletionResponse};
use crate::model::MockChatService;
use crate::test_utils::MockVfs;

fn response_with(text: &str) -> ChatCompletionResponse {
    ChatCompletionResponse {
        id: "resp-1".to_string(),
        object: "chat.completion".to_string(),
        created: 0,
        model: "test".to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: crate::common::types::Message::assistant(text.to_string()),
            finish_reason: Some("stop".to_string()),
        }],
        usage: crate::common::types::TokenUsage::default(),
    }
}

fn msg(role: MessageRole, parts: Vec<Part>) -> StructuredMessage {
    StructuredMessage {
        id: format!("msg_{}", rand_id()),
        parent_id: None,
        role,
        parts,
        tokens: Default::default(),
        cost: 0.0,
        model_id: None,
        time: MessageTime::default(),
        session_id: "sess-1".to_string(),
        finish: None,
        compression_marker: false,
    }
}

fn rand_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::SeqCst)
}

fn conversation_jsonl() -> String {
    let messages = vec![
        msg(
            MessageRole::User,
            vec![Part::Text {
                text: "帮我做个计划".to_string(),
                time: PartTime::default(),
            }],
        ),
        msg(
            MessageRole::Assistant,
            vec![Part::ToolCall {
                id: "tc1".to_string(),
                name: "call_skill".to_string(),
                arguments: r#"{"skill_id":"planning","parameters":{}}"#.to_string(),
                time: PartTime::default(),
            }],
        ),
        msg(
            MessageRole::Tool,
            vec![Part::ToolResult {
                tool_call_id: "tc1".to_string(),
                content: "计划指南已返回".to_string(),
                time: PartTime::default(),
            }],
        ),
        msg(
            MessageRole::Assistant,
            vec![Part::Text {
                text: "好的，按计划执行。".to_string(),
                time: PartTime::default(),
            }],
        ),
        msg(
            MessageRole::User,
            vec![Part::Text {
                text: "不错，就这样做".to_string(),
                time: PartTime::default(),
            }],
        ),
    ];
    messages
        .iter()
        .map(|m| serde_json::to_string(m).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn test_review_conversation_extracts_and_scores() {
    // mock LLM 返回评审 JSON；MockVfs 提供技能摘要
    let review_json = r#"{
  "score": 8,
  "verdict": "positive",
  "user_feedback": "positive",
  "reason": "技能被正确调用，用户反馈正面"
}"#;
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .times(1)
        .returning(move |_| Ok(response_with(review_json)));

    let vfs = Arc::new(MockVfs::new());
    let skill_uri = TianyanUri::new(ContextNamespace::Skill, vec!["planning".to_string()]);
    vfs.set_content(
        &skill_uri,
        ContentLevel::Abstract,
        "计划类技能：提供任务计划指南",
    );
    let reviewer = SkillReviewer::new(Arc::new(mock), vfs.clone(), "test-model".to_string());

    let reviews = reviewer
        .review_conversation("sess-1", &conversation_jsonl())
        .await
        .unwrap();
    assert_eq!(reviews.len(), 1, "应提取到 planning 调用并评审");
    let r = &reviews[0];
    assert_eq!(r.skill_id, "planning");
    assert_eq!(r.score, 8);
    assert_eq!(r.verdict, "positive");
    assert_eq!(r.user_feedback, "positive");
    assert!(!r.reason.is_empty());

    // 落盘验证：latest_review 可读回
    let latest = reviewer
        .latest_review("planning")
        .await
        .unwrap()
        .expect("评审应已落盘");
    assert_eq!(latest.score, 8);
}

#[tokio::test]
async fn test_review_no_skill_calls_returns_empty() {
    // 无 call_skill 的会话：不触发 LLM，返回空
    let mock = MockChatService::new();
    let reviewer = SkillReviewer::new(
        Arc::new(mock),
        Arc::new(MockVfs::new()),
        "test-model".to_string(),
    );
    let plain = msg(
        MessageRole::User,
        vec![Part::Text {
            text: "你好".to_string(),
            time: PartTime::default(),
        }],
    );
    let jsonl = serde_json::to_string(&plain).unwrap();
    let reviews = reviewer
        .review_conversation("sess-1", &jsonl)
        .await
        .unwrap();
    assert!(reviews.is_empty());
}

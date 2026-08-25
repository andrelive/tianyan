use super::*;
use crate::common::error::Result;
use crate::common::types::StructuredMessage;
use crate::session::Session;
use async_trait::async_trait;

struct MockSessionManager;
#[async_trait]
impl SessionManager for MockSessionManager {
    async fn add_structured_message(
        &self,
        _session_id: &str,
        _msg: StructuredMessage,
    ) -> Result<()> {
        Ok(())
    }
    async fn rewrite_messages(
        &self,
        _session_id: &str,
        _messages: &[StructuredMessage],
    ) -> Result<()> {
        Ok(())
    }
    async fn create_session(&self, _id: &str, _message: Message) -> Result<Session> {
        Ok(Session::new(_id))
    }
    async fn get_session(&self, _id: &str) -> Result<Option<Session>> {
        Ok(None)
    }
    async fn update_session(&self, _session: &Session) -> Result<()> {
        Ok(())
    }
    async fn list_sessions(&self) -> Result<Vec<Session>> {
        Ok(vec![])
    }
    async fn delete_session(&self, _id: &str) -> Result<()> {
        Ok(())
    }
}

#[test]
fn test_agent_loop_config_default() {
    let config = AgentLoopConfig::default();
    assert_eq!(config.max_turns, 200);
    // Touch the mock to keep it "constructed" (dead-code lint).
    let _mgr = MockSessionManager;
}

#[test]
fn test_with_chat_spec_chain() {
    // 链式注入：with_chat_spec(Some(spec)) 设置 chat_spec，
    // with_chat_spec(None) 清除；默认 new() 为 None。
    let spec = ModelSpec {
        context_length: 100_000,
        max_output_tokens: 8_192,
        max_input_tokens: 91_808,
    };
    let base = make_loop(MockChatService::new(), 5);
    assert_eq!(
        base.chat_spec, None,
        "新建 AgentLoop 的 chat_spec 应为 None"
    );

    let with_spec = base.with_chat_spec(Some(spec));
    assert_eq!(
        with_spec.chat_spec,
        Some(spec),
        "注入 spec 后 chat_spec 应为 Some(spec)"
    );

    let cleared = with_spec.with_chat_spec(None);
    assert_eq!(cleared.chat_spec, None, "with_chat_spec(None) 应清除 spec");
}

#[test]
fn test_agent_loop_error_display() {
    let err = TianyanError::Custom(format!("agent_loop: 达到最大轮数限制：{}", 5));
    assert!(err.to_string().contains("5"));
}

// ── 完整循环测试（MockChatService + 真实 ToolRegistry） ────

use crate::common::types::TokenUsage as CommonTokenUsage;
use crate::common::types::{FunctionCall, ToolCall, ToolCallType};
use crate::executor::SecurityPolicy;
use crate::model::types::{
    ChatChoice, ChatCompletionChunk, ChatCompletionResponse, ChunkChoice, DeltaContent,
    ToolCallDelta, ToolCallFunctionDelta,
};
use crate::model::MockChatService;

// 流式回归测试：AgentStreamChunk / StreamChunkType
use crate::agent::types::{AgentStreamChunk, StreamChunkType};

fn response_with(assistant: Message) -> ChatCompletionResponse {
    ChatCompletionResponse {
        id: "resp-1".to_string(),
        object: "chat.completion".to_string(),
        created: 0,
        model: "test".to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: assistant,
            finish_reason: Some("stop".to_string()),
        }],
        usage: CommonTokenUsage::default(),
    }
}

fn tool_call_msg(name: &str) -> Message {
    Message::assistant_with_tools(
        "",
        vec![ToolCall {
            id: format!("call_{}", name),
            call_type: ToolCallType::Function,
            function: FunctionCall {
                name: name.to_string(),
                arguments: "{}".to_string(),
            },
        }],
    )
}

fn make_loop(mock: MockChatService, max_turns: usize) -> AgentLoop {
    let registry = ToolRegistry::new(SecurityPolicy::default());
    AgentLoop::new(
        Arc::new(mock),
        registry,
        Arc::new(MockSessionManager),
        AgentLoopConfig {
            max_turns,
            ..Default::default()
        },
    )
}

#[tokio::test]
async fn test_run_completes_tool_loop() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion().returning(|req| {
        // 第一轮：仅 user 消息 → 返回工具调用（未知工具 → 工具错误结果）
        // 第二轮：user + assistant + tool → 返回最终回答
        if req.messages.len() <= 1 {
            Ok(response_with(tool_call_msg("nonexistent_tool")))
        } else {
            Ok(response_with(Message::assistant("最终回答")))
        }
    });
    let agent_loop = make_loop(mock, 5);

    let mut messages = vec![Message::user("帮我做点事")];
    let result = agent_loop
        .run(&mut messages, "session-1", None, "test-model", None, None)
        .await
        .unwrap();

    match result {
        AgentLoopResult::Answer { content, turns, .. } => {
            assert_eq!(content, "最终回答");
            assert_eq!(turns, 2);
        }
        other => panic!("期望 Answer，得到 {:?}", other),
    }
    // 循环结束后消息历史包含 user + assistant + tool + assistant
    assert!(messages.len() >= 4);
}

#[tokio::test]
async fn test_run_ask_user_returns_clarification() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion().returning(|_| {
        Ok(response_with(Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: "call_ask".to_string(),
                call_type: ToolCallType::Function,
                function: FunctionCall {
                    name: "ask_user".to_string(),
                    arguments: r#"{"question": "你希望我怎么处理？"}"#.to_string(),
                },
            }],
        )))
    });
    let agent_loop = make_loop(mock, 5);

    let mut messages = vec![Message::user("帮我决定一下")];
    let result = agent_loop
        .run(&mut messages, "session-1", None, "test-model", None, None)
        .await
        .unwrap();

    match result {
        AgentLoopResult::NeedsClarification {
            questions, turns, ..
        } => {
            assert_eq!(questions.len(), 1);
            assert_eq!(questions[0].question, "你希望我怎么处理？");
            assert_eq!(turns, 1);
        }
        other => panic!("期望 NeedsClarification，得到 {:?}", other),
    }
}

/// 回归测试：审批门控拒绝必须通过 `has_pending_approval()` 类型化信号
/// 降级为追问，而不是解析错误消息中的字符串标记。
#[tokio::test]
async fn test_run_approval_denied_returns_clarification() {
    use crate::executor::approval::{ApprovalWorkflow, ApprovalWorkflowConfig};
    use std::sync::Arc as StdArc;

    let mut mock = MockChatService::new();
    mock.expect_chat_completion().returning(|_| {
        // write_file 到非 test/temp/tmp 路径 → Medium 风险 → 审批拒绝
        Ok(response_with(Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: "call_write".to_string(),
                call_type: ToolCallType::Function,
                function: FunctionCall {
                    name: "write_file".to_string(),
                    arguments: r#"{"path": "project/src/main.rs", "content": "fn main() {}"}"#
                        .to_string(),
                },
            }],
        )))
    });

    let mut registry = ToolRegistry::new(SecurityPolicy::default());
    registry = registry.with_approval_workflow(StdArc::new(ApprovalWorkflow::new(
        ApprovalWorkflowConfig::default(),
    )));
    let agent_loop = AgentLoop::new(
        Arc::new(mock),
        registry,
        Arc::new(MockSessionManager),
        AgentLoopConfig {
            max_turns: 5,
            ..Default::default()
        },
    );

    let mut messages = vec![Message::user("请帮我写入文件")];
    let result = agent_loop
        .run(&mut messages, "session-1", None, "test-model", None, None)
        .await
        .unwrap();

    match result {
        AgentLoopResult::NeedsClarification { questions, .. } => {
            assert_eq!(questions.len(), 1);
            assert!(
                questions[0].question.contains("安全策略要求确认"),
                "追问应包含审批确认提示，实际: {}",
                questions[0].question
            );
        }
        other => panic!("期望审批降级为 NeedsClarification，得到 {:?}", other),
    }
}

#[tokio::test]
async fn test_run_empty_response_retries_then_completes() {
    // 空输出 = 正常结束（对齐 DSH：无工具调用即 completed）：
    // 普通轮重试一次（恢复机会），重试仍空 → Answer(空) 而非报错。
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .times(2)
        .returning(|_| Ok(response_with(Message::assistant(""))));
    let agent_loop = make_loop(mock, 5);

    let mut messages = vec![Message::user("你好")];
    let result = agent_loop
        .run(&mut messages, "session-1", None, "test-model", None, None)
        .await
        .unwrap();
    match result {
        AgentLoopResult::Answer { content, turns, .. } => {
            assert_eq!(content, "", "空输出应作为空回答结束");
            // 重试在同一轮内（turn 计数不增加）；mock times(2) 已验证 2 次 LLM 调用
            assert_eq!(turns, 1);
        }
        other => panic!("期望 Answer(空)，得到 {:?}", other),
    }
}

#[tokio::test]
async fn test_run_empty_response_wake_turn_no_retry() {
    // 唤醒轮（allow_empty_answer）：空输出直接结束，不重试（模型有意无需回复）
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .times(1)
        .returning(|_| Ok(response_with(Message::assistant(""))));
    let agent_loop = make_loop(mock, 5).with_allow_empty_answer();

    let mut messages = vec![Message::user("你好")];
    let result = agent_loop
        .run(&mut messages, "session-1", None, "test-model", None, None)
        .await
        .unwrap();
    match result {
        AgentLoopResult::Answer { content, turns, .. } => {
            assert_eq!(content, "");
            assert_eq!(turns, 1, "唤醒轮空输出不重试");
        }
        other => panic!("期望 Answer(空)，得到 {:?}", other),
    }
}

#[tokio::test]
async fn test_run_max_turns_exceeded() {
    let mut mock = MockChatService::new();
    // 每轮都返回工具调用 → 永远不结束 → 达到 max_turns 报错
    mock.expect_chat_completion()
        .returning(|_| Ok(response_with(tool_call_msg("nonexistent_tool"))));
    let agent_loop = make_loop(mock, 2);

    let mut messages = vec![Message::user("循环测试")];
    let err = agent_loop
        .run(&mut messages, "session-1", None, "test-model", None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("最大轮数"));
}

#[tokio::test]
async fn test_run_llm_error_propagates() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .returning(|_| Err(TianyanError::Custom("模型服务错误：连接超时".to_string())));
    let agent_loop = make_loop(mock, 5);

    let mut messages = vec![Message::user("测试")];
    let err = agent_loop
        .run(&mut messages, "session-1", None, "test-model", None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("LLM 调用失败"));
}

// ── 取消路径（cancel 标志） ──────────────────────────────────────

/// 预置取消标志：run 应在轮顶立即返回 Cancelled（不发起 LLM 调用）。
#[tokio::test]
async fn test_run_returns_cancelled_when_flag_pre_set() {
    // 不配置任何 LLM expectation：若循环发起调用会 panic
    let agent_loop = make_loop(MockChatService::new(), 5);
    let cancel = AtomicBool::new(true);

    let mut messages = vec![Message::user("测试")];
    let result = agent_loop
        .run(
            &mut messages,
            "session-1",
            None,
            "test-model",
            Some(&cancel),
            None,
        )
        .await
        .unwrap();

    assert!(
        matches!(result, AgentLoopResult::Cancelled { turns: 0, .. }),
        "预置取消应返回 Cancelled，实际: {:?}",
        result
    );
}

/// 流式中途取消：chunk 循环检测到 cancel 后中断，返回 Cancelled（而非错误）。
#[tokio::test]
async fn test_run_stream_returns_cancelled_mid_chunk() {
    use std::sync::atomic::Ordering;

    // mock 流式响应：发第一段后等待测试信号（期间测试置位 cancel）再发后续
    let (gate_tx, gate_rx) = tokio::sync::mpsc::channel::<()>(1);
    let mut gate_rx = Some(gate_rx);
    let mut mock = MockChatService::new();
    mock.expect_chat_completion_stream().returning(move |_| {
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(16);
        let mut gate_rx = gate_rx.take();
        tokio::spawn(async move {
            chunk_tx
                .send(Ok(stream_chunk(Some("第一段"), None, None)))
                .await
                .ok();
            if let Some(rx) = &mut gate_rx {
                rx.recv().await; // 等测试置位 cancel
            }
            chunk_tx
                .send(Ok(stream_chunk(Some("第二段"), None, None)))
                .await
                .ok();
            chunk_tx
                .send(Ok(stream_chunk(None, None, Some(TokenUsage::default()))))
                .await
                .ok();
        });
        Ok(chunk_rx)
    });
    let agent_loop = make_loop(mock, 5);

    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<AgentStreamChunk>>(8);
    let sender = StreamEventSender::new(tx);

    // 旁路任务：收到第一段 delta 后置位 cancel 并释放 mock 的 gate
    let cancel_clone = cancel.clone();
    let gate_tx = gate_tx.clone();
    let cancel_watcher = tokio::spawn(async move {
        while let Some(Ok(chunk)) = rx.recv().await {
            if chunk.chunk_type == StreamChunkType::Answer && !chunk.delta.is_empty() {
                cancel_clone.store(true, Ordering::Relaxed);
                gate_tx.send(()).await.ok();
                break;
            }
        }
    });

    let mut messages = vec![Message::user("测试")];
    let result = agent_loop
        .run_stream(
            &mut messages,
            sender,
            "session-1",
            None,
            "test-model",
            Some(&cancel),
            None,
        )
        .await
        .unwrap();
    cancel_watcher.await.ok();

    assert!(
        matches!(result, AgentLoopResult::Cancelled { .. }),
        "流式中途取消应返回 Cancelled，实际: {:?}",
        result
    );
}

// ── 流式回归测试（run_stream） ──────────────────────────────────

/// 构造单个流式 chunk（文本 / tool_call delta / usage 三选一组合）。
fn stream_chunk(
    content: Option<&str>,
    tool_calls: Option<Vec<ToolCallDelta>>,
    usage: Option<TokenUsage>,
) -> ChatCompletionChunk {
    stream_chunk_with_finish(content, tool_calls, usage, None)
}

/// 构造携带 finish_reason 的流式 chunk（OpenAI 语义：最终 chunk 携带）。
fn stream_chunk_with_finish(
    content: Option<&str>,
    tool_calls: Option<Vec<ToolCallDelta>>,
    usage: Option<TokenUsage>,
    finish_reason: Option<&str>,
) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: "chunk-1".to_string(),
        object: "chat.completion.chunk".to_string(),
        created: 0,
        model: "test".to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: DeltaContent {
                role: None,
                content: content.map(|s| s.to_string()),
                reasoning_content: None,
                tool_calls,
            },
            finish_reason: finish_reason.map(|s| s.to_string()),
        }],
        usage,
    }
}

/// 构造仅携带 reasoning_content 的流式 chunk（思考模型的推理段）。
fn stream_chunk_with_reasoning(reasoning: Option<&str>) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: "chunk-r".to_string(),
        object: "chat.completion.chunk".to_string(),
        created: 0,
        model: "test".to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: DeltaContent {
                role: None,
                content: None,
                reasoning_content: reasoning.map(|s| s.to_string()),
                tool_calls: None,
            },
            finish_reason: None,
        }],
        usage: None,
    }
}

/// mock 固定 chunk 序列的流式响应（每次调用重放同一序列）。
fn stream_mock(chunks: Vec<ChatCompletionChunk>) -> MockChatService {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion_stream().returning(move |_| {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        for chunk in &chunks {
            tx.try_send(Ok(chunk.clone())).unwrap();
        }
        Ok(rx)
    });
    mock
}

/// 收集 stream_sender 上所有 Answer 类型 delta。
async fn collect_answer_deltas(
    mut rx: tokio::sync::mpsc::Receiver<Result<AgentStreamChunk>>,
) -> Vec<String> {
    let mut deltas = Vec::new();
    while let Some(Ok(chunk)) = rx.recv().await {
        if chunk.chunk_type == StreamChunkType::Answer {
            deltas.push(chunk.delta);
        }
    }
    deltas
}

/// 流式路径：多 chunk 文本累积 —— 最终消息内容 = 各 chunk 拼接。
#[tokio::test]
async fn test_run_stream_accumulates_multi_chunk_content() {
    let chunks = vec![
        stream_chunk(Some("你好"), None, None),
        stream_chunk(Some("，世界"), None, None),
        stream_chunk(Some("！"), None, None),
    ];
    let agent_loop = make_loop(stream_mock(chunks), 5);

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(16);
    let sender = StreamEventSender::new(event_tx);

    let mut messages = vec![Message::user("流式测试")];
    let result = agent_loop
        .run_stream(
            &mut messages,
            sender,
            "session-1",
            None,
            "test-model",
            None,
            None,
        )
        .await
        .unwrap();

    match result {
        AgentLoopResult::Answer { content, turns, .. } => {
            assert_eq!(content, "你好，世界！");
            assert_eq!(turns, 1);
        }
        other => panic!("期望 Answer，得到 {:?}", other),
    }

    // chunk 顺序：stream_sender 收到的 answer_delta 顺序与发送一致
    let deltas = collect_answer_deltas(event_rx).await;
    assert_eq!(deltas, vec!["你好", "，世界", "！"]);
}

/// 流式路径：reasoning_content delta 累积进 assistant 消息并逐段推送
/// Thought 事件（修复思考模型推理内容被静默丢弃导致的空响应误判）。
#[tokio::test]
async fn test_run_stream_accumulates_reasoning_and_emits_thought() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion_stream().returning(|_| {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        // 思考段：仅 reasoning_content（无正文、无 tool_calls）
        tx.try_send(Ok(stream_chunk_with_reasoning(Some("先分析项目结构"))))
            .unwrap();
        tx.try_send(Ok(stream_chunk_with_reasoning(Some("再检查测试"))))
            .unwrap();
        // 正文段
        tx.try_send(Ok(stream_chunk(Some("审查完成"), None, None)))
            .unwrap();
        Ok(rx)
    });
    let agent_loop = make_loop(mock, 5);

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
    let sender = StreamEventSender::new(event_tx);

    let mut messages = vec![Message::user("审查项目")];
    let result = agent_loop
        .run_stream(
            &mut messages,
            sender,
            "session-1",
            None,
            "test-model",
            None,
            None,
        )
        .await
        .unwrap();

    // 最终回答不受推理段影响
    match result {
        AgentLoopResult::Answer { content, .. } => assert_eq!(content, "审查完成"),
        other => panic!("期望 Answer，得到 {:?}", other),
    }

    // assistant 消息携带累积的 reasoning_content（供后续轮次/持久化使用）
    let persisted = messages
        .iter()
        .find(|m| m.role == MessageRole::Assistant)
        .expect("应存在 assistant 消息");
    assert_eq!(
        persisted.reasoning_content.as_deref(),
        Some("先分析项目结构再检查测试")
    );

    // Thought 事件按推理段顺序实时推送（前端 thinking 渲染）
    let mut thoughts = Vec::new();
    while let Some(Ok(chunk)) = event_rx.recv().await {
        if chunk.chunk_type == StreamChunkType::Thought {
            thoughts.push(chunk.delta);
        }
    }
    assert_eq!(thoughts, vec!["先分析项目结构", "再检查测试"]);
}

/// 流式路径：同一 chunk 同时携带 reasoning_content 与 content（推理→正文
/// 切换边界）时，Thought 事件必须先于 Answer 事件——否则前端出现
/// "思考未结束正文已插入"的乱序。
#[tokio::test]
async fn test_run_stream_reasoning_before_content_in_same_chunk() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion_stream().returning(|_| {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        // 边界 chunk：同一 delta 同时携带 reasoning 结尾与正文开头
        tx.try_send(Ok(ChatCompletionChunk {
            id: "chunk-edge".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: DeltaContent {
                    role: None,
                    content: Some("正文开头".to_string()),
                    reasoning_content: Some("思考结尾".to_string()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        }))
        .unwrap();
        tx.try_send(Ok(stream_chunk(Some("正文继续"), None, None)))
            .unwrap();
        tx.try_send(Ok(stream_chunk(None, None, Some(TokenUsage::default()))))
            .unwrap();
        Ok(rx)
    });
    let agent_loop = make_loop(mock, 5);

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
    let sender = StreamEventSender::new(event_tx);

    let mut messages = vec![Message::user("测试")];
    let result = agent_loop
        .run_stream(
            &mut messages,
            sender,
            "session-1",
            None,
            "test-model",
            None,
            None,
        )
        .await
        .unwrap();
    match result {
        AgentLoopResult::Answer { content, .. } => assert_eq!(content, "正文开头正文继续"),
        other => panic!("期望 Answer，得到 {:?}", other),
    }

    // 事件顺序：Thought 必须先于 Answer（同一 chunk 内 reasoning 先发）
    let mut order = Vec::new();
    while let Some(Ok(chunk)) = event_rx.recv().await {
        match chunk.chunk_type {
            StreamChunkType::Thought => order.push("thought"),
            StreamChunkType::Answer => order.push("answer"),
            _ => {}
        }
    }
    assert_eq!(order, vec!["thought", "answer", "answer"]);
}

/// 流式路径：tool_calls delta 按 index 累积 —— 分片 id/name/arguments 最终完整。
#[tokio::test]
async fn test_run_stream_accumulates_tool_call_deltas() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion_stream().returning(|req| {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        if req.messages.len() <= 1 {
            // 第一轮：tool_call 分片 —— id/name 在首个 delta，arguments 分两次累积
            let chunks = [
                stream_chunk(
                    None,
                    Some(vec![ToolCallDelta {
                        index: 0,
                        id: Some("call_abc".to_string()),
                        call_type: Some("function".to_string()),
                        function: Some(ToolCallFunctionDelta {
                            name: Some("get_weather".to_string()),
                            arguments: Some(String::new()),
                        }),
                    }]),
                    None,
                ),
                stream_chunk(
                    None,
                    Some(vec![ToolCallDelta {
                        index: 0,
                        id: None,
                        call_type: None,
                        function: Some(ToolCallFunctionDelta {
                            name: None,
                            arguments: Some(r#"{"city":"#.to_string()),
                        }),
                    }]),
                    None,
                ),
                stream_chunk(
                    None,
                    Some(vec![ToolCallDelta {
                        index: 0,
                        id: None,
                        call_type: None,
                        function: Some(ToolCallFunctionDelta {
                            name: None,
                            arguments: Some("\"北京\"}".to_string()),
                        }),
                    }]),
                    None,
                ),
            ];
            for chunk in chunks {
                tx.try_send(Ok(chunk)).unwrap();
            }
        } else {
            // 第二轮：最终回答
            tx.try_send(Ok(stream_chunk(Some("天气：晴"), None, None)))
                .unwrap();
        }
        Ok(rx)
    });
    let agent_loop = make_loop(mock, 5);

    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
    let sender = StreamEventSender::new(event_tx);

    let mut messages = vec![Message::user("北京天气如何？")];
    let result = agent_loop
        .run_stream(
            &mut messages,
            sender,
            "session-1",
            None,
            "test-model",
            None,
            None,
        )
        .await
        .unwrap();

    match result {
        AgentLoopResult::Answer { content, turns, .. } => {
            assert_eq!(content, "天气：晴");
            assert_eq!(turns, 2);
        }
        other => panic!("期望 Answer，得到 {:?}", other),
    }

    // 第一轮累积的 assistant 消息应包含完整的 ToolCall（id/name/arguments 拼接）
    let assistant_msg = &messages[1];
    let tool_calls = assistant_msg
        .tool_calls
        .as_ref()
        .expect("第一轮应有 tool_calls");
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id, "call_abc");
    assert_eq!(tool_calls[0].function.name, "get_weather");
    assert_eq!(tool_calls[0].function.arguments, r#"{"city":"北京"}"#);
}

/// 流式路径：已有部分内容后流中断 —— 保留已收内容为截断输出
/// （finish=length），不再整体报错（对齐 DSH 中断不丢内容）。
#[tokio::test]
async fn test_run_stream_mid_stream_error_keeps_partial() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion_stream().returning(|_| {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tx.try_send(Ok(stream_chunk(Some("部分回答"), None, None)))
            .unwrap();
        tx.try_send(Ok(stream_chunk(Some("继续"), None, None)))
            .unwrap();
        // 第 2 个 chunk 之后流中断
        tx.try_send(Err(TianyanError::Custom("网络中断".to_string())))
            .unwrap();
        Ok(rx)
    });
    let agent_loop = make_loop(mock, 5);

    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
    let sender = StreamEventSender::new(event_tx);

    let mut messages = vec![Message::user("测试")];
    let result = agent_loop
        .run_stream(
            &mut messages,
            sender,
            "session-1",
            None,
            "test-model",
            None,
            None,
        )
        .await
        .unwrap();
    match result {
        AgentLoopResult::Answer { content, .. } => {
            assert_eq!(content, "部分回答继续", "应保留已收内容");
        }
        other => panic!("应返回 Answer（保留部分内容），得到 {other:?}"),
    }
}

/// 流式路径：无任何内容时流中断 —— 按失败上报（无内容可保留）。
#[tokio::test]
async fn test_run_stream_immediate_error_returns_custom() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion_stream().returning(|_| {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tx.try_send(Err(TianyanError::Custom("网络中断".to_string())))
            .unwrap();
        Ok(rx)
    });
    let agent_loop = make_loop(mock, 5);

    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
    let sender = StreamEventSender::new(event_tx);

    let mut messages = vec![Message::user("测试")];
    let err = agent_loop
        .run_stream(
            &mut messages,
            sender,
            "session-1",
            None,
            "test-model",
            None,
            None,
        )
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("agent_loop"), "应含 agent_loop 前缀：{msg}");
    assert!(msg.contains("LLM 调用失败"), "应含 LLM 调用失败：{msg}");
    assert!(msg.contains("流式接收中断"), "应含流式接收中断：{msg}");
}

/// 流式路径：usage 在最后 chunk —— 最终 token 统计正确。
#[tokio::test]
async fn test_run_stream_usage_from_final_chunk() {
    let chunks = vec![
        stream_chunk(Some("你好"), None, None),
        stream_chunk(Some("世界"), None, Some(TokenUsage::new(100, 50))),
    ];
    let agent_loop = make_loop(stream_mock(chunks), 5);

    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
    let sender = StreamEventSender::new(event_tx);

    let mut messages = vec![Message::user("流式测试")];
    let result = agent_loop
        .run_stream(
            &mut messages,
            sender,
            "session-1",
            None,
            "test-model",
            None,
            None,
        )
        .await
        .unwrap();

    match result {
        AgentLoopResult::Answer { total_tokens, .. } => {
            assert_eq!(total_tokens.prompt_tokens, 100);
            assert_eq!(total_tokens.completion_tokens, 50);
            assert_eq!(total_tokens.total_tokens, 150);
        }
        other => panic!("期望 Answer，得到 {:?}", other),
    }
}

/// 流式路径：answer_delta 顺序与发送 chunk 顺序一致。
#[tokio::test]
async fn test_run_stream_answer_delta_order_preserved() {
    let chunks = vec![
        stream_chunk(Some("A"), None, None),
        stream_chunk(Some("B"), None, None),
        stream_chunk(Some("C"), None, None),
        stream_chunk(Some("D"), None, None),
    ];
    let agent_loop = make_loop(stream_mock(chunks), 5);

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(16);
    let sender = StreamEventSender::new(event_tx);

    let mut messages = vec![Message::user("顺序测试")];
    let result = agent_loop
        .run_stream(
            &mut messages,
            sender,
            "session-1",
            None,
            "test-model",
            None,
            None,
        )
        .await
        .unwrap();
    assert!(matches!(result, AgentLoopResult::Answer { .. }));

    let deltas = collect_answer_deltas(event_rx).await;
    assert_eq!(deltas, vec!["A", "B", "C", "D"]);
}

// ── finish_reason 链路（T4） ─────────────────────────────────

/// 非流式路径：响应 choice 的 finish_reason → 持久化 StructuredMessage.finish。
#[tokio::test]
async fn test_run_persists_finish_reason() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion().returning(|_| {
        let mut resp = response_with(Message::assistant("完成"));
        resp.choices[0].finish_reason = Some("length".to_string());
        Ok(resp)
    });
    let agent_loop = make_loop(mock, 5);

    let mut messages = vec![Message::user("测试")];
    let result = agent_loop
        .run(&mut messages, "session-1", None, "test-model", None, None)
        .await
        .unwrap();

    match result {
        AgentLoopResult::Answer {
            persisted_message, ..
        } => {
            assert_eq!(persisted_message.finish.as_deref(), Some("length"));
        }
        other => panic!("期望 Answer，得到 {:?}", other),
    }
}

/// 流式路径：最终 chunk 的 finish_reason=stop → 持久化消息 finish。
#[tokio::test]
async fn test_run_stream_persists_finish_reason() {
    let chunks = vec![
        stream_chunk(Some("你好"), None, None),
        stream_chunk_with_finish(Some("世界"), None, None, Some("stop")),
    ];
    let agent_loop = make_loop(stream_mock(chunks), 5);

    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
    let sender = StreamEventSender::new(event_tx);

    let mut messages = vec![Message::user("流式测试")];
    let result = agent_loop
        .run_stream(
            &mut messages,
            sender,
            "session-1",
            None,
            "test-model",
            None,
            None,
        )
        .await
        .unwrap();

    match result {
        AgentLoopResult::Answer {
            persisted_message, ..
        } => {
            assert_eq!(persisted_message.finish.as_deref(), Some("stop"));
        }
        other => panic!("期望 Answer，得到 {:?}", other),
    }
}

/// 流式路径：全部 chunk 无 finish_reason → finish 为 None（行为不变兼容）。
#[tokio::test]
async fn test_run_stream_no_finish_reason_keeps_none() {
    let chunks = vec![
        stream_chunk(Some("你好"), None, None),
        stream_chunk(Some("世界"), None, None),
    ];
    let agent_loop = make_loop(stream_mock(chunks), 5);

    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
    let sender = StreamEventSender::new(event_tx);

    let mut messages = vec![Message::user("流式测试")];
    let result = agent_loop
        .run_stream(
            &mut messages,
            sender,
            "session-1",
            None,
            "test-model",
            None,
            None,
        )
        .await
        .unwrap();

    match result {
        AgentLoopResult::Answer {
            persisted_message, ..
        } => {
            assert_eq!(persisted_message.finish, None);
        }
        other => panic!("期望 Answer，得到 {:?}", other),
    }
}

// ── 动态 max_tokens（T6） ────────────────────────────────────

/// 128k/16k 规格的模型规格，供 T6 测试复用。
fn spec_128k_16k() -> ModelSpec {
    ModelSpec {
        context_length: 128_000,
        max_output_tokens: 16_000,
        max_input_tokens: 112_000,
    }
}

/// 非流式：小消息 + 128k 规格 → 请求带 max_tokens = Some(16_000)（预算远超 cap）。
#[tokio::test]
async fn test_run_sets_dynamic_max_tokens_with_spec() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion().returning(|req| {
        assert_eq!(
            req.max_tokens,
            Some(16_000),
            "小消息 + 128k 窗口应达到输出上限 16_000"
        );
        Ok(response_with(Message::assistant("回答")))
    });
    let agent_loop = make_loop(mock, 5).with_chat_spec(Some(spec_128k_16k()));

    let mut messages = vec![Message::user("帮我做点事")];
    let result = agent_loop
        .run(&mut messages, "session-1", None, "test-model", None, None)
        .await
        .unwrap();
    assert!(matches!(result, AgentLoopResult::Answer { .. }));
}

/// 校准跨轮：首轮响应 usage.prompt_tokens=120_000 → 次轮 max_tokens=4096
/// （剩余 8_000×0.9=7_200 → pow2_floor=4096）。
#[tokio::test]
async fn test_run_calibrates_max_tokens_from_prior_usage() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_in_mock = calls.clone();
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .times(2)
        .returning(
            move |req| match calls_in_mock.fetch_add(1, Ordering::Relaxed) {
                0 => {
                    assert_eq!(
                        req.max_tokens,
                        Some(16_000),
                        "首轮无实测输入 → 估算 → 达到输出上限"
                    );
                    let mut resp = response_with(Message::assistant("第一轮"));
                    resp.usage = CommonTokenUsage::new(120_000, 0);
                    Ok(resp)
                }
                _ => {
                    assert_eq!(
                        req.max_tokens,
                        Some(4_096),
                        "120_000 实测 → 剩余 8_000×0.9=7_200 → pow2_floor=4096"
                    );
                    Ok(response_with(Message::assistant("第二轮")))
                }
            },
        );
    let agent_loop = make_loop(mock, 5).with_chat_spec(Some(spec_128k_16k()));

    let mut messages = vec![Message::user("帮我做点事")];
    let r1 = agent_loop
        .run(&mut messages, "session-1", None, "test-model", None, None)
        .await
        .unwrap();
    let r2 = agent_loop
        .run(&mut messages, "session-1", None, "test-model", None, None)
        .await
        .unwrap();
    assert!(matches!(r1, AgentLoopResult::Answer { .. }));
    assert!(matches!(r2, AgentLoopResult::Answer { .. }));
    assert_eq!(calls.load(Ordering::Relaxed), 2, "应恰好发起两次请求");
}

/// 流式：with_stream(true) 请求同样携带 max_tokens = Some(16_000)。
#[tokio::test]
async fn test_run_stream_sets_dynamic_max_tokens_with_spec() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion_stream().returning(|req| {
        assert_eq!(
            req.max_tokens,
            Some(16_000),
            "流式请求应携带动态 max_tokens"
        );
        assert_eq!(req.stream, Some(true));
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tx.try_send(Ok(stream_chunk(Some("你好"), None, None)))
            .unwrap();
        Ok(rx)
    });
    let agent_loop = make_loop(mock, 5).with_chat_spec(Some(spec_128k_16k()));

    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
    let sender = StreamEventSender::new(event_tx);

    let mut messages = vec![Message::user("流式测试")];
    let result = agent_loop
        .run_stream(
            &mut messages,
            sender,
            "session-1",
            None,
            "test-model",
            None,
            None,
        )
        .await
        .unwrap();
    assert!(matches!(result, AgentLoopResult::Answer { .. }));
}

/// 1024 门槛：预置实测输入使剩余预算 < 1024 → 返回 Err 且不发送请求。
#[tokio::test]
async fn test_run_errors_when_budget_below_min() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .returning(|_| panic!("预算不足时不应发送请求"));
    let agent_loop = make_loop(mock, 5).with_chat_spec(Some(spec_128k_16k()));
    // 剩余 1137 × 0.9 = 1023 → pow2_floor = 512 < 1024 → dynamic_max_tokens 返回 None
    agent_loop
        .last_input_usage
        .store(128_000 - 1137, Ordering::Relaxed);

    let mut messages = vec![Message::user("测试")];
    let err = agent_loop
        .run(&mut messages, "session-1", None, "test-model", None, None)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("上下文预算不足"),
        "错误应提示预算不足，实际: {err}"
    );
}

/// 无规格：chat_spec=None → 不设 max_tokens、不报错（行为保持现状）。
#[tokio::test]
async fn test_run_without_spec_skips_max_tokens() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion().returning(|req| {
        assert_eq!(req.max_tokens, None, "无 chat_spec 时不应设置 max_tokens");
        Ok(response_with(Message::assistant("回答")))
    });
    let agent_loop = make_loop(mock, 5); // 默认 chat_spec = None

    let mut messages = vec![Message::user("测试")];
    let result = agent_loop
        .run(&mut messages, "session-1", None, "test-model", None, None)
        .await
        .unwrap();
    assert!(matches!(result, AgentLoopResult::Answer { .. }));
}

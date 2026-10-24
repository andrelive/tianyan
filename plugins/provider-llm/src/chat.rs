// 本模块已完全不依赖 async-openai：请求体与响应解析都是 JSON（见
// `convert_messages` / `parse_chat_response` 的文档）。
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use tianyan::common::error::{Result, TianyanError};
use tianyan::common::types::{Message, MessageRole, TokenUsage};
use tianyan::config::{ProviderDialect, ThinkingField, ThinkingParam};
use tianyan::model::types::{
    ChatChoice, ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ToolCall,
    ToolCallType, ToolChoice,
};
use tianyan::model::ChatService;

use tianyan_provider_client::{classify_http_status, RetryableFailure};

use crate::client::ChatClient;

#[async_trait]
impl ChatService for ChatClient {
    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        // 请求构建与调用在下方重试闭包内完成（每次重试重建请求）。

        // 非流式调用内置指数退避重试（全局默认策略，对齐 opencode/dsh-llm-retry）：
        // 与流式路径同构（ADR-046）：统一走发送单点 `send_chat_request`——
        // 自定义请求头（UA / 动态会话头）与状态码语义分类（429/5xx 重试判定）
        // 全路径生效；不再经 async-openai 的 create_byot（其不支持 per-request 头）。
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let response = tianyan::model::retry::with_retry(
            &self.retry_policy,
            |e: &RetryableFailure| e.retryable,
            || async {
                // 请求体直接以 JSON 构造（见 `convert_messages` 的文档）：没有
                // 「类型 → to_value → 方言改写」的中间态，方言层就在这份 JSON 上作业。
                let mut body = Self::build_body(self.dialect, &request, false);
                if let Some(effort) = request.thinking_effort.as_deref() {
                    if effort != "off" {
                        apply_thinking_params(
                            &mut body,
                            effort,
                            self.thinking_param_for(&request.model),
                        );
                    }
                }
                let resp = self
                    .send_chat_request(&url, &body, "模型服务错误", request.session_id.as_deref())
                    .await?;
                resp.json::<Value>().await.map_err(|e| RetryableFailure {
                    retryable: false,
                    message: format!("模型服务错误：解析响应失败：{}", e),
                })
            },
        )
        .await
        .map_err(|e| TianyanError::Custom(e.message))?;

        Ok(parse_chat_response(&response))
    }

    async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<mpsc::Receiver<Result<ChatCompletionChunk>>> {
        // 显式请求流式 usage（对齐 DSH/OpenAI 规范）：流式响应默认不返回 usage，
        // 除非请求携带 stream_options: {include_usage: true}——否则 ollama 等严格
        // 遵守规范的网关永远不返回 usage，上下文占用只能走估算。
        //
        // 请求体直接以 JSON 构造（见 `convert_messages` 的文档）。下面一律走手写
        // SSE 解析（create_raw_stream）：async-openai 0.34 不解析 DeepSeek 思考模型的
        // reasoning_content 增量字段——推理内容在反序列化时被静默丢弃，模型"想完
        // 没说话"（content 空 + 仅 reasoning）会被误判为空响应。
        let mut body = Self::build_body(self.dialect, &request, true);
        if let Some(effort) = request.thinking_effort.as_deref() {
            if effort != "off" {
                apply_thinking_params(&mut body, effort, self.thinking_param_for(&request.model));
            }
        }
        self.create_raw_stream(body, request.session_id.as_deref())
            .await
    }
}

/// 把思考强度档位映射为请求体参数（**形态由参数决定**，档位值不做本地翻译）：
/// - [`ThinkingParam::ThinkingBudget`]（DashScope Qwen 思考族）：`enable_thinking: true` +
///   `thinking_budget`（已知档位映射预算，未知档位用默认预算 4096）
/// - [`ThinkingParam::ReasoningEffort`]（其余 OpenAI 兼容族）：仅 `reasoning_effort: <原档位值>`
///   （原样透传，OpenAI 标准强度参数）
///
/// **`enable_thinking` 只属于 DashScope 方言，不得无条件附加**：其余族不带该字段时
/// 思考照常输出（2026-09-29 实测 deepseek / ollama / glm 各渠道），而严格校验的上游会
/// 拒绝整个请求——opencode zen 的 `glm-5.3-flash` 对 `enable_thinking` 返回 400
/// （`Upstream request failed: ... unknown field "enable_thinking"`），曾致会话轮中断
/// （此前实现依赖「DeepSeek 等实测容忍附加参数」，在多上游网关下不成立）。
///
/// 形态解析在调用方**单点**完成（[`ChatClient::thinking_param_for`]：
/// 模型级显式 > provider 方言 > 模型名嗅探）；本函数只负责按形态写参数。
///
/// 档位为 "off" 时调用方不进入本函数，不附加任何参数（模型默认行为）。
fn apply_thinking_params(body: &mut Value, effort: &str, param: ThinkingParam) {
    match param {
        ThinkingParam::ThinkingBudget => {
            body["enable_thinking"] = Value::Bool(true);
            let budget = match effort {
                "low" => 1024,
                "medium" => 4096,
                "high" => 16384,
                _ => 4096,
            };
            body["thinking_budget"] = Value::Number(budget.into());
        }
        ThinkingParam::ReasoningEffort => {
            body["reasoning_effort"] = Value::String(effort.to_string());
        }
    }
}

impl ChatClient {
    /// 手写 SSE 流式补全：POST {base_url}/chat/completions（body 已含 stream: true），
    /// 逐行解析 data: 事件为内部 ChatCompletionChunk 并经 channel 返回。
    ///
    /// async-openai 0.34 的流式反序列化丢弃 DeepSeek 思考模型的
    /// reasoning_content 增量字段（其 chunk 结构体无此字段），本方法绕开
    /// async-openai 直接解析原始 SSE，保留推理过程增量；非思考模型不携带
    /// 该字段，解析结果与原有 async-openai 路径一致。
    ///
    /// `session_id`：动态请求头求值输入（ADR-046；None = 非会话请求，
    /// 注入点走稳定兜底值）。
    async fn create_raw_stream(
        &self,
        body: Value,
        session_id: Option<&str>,
    ) -> Result<mpsc::Receiver<Result<ChatCompletionChunk>>> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        // 请求发送阶段内置指数退避重试（全局默认策略，对齐 opencode/dsh-llm-retry）：
        // HTTP 429 / 5xx / 网络 / 超时 自动退避重试；一旦进入 2xx，流读取失败不再整段重发。
        let response = tianyan::model::retry::with_retry(
            &self.retry_policy,
            |e: &RetryableFailure| e.retryable,
            || self.send_chat_request(&url, &body, "模型服务错误", session_id),
        )
        .await
        .map_err(|e| TianyanError::Custom(e.message))?;

        let mut byte_stream = response.bytes_stream();
        let (tx, rx) = mpsc::channel(100);
        let read_idle = self.read_idle;
        let first_token = self.first_token;
        // 方言在 spawn 前取出（与 read_idle 同款：闭包内不再触达 self）
        let dialect = self.dialect;

        tokio::spawn(async move {
            let mut buf: Vec<u8> = Vec::new();
            let mut first_byte_seen = false;
            loop {
                // 空闲超时：相邻两块之间静默超过 provider timeout 即报错——
                // 活跃流（持续吐块）总时长不受限；纯传输层 read_timeout
                // 对停滞 body 不可靠，必须在此显式保证。
                //
                // 取消传导（结构性取消的最后一环）：接收端 drop（用户停止 /
                // 消费者取消）→ `tx.closed()` 立即就绪 → 退出读取并断开连接，
                // 不等下一块数据或空闲超时。
                // 双预算（ADR-044）：首字节前用**首 token 预算**（覆盖大上下文
                // 预填充——上游在算 prompt，长时间无输出是正常的）；首字节后切
                // **块间空闲**（增量/心跳会重置）。旧实现共用一个预算：长预填充
                // 被当空闲误杀 → 重试再被杀（「首字符一直回不来」）。
                let budget = if first_byte_seen {
                    read_idle
                } else {
                    first_token
                };
                let next = tokio::select! {
                    r = tokio::time::timeout(budget, byte_stream.next()) => match r {
                        Ok(item) => item,
                        Err(_) => {
                            let message = if first_byte_seen {
                                format!(
                                    "模型服务错误：流式读取空闲超时（{}s 无数据）",
                                    read_idle.as_secs()
                                )
                            } else {
                                format!(
                                    "模型服务错误：首 token 超时（{}s 内无任何输出——预填充超预算或服务不可达）",
                                    first_token.as_secs()
                                )
                            };
                            let _ = tx.send(Err(TianyanError::Custom(message))).await;
                            return;
                        }
                    },
                    _ = tx.closed() => return,
                };
                match next {
                    Some(Ok(bytes)) => {
                        first_byte_seen = true;
                        buf.extend_from_slice(&bytes);
                        drain_sse_lines(&mut buf, &tx, dialect).await;
                    }
                    Some(Err(e)) => {
                        let _ = tx
                            .send(Err(TianyanError::Custom(format!(
                                "模型服务错误：流读取失败：{}",
                                e
                            ))))
                            .await;
                        return;
                    }
                    None => {
                        // 流结束：处理残留缓冲（无换行的最后一个 data 行）
                        drain_sse_lines(&mut buf, &tx, dialect).await;
                        return;
                    }
                }
            }
        });

        Ok(rx)
    }

    /// 发送一次 chat completions 请求并检查状态（供重试循环调用；每次调用重建请求）。
    ///
    /// 流式（SSE）、非流式与 VLM（JSON 响应）**共用**：本函数只负责「发送 +
    /// 静态 / 动态请求头 + 状态码判定 + 语义分类」，不关心响应体形态。
    /// `err_prefix` 是调用方场景前缀（`模型服务错误` / `VLM 服务错误`），
    /// 使错误消息归属正确；`session_id` 是动态请求头求值输入（ADR-046；
    /// None = 非会话请求 → 稳定兜底值）。
    pub(super) async fn send_chat_request(
        &self,
        url: &str,
        body: &Value,
        err_prefix: &str,
        session_id: Option<&str>,
    ) -> std::result::Result<reqwest::Response, RetryableFailure> {
        let mut req = self.http.post(url).json(body);
        for (k, v) in &self.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        // 动态请求头（ADR-046）：静态之后应用（后写覆盖）；变量无值（非会话
        // 请求）→ 用稳定兜底标识注入——网关可能要求该头必须存在，缺失会 400
        // （如 opencode 的 `MissingSessionID`，会静默杀死压缩 / 摘要等整类非
        // 对话请求）；值非法 → warn + 跳过（头是附属信息，不得拖死请求本体）。
        for (name, source) in &self.dynamic_headers {
            let value = source
                .resolve(session_id)
                .unwrap_or_else(|| tianyan::config::HeaderSource::NON_SESSION_FALLBACK.to_string());
            match reqwest::header::HeaderValue::from_str(&value) {
                Ok(parsed) => req = req.header(name.clone(), parsed),
                Err(e) => {
                    tracing::warn!(header = %name, error = %e, "动态请求头值非法，跳过");
                }
            }
        }
        if !self.api_key.is_empty() && !self.headers.contains_key("Authorization") {
            req = req.bearer_auth(&self.api_key);
        }
        let started = std::time::Instant::now();
        let response = req.send().await.map_err(|e| {
            let retryable = tianyan::model::retry::should_retry_transport(&e);
            let raw = format!("{err_prefix}：请求失败：{e}");
            let message = if e.is_timeout() {
                TianyanError::timeout(raw).to_string()
            } else {
                raw
            };
            RetryableFailure { retryable, message }
        })?;
        let status = response.status();
        tracing::debug!(
            url = %url,
            status = %status,
            version = ?response.version(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "chat 请求已收到响应头（等响应头耗时 = elapsed_ms）"
        );
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            let detail: String = detail.chars().take(300).collect();
            let retryable = tianyan::model::retry::should_retry_http(status);
            let raw = format!("{err_prefix}：请求被拒绝（HTTP {status}）：{detail}");
            return Err(RetryableFailure {
                retryable,
                message: classify_http_status(status, &raw),
            });
        }
        Ok(response)
    }

    /// 把请求的通用参数写入请求体（JSON 直接构造，理由见 `convert_messages`）。
    ///
    /// 字段名与取值形态对齐 OpenAI Chat Completions wire 格式；`None` 一律不写
    /// 字段（不写 null）——与改造前 `Option` + `skip_serializing_if` 的字节一致。
    fn apply_shared_params(body: &mut Value, request: &ChatCompletionRequest) {
        if let Some(max_tokens) = request.max_tokens {
            body["max_completion_tokens"] = json!(max_tokens);
        }
        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
        if let Some(top_p) = request.top_p {
            body["top_p"] = json!(top_p);
        }
        if let Some(ref stop) = request.stop {
            body["stop"] = json!(stop);
        }
        if let Some(presence_penalty) = request.presence_penalty {
            body["presence_penalty"] = json!(presence_penalty);
        }
        if let Some(frequency_penalty) = request.frequency_penalty {
            body["frequency_penalty"] = json!(frequency_penalty);
        }
        if let Some(ref tools) = request.tools {
            let tools_json: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.function.name,
                            "description": t.function.description,
                            "parameters": t.function.parameters,
                        },
                    })
                })
                .collect();
            body["tools"] = Value::Array(tools_json);
        }
        if let Some(ref tool_choice) = request.tool_choice {
            body["tool_choice"] = match tool_choice {
                ToolChoice::Auto => json!("auto"),
                ToolChoice::None => json!("none"),
                ToolChoice::Function { function } => json!({
                    "type": "function",
                    "function": { "name": function.name },
                }),
            };
        }
    }

    /// 构造一份请求体（流式/非流式共用；**测试直接断言它的 wire 形态**——
    /// `stream_options` 这类硬编码字段只有在这里才守得住）。
    ///
    /// 只负责「消息 + 通用参数 + 思考内容回传」；思考强度参数由调用方按方言
    /// 追加（`apply_thinking_params`），因为它的形态解析需要 `self` 的模型级配置。
    fn build_body(
        dialect: ProviderDialect,
        request: &ChatCompletionRequest,
        stream: bool,
    ) -> Value {
        let mut body = if stream {
            // 显式请求流式 usage（OpenAI 规范：流式默认不返回 usage）——否则
            // ollama 等严格网关永远不返回 usage，上下文占用只能走估算。
            json!({
                "model": request.model,
                "messages": convert_messages(&request.messages),
                "stream": true,
                "stream_options": { "include_usage": true },
            })
        } else {
            json!({
                "model": request.model,
                "messages": convert_messages(&request.messages),
                "stream": false,
            })
        };
        if !stream {
            if let Some(n) = request.n {
                body["n"] = json!(n.min(u8::MAX as usize));
            }
        }
        Self::apply_shared_params(&mut body, request);
        // 历史思考内容回传（DeepSeek 思考模型要求：携带 tools 的请求必须完整
        // 回传 reasoning_content，即使该轮未实际进行工具调用）。
        inject_reasoning_content(&mut body, &request.messages, dialect.thinking_field);
        body
    }
}

/// 从 SSE 缓冲中逐行提取完整的 data: 事件并送入 channel；
/// 缓冲中未以换行结尾的残留数据留待后续字节到达或流结束时再处理。
///
/// 容错契约（对齐 DSH 流式协议：usage 可附着 finish chunk 或作为尾随的
/// usage-only chunk 到达，两者都不应中断或刷屏）：
/// - `[DONE]`：正常流结束标记；
/// - usage-only 块（`{"choices":[],"usage":...}` / 兼容层 `{"choices":[],
///   "cost":"..."}`）：结构化识别后静默跳过（非错误）；
/// - 真正的错误对象（带 `error` 字段）：照常上报并终止；
/// - 其它无法解析的行：debug 级记录后跳过（兼容未知扩展字段）。
async fn drain_sse_lines(
    buf: &mut Vec<u8>,
    tx: &mpsc::Sender<Result<ChatCompletionChunk>>,
    dialect: ProviderDialect,
) {
    loop {
        let Some(nl) = buf.iter().position(|&b| b == b'\n') else {
            return;
        };
        let line: Vec<u8> = buf.drain(..=nl).collect();
        // 去掉行尾换行符；空行与注释行（: keep-alive）直接跳过
        let line = String::from_utf8_lossy(&line[..line.len().saturating_sub(1)]);
        let line = line.trim();
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            continue;
        }
        // 先解析原始 JSON：既能反序列化 chunk，又能从 usage 原始字段提取缓存命中
        // （DeepSeek prompt_cache_hit_tokens 等不进 async-openai 类型，须在此补充）。
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            tracing::debug!(line = %data, "跳过非 JSON 流式数据行");
            continue;
        };
        match serde_json::from_value::<ChatCompletionChunk>(value.clone()) {
            Ok(mut chunk) => {
                // 空块过滤：choices 为空且无 usage（如兼容层纯 cost 块
                // `{"choices":[],"cost":"..."}`）反序列化成功但无内容可下发，
                // 静默跳过（不产生空块、不产生错误）。
                if chunk.choices.is_empty() && chunk.usage.is_none() {
                    tracing::debug!(line = %data, "流式空块（无 choices 无 usage，跳过）");
                    continue;
                }
                if let (Some(usage_value), Some(usage)) = (value.get("usage"), chunk.usage.as_mut())
                {
                    extract_cache_tokens(usage_value, usage, dialect);
                }
                if tx.send(Ok(chunk)).await.is_err() {
                    return;
                }
            }
            Err(_) => {
                // 结构化分流：先识别 usage-only / cost 块（正常流尾，静默跳过），
                // 再识别错误对象（上报），其余按 debug 记录（不刷屏）。
                if value.get("error").is_some() {
                    let _ = tx
                        .send(Err(TianyanError::Custom(format!(
                            "模型服务错误：提供商返回错误：{}",
                            value
                        ))))
                        .await;
                    return;
                }
                // usage-only / cost 块：choices 为空（或缺失）且无 delta 内容。
                // 这是 include_usage=true 的正常流尾（OpenAI 规范：附加一个
                // 空 choices 的 usage chunk 在 [DONE] 之前）——usage 必须保留
                // 下发（上下文占用/校准依赖它），不能静默丢弃。
                let choices_empty = value
                    .get("choices")
                    .and_then(|c| c.as_array())
                    .map(|c| c.is_empty())
                    .unwrap_or(true);
                let has_usage = value.get("usage").is_some() || value.get("cost").is_some();
                if choices_empty && has_usage {
                    // 从原始 JSON 提取 usage 并构造带 usage 的空 chunk 下发
                    if let Ok(usage) = serde_json::from_value::<TokenUsage>(
                        value.get("usage").cloned().unwrap_or(serde_json::json!({})),
                    ) {
                        if usage.prompt_tokens > 0 || usage.total_tokens > 0 {
                            let mut tail = ChatCompletionChunk {
                                id: value
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                object: "chat.completion.chunk".to_string(),
                                created: value.get("created").and_then(|v| v.as_i64()).unwrap_or(0),
                                model: value
                                    .get("model")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                choices: vec![],
                                usage: Some(usage),
                            };
                            if let (Some(usage_value), Some(usage_slot)) =
                                (value.get("usage"), tail.usage.as_mut())
                            {
                                extract_cache_tokens(usage_value, usage_slot, dialect);
                            }
                            tracing::debug!(
                                usage = ?tail.usage,
                                "流式 usage-only 尾块（include_usage）"
                            );
                            if tx.send(Ok(tail)).await.is_err() {
                                return;
                            }
                            continue;
                        }
                    }
                    tracing::debug!(line = %data, "流式 cost 块（无 usage，跳过）");
                    continue;
                }
                // 非标准 finish_reason（如网关的 network_error）：typed 反序列化
                // 会失败，但这是合法的流结束信号——显式上抛保留可诊断性，
                // 绝不静默丢弃（曾导致断流看起来像"正常结束"）。
                let raw_finish = value
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("finish_reason"))
                    .and_then(|f| f.as_str());
                if let Some(reason) = raw_finish {
                    let _ = tx
                        .send(Err(TianyanError::Custom(format!(
                            "模型服务错误：Provider finish_reason: {}",
                            reason
                        ))))
                        .await;
                    return;
                }
                tracing::debug!(line = %data, "跳过无法解析的流式数据行");
                continue;
            }
        }
    }
}

/// 缓存命中字段的全字段探测链（方言声明未命中时的兜底；顺序与历史行为一致）。
fn probe_cache_tokens(usage_value: &Value) -> Option<u64> {
    usage_value
        .get("prompt_cache_hit_tokens")
        .or_else(|| usage_value.get("input_cache_hit_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| {
            usage_value
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(Value::as_u64)
        })
}

/// 从提供商原始 usage JSON 提取缓存命中 token 数。
///
/// **声明优先 + 探测兜底**：先按 provider 方言声明的字段路径取
/// （`dialect.cache_field`；各家标识不同：DeepSeek `prompt_cache_hit_tokens`、
/// DashScope `input_cache_hit_tokens`、OpenAI `prompt_tokens_details.cached_tokens`），
/// 未命中时回落全字段探测链 [`probe_cache_tokens`]。
///
/// 保留兜底是刻意的：该差异失败性质为「可见」（探不到即 0、语义无歧义），兜底
/// 不引入静默错误，却消除了「用户配错 dialect 就静默丢缓存统计」这一新失败模式
/// （声明只负责更精确，不负责唯一）。
///
/// `cache_write` 提供商一般不下发，保持 0。
fn extract_cache_tokens(usage_value: &Value, usage: &mut TokenUsage, dialect: ProviderDialect) {
    let declared = dialect
        .cache_field
        .wire_path()
        .iter()
        .try_fold(usage_value, |node, key| node.get(*key))
        .and_then(Value::as_u64);
    if let Some(hit) = declared.or_else(|| probe_cache_tokens(usage_value)) {
        usage.cache_read = hit as usize;
    }
}

/// 构造一条消息的 `content` 值（**user 与 tool 两个角色共用**）：携带多模态
/// 片段时输出数组格式（文本 + 图片），否则保持纯文本（与旧行为一致——纯文本
/// 走字符串形式，上游兼容性最好）。
///
/// tool 结果带图时（ADR-050），tool 消息的 `content` 就是这份内容块数组
/// （`[{type:"text"},{type:"image_url"},…]`）；纯文本工具结果仍是字符串，
/// 与改造前逐字节一致。
///
/// 返回 [`Value`] 而非类型化请求消息：OpenAI 兼容生态的方言差异（字段名、
/// 数组元素形状、非标准扩展字段）无法被任何库的类型枚举穷举——例如
/// `role: "tool"` 的 `content` 在 DeepSeek Chat Completions 上可以是内容块数组
/// （实测 200），而严格实现的兼容网关只接受字符串。请求体一律以 JSON 构造，
/// 方言修饰层（[`inject_reasoning_content`] / [`apply_thinking_params`]）直接
/// 在同一份 `Value` 上作业，不再有「类型 → JSON → 改写」的中间态。
fn content_value(m: &Message) -> Value {
    let Some(parts) = &m.content_parts else {
        return Value::String(m.content.clone());
    };
    if parts.is_empty() {
        return Value::String(m.content.clone());
    }

    let parts_json: Vec<Value> = parts
        .iter()
        .map(|p| {
            if p.content_type == "image_url" {
                match &p.image_url {
                    Some(url) => {
                        let mut image_url = json!({ "url": url.url });
                        // 只透传标准档位：未知取值不透传——严格上游会因枚举外取值
                        // 拒绝整个请求（与改造前的行为一致）。
                        if let Some(detail) = url
                            .detail
                            .as_deref()
                            .filter(|d| matches!(*d, "low" | "high" | "auto"))
                        {
                            image_url["detail"] = Value::String(detail.to_string());
                        }
                        json!({ "type": "image_url", "image_url": image_url })
                    }
                    None => json!({ "type": "text", "text": "[图片（内容缺失）]" }),
                }
            } else {
                json!({ "type": "text", "text": p.text.clone().unwrap_or_default() })
            }
        })
        .collect();

    Value::Array(parts_json)
}

/// 把历史 assistant 消息的 reasoning_content 注入序列化后的请求体。
///
/// async-openai 0.34 的 `ChatCompletionRequestAssistantMessage` 没有
/// `reasoning_content` 字段，类型化构造会静默丢弃。DeepSeek 思考模型要求：
/// 携带 tools 的请求必须完整回传 reasoning_content（即使该轮未实际进行
/// 工具调用），否则多轮工具调用会报错/丢失上下文。序列化后按索引回填。
/// `convert_messages` 对每条 [`Message`] 恰好生成一条请求消息，索引一一对应。
///
/// **字段名按 provider 方言决定**（[`ThinkingField`]）：DeepSeek / OpenAI 生态
/// 用 `reasoning_content`，ollama 的 OpenAI 兼容层用 `reasoning`。发错字段名
/// 不会报错（服务端忽略未知字段），但思考内容会被静默丢弃、根本没进 prompt。
fn inject_reasoning_content(body: &mut Value, messages: &[Message], dialect: ThinkingField) {
    let Some(arr) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    let field = dialect.wire_name();
    for (msg, entry) in messages.iter().zip(arr.iter_mut()) {
        if msg.role != MessageRole::Assistant {
            continue;
        }
        let Some(reasoning) = msg.reasoning_content.as_deref() else {
            continue;
        };
        if reasoning.is_empty() {
            continue;
        }
        entry[field] = Value::String(reasoning.to_string());
    }
}

/// 把内部消息转换为 OpenAI 兼容请求消息（JSON 构造，不依赖库类型）。
///
/// 为什么脱离类型化构造（阶段 1 的核心动机）：
/// - **方言差异无法穷举**：`role: "tool"` 的 `content` 在 DeepSeek 上可以是内容块
///   数组，而库的类型（`ChatCompletionRequestToolMessageContentPart` 仅有 `Text`）
///   把这条能力直接堵死；
/// - **少一层中间态**：过去是「类型构造 → `serde_json::to_value` → 方言改写」，
///   现在直接构造目标 JSON，方言层在同一份 `Value` 上作业。
///
/// **逐字节兼容纪律**（改这里前先读）：空 assistant 正文**省略 `content` 字段**
/// （与 async-openai `skip_serializing_if = "Option::is_none"` 的既有序列化一致）——
/// 写成 `null` 会被部分网关拒绝，写成 `""` 会让请求与历史字节不一致。
fn convert_messages(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| match m.role {
            MessageRole::System => json!({ "role": "system", "content": m.content }),
            MessageRole::User => json!({ "role": "user", "content": content_value(m) }),
            MessageRole::Assistant => {
                let mut msg = json!({ "role": "assistant" });
                // 空正文省略该字段（纯工具调用轮的原样回放；见函数文档）
                if !m.content.is_empty() {
                    msg["content"] = Value::String(m.content.clone());
                }
                if let Some(calls) = &m.tool_calls {
                    let tool_calls: Vec<Value> = calls
                        .iter()
                        .map(|c| {
                            // 健壮性：历史中的工具调用参数若为非法 JSON（如流式截断/
                            // 模型中途停止导致的截断参数），降级为 {} 再发送——否则上游
                            // 因单个坏工具调用 400 拒绝整轮请求，污染后续所有对话。
                            let arguments =
                                if serde_json::from_str::<Value>(&c.function.arguments).is_ok() {
                                    c.function.arguments.clone()
                                } else {
                                    tracing::warn!(
                                        tool = %c.function.name,
                                        id = %c.id,
                                        "工具调用参数非合法 JSON，降级为 {{}} 发送"
                                    );
                                    "{}".to_string()
                                };
                            json!({
                                "id": c.id,
                                "type": "function",
                                "function": {
                                    "name": c.function.name,
                                    "arguments": arguments,
                                },
                            })
                        })
                        .collect();
                    msg["tool_calls"] = Value::Array(tool_calls);
                }
                msg
            }
            MessageRole::Tool => {
                let mut msg = json!({
                    "role": "tool",
                    "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                });
                // ADR-050：tool 结果带图时 content 是内容块数组；纯文本仍是字符串
                // （与改造前逐字节一致）
                msg["content"] = content_value(m);
                msg
            }
        })
        .collect()
}

/// 响应里的角色字符串转换为内部角色。
///
/// 未知取值（含旧的 `function` 角色）一律回退 assistant，与改造前库枚举映射的
/// 兜底行为一致。
fn parse_role(role: Option<&str>) -> MessageRole {
    match role {
        Some("system") => MessageRole::System,
        Some("user") => MessageRole::User,
        Some("tool") => MessageRole::Tool,
        _ => MessageRole::Assistant,
    }
}

/// 解析非流式聊天响应（JSON 宽容提取）。
///
/// 为什么不反序列化成库类型：OpenAI 兼容生态的**响应**字段同样不可穷举，
/// usage 的缓存计数在 DeepSeek / DashScope / OpenAI 三家各有形状（见
/// [`extract_cache_tokens`]），任何未知扩展字段都会让严格类型反序列化整条失败，
/// 而失败形态是「整轮对话不可用」。这里逐字段宽容取值：缺失走默认、类型不符
/// 忽略、未知 `finish_reason` 原样透传。
fn parse_chat_response(value: &Value) -> ChatCompletionResponse {
    let choices = value
        .get("choices")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|c| ChatChoice {
                    index: c.get("index").and_then(Value::as_u64).unwrap_or(0) as usize,
                    message: parse_assistant_message(c.get("message")),
                    finish_reason: c
                        .get("finish_reason")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default();

    ChatCompletionResponse {
        id: value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        object: "chat.completion".to_string(),
        created: value.get("created").and_then(Value::as_i64).unwrap_or(0),
        model: value
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        choices,
        usage: parse_usage(value.get("usage")),
    }
}

/// 从响应 JSON 提取助手消息。
///
/// `tool_calls` 只接受 `type: "function"`（与改造前库枚举的 `Function` 分支一致），
/// 其余类型静默跳过：不让一个未知调用类型毁掉整轮回复。
fn parse_assistant_message(msg: Option<&Value>) -> Message {
    let mut out = Message {
        role: MessageRole::Assistant,
        content: String::new(),
        content_parts: None,
        tool_calls: None,
        tool_call_id: None,
        tool_duration_ms: None,
        tool_error: None,
        reasoning_content: None,
    };
    let Some(msg) = msg else {
        return out;
    };
    out.role = parse_role(msg.get("role").and_then(Value::as_str));
    out.content = msg
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    out.tool_calls = msg.get("tool_calls").and_then(Value::as_array).map(|arr| {
        arr.iter()
            .filter(|tc| tc.get("type").and_then(Value::as_str) == Some("function"))
            .filter_map(|tc| {
                let function = tc.get("function")?;
                Some(ToolCall {
                    id: tc
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    call_type: ToolCallType::Function,
                    function: tianyan::model::types::FunctionCall {
                        name: function
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        arguments: function
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    },
                })
            })
            .collect()
    });
    out
}

/// 解析 usage（缺失或类型不符一律默认值）。
///
/// 缓存命中只取 OpenAI 标准的 `prompt_tokens_details.cached_tokens`，与改造前一致。
/// DeepSeek 顶层 `prompt_cache_hit_tokens` 在非流式路径尚未提取（流式路径已由
/// [`extract_cache_tokens`] 覆盖三家形状）——这是重构前就存在的统计缺口，
/// 不是本次引入，另行处理。
fn parse_usage(usage: Option<&Value>) -> TokenUsage {
    let Some(u) = usage else {
        return TokenUsage::default();
    };
    let mut tu = TokenUsage::new(
        u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0) as usize,
        u.get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
    );
    if let Some(cached) = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
    {
        tu.cache_read = cached as usize;
    }
    tu
}

/// 响应解析的**宽容性基线**：这是阶段 2 的核心风险点——手写解析一旦对未知
/// 扩展字段过敏，失败形态是「整轮对话不可用」。以下用例锁定四条契约：
/// 完整响应正确提取、空/未知字段不 panic、未知 finish_reason 原样透传、
/// 非 function 类型与残缺 tool_calls 静默跳过。
#[cfg(test)]
mod response_parse_tests {
    use super::*;

    #[test]
    fn test_parse_full_response() {
        let v = serde_json::json!({
            "id": "chatcmpl-1",
            "created": 1700000000,
            "model": "deepseek-v4.1-flash",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "好的",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "read_file", "arguments": "{\"path\":\"a.rs\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 20,
                "prompt_tokens_details": { "cached_tokens": 64 }
            }
        });
        let r = parse_chat_response(&v);
        assert_eq!(r.id, "chatcmpl-1");
        assert_eq!(r.created, 1700000000);
        assert_eq!(r.model, "deepseek-v4.1-flash");
        assert_eq!(r.choices.len(), 1);
        assert_eq!(r.choices[0].message.content, "好的");
        assert_eq!(r.choices[0].message.role, MessageRole::Assistant);
        assert_eq!(r.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        let calls = r.choices[0]
            .message
            .tool_calls
            .as_ref()
            .expect("应有工具调用");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "read_file");
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(r.usage.prompt_tokens, 100);
        assert_eq!(r.usage.completion_tokens, 20);
        assert_eq!(r.usage.cache_read, 64);
    }

    #[test]
    fn test_parse_tolerates_empty_and_unknown_fields() {
        let r = parse_chat_response(&serde_json::json!({}));
        assert_eq!(r.id, "");
        assert!(r.choices.is_empty());
        assert_eq!(r.usage.prompt_tokens, 0);

        // 未知顶层字段不影响解析（方言扩展）
        let r2 = parse_chat_response(&serde_json::json!({
            "id": "x",
            "future_field": { "a": 1 },
            "choices": [{ "message": { "role": "assistant", "content": "y" } }]
        }));
        assert_eq!(r2.id, "x");
        assert_eq!(r2.choices[0].message.content, "y");
    }

    #[test]
    fn test_parse_unknown_finish_reason_passthrough() {
        let v = serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": "x" },
                "finish_reason": "some_new_reason"
            }]
        });
        let r = parse_chat_response(&v);
        assert_eq!(
            r.choices[0].finish_reason.as_deref(),
            Some("some_new_reason"),
            "未知取值原样透传，不丢弃整条 choice"
        );
    }

    #[test]
    fn test_parse_skips_non_function_and_incomplete_tool_calls() {
        let v = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [
                        { "id": "c1", "type": "custom", "function": { "name": "x", "arguments": "{}" } },
                        { "id": "c2", "type": "function" }
                    ]
                }
            }]
        });
        let r = parse_chat_response(&v);
        let calls = r.choices[0].message.tool_calls.as_ref();
        assert_eq!(
            calls.map(|c| c.len()),
            Some(0),
            "非 function 类型与缺 function 字段的调用都跳过"
        );
    }
}

#[cfg(test)]
mod convert_tests {
    use super::*;
    use tianyan::common::types::Message;
    use tianyan::model::types::{FunctionCall, ToolCall, ToolCallType};

    #[test]
    fn test_convert_system_message() {
        let msg = Message::system("You are a helpful assistant");
        let converted = convert_messages(&[msg]);
        assert_eq!(converted.len(), 1);
    }

    #[test]
    fn test_convert_user_message() {
        let msg = Message::user("Hello");
        let converted = convert_messages(&[msg]);
        assert_eq!(converted.len(), 1);
    }

    #[test]
    fn test_convert_user_message_with_images_produces_array() {
        let msg =
            Message::user_with_images("描述这张图", vec!["data:image/png;base64,AAAA".to_string()]);
        let converted = convert_messages(&[msg]);
        assert_eq!(converted[0]["role"], serde_json::json!("user"));
        let parts = converted[0]["content"]
            .as_array()
            .expect("带图片的 User 消息应为数组格式");
        assert_eq!(parts.len(), 2, "文本 + 图片共 2 个片段");
        assert_eq!(parts[0]["type"], serde_json::json!("text"));
        assert_eq!(parts[0]["text"], serde_json::json!("描述这张图"));
        assert_eq!(parts[1]["type"], serde_json::json!("image_url"));
        assert_eq!(
            parts[1]["image_url"]["url"],
            serde_json::json!("data:image/png;base64,AAAA")
        );
    }

    #[test]
    fn test_convert_user_message_plain_text_unchanged() {
        let msg = Message::user("Hello");
        let converted = convert_messages(&[msg]);
        assert_eq!(
            converted[0]["content"],
            serde_json::json!("Hello"),
            "无图片时保持纯文本字符串形式（向后兼容）"
        );
    }

    #[test]
    fn test_convert_assistant_with_tools() {
        let msg = Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: "call_abc".to_string(),
                call_type: ToolCallType::Function,
                function: FunctionCall {
                    name: "search".to_string(),
                    arguments: r#"{"q":"test"}"#.to_string(),
                },
            }],
        );
        let converted = convert_messages(&[msg]);
        assert_eq!(converted.len(), 1);
    }

    #[test]
    fn test_convert_assistant_sanitizes_invalid_tool_arguments() {
        // 流式截断导致的非法 JSON 参数：发送前降级为 {}，避免上游 400
        let msg = Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: "call_bad".to_string(),
                call_type: ToolCallType::Function,
                function: FunctionCall {
                    name: "apply_patch".to_string(),
                    // 未闭合的 JSON（模拟截断）
                    arguments: r#"{"patch": "*** Begin Patch"#.to_string(),
                },
            }],
        );
        let converted = convert_messages(&[msg]);
        let calls = converted[0]["tool_calls"]
            .as_array()
            .expect("应有 tool_calls");
        assert_eq!(calls[0]["type"], serde_json::json!("function"));
        assert_eq!(calls[0]["function"]["arguments"], serde_json::json!("{}"));
    }

    #[test]
    fn test_convert_tool_message_with_id() {
        let msg = Message::tool("call_abc", r#"{"result":"ok"}"#);
        let converted = convert_messages(&[msg]);
        assert_eq!(converted.len(), 1);
    }

    #[test]
    fn test_convert_plain_tool_message_content_is_string() {
        // ADR-050 改造后纯文本工具结果的 content 仍是**字符串**，与改造前一致
        let original = r#"{"result":"ok","path":"a.rs"}"#;
        let msg = Message::tool("call_abc", original);
        let converted = convert_messages(&[msg]);
        assert_eq!(converted[0]["role"], serde_json::json!("tool"));
        assert_eq!(
            converted[0]["content"],
            serde_json::json!(original),
            "纯文本工具结果 content 必须是字符串"
        );
        assert_eq!(converted[0]["tool_call_id"], serde_json::json!("call_abc"));
    }

    #[test]
    fn test_convert_tool_message_with_images_produces_array() {
        // ADR-050：tool 结果直通图片——content 是内容块数组（文本 + 图片）
        let original = r#"{"image":"session_images/a.png"}"#;
        let msg = Message::tool_with_images(
            "call_img",
            original,
            vec!["data:image/png;base64,AAAA".to_string()],
        );
        let converted = convert_messages(&[msg]);
        assert_eq!(converted[0]["role"], serde_json::json!("tool"));
        assert_eq!(converted[0]["tool_call_id"], serde_json::json!("call_img"));
        let parts = converted[0]["content"]
            .as_array()
            .expect("带图片的 tool 消息 content 应为数组");
        assert_eq!(parts.len(), 2, "文本 + 图片共 2 个片段");
        assert_eq!(parts[0]["type"], serde_json::json!("text"));
        assert_eq!(parts[0]["text"], serde_json::json!(original));
        assert_eq!(parts[1]["type"], serde_json::json!("image_url"));
        assert_eq!(
            parts[1]["image_url"]["url"],
            serde_json::json!("data:image/png;base64,AAAA")
        );
    }

    #[test]
    fn test_inject_reasoning_content_roundtrip() {
        // 携带 tools 的请求必须完整回传 reasoning_content（DeepSeek 思考模型要求）
        let mut assistant = Message::assistant("42");
        assistant.reasoning_content = Some("思考过程".to_string());
        let messages = vec![
            Message::system("You are helpful"),
            Message::user("What is 6*7?"),
            assistant,
        ];
        let converted = convert_messages(&messages);
        let body = serde_json::to_value(converted).unwrap();
        // 模拟完整请求体（messages 数组）
        let mut request_body = serde_json::json!({ "messages": body });
        inject_reasoning_content(
            &mut request_body,
            &messages,
            ThinkingField::ReasoningContent,
        );
        let arr = request_body["messages"].as_array().unwrap();
        assert!(arr[0].get("reasoning_content").is_none(), "system 不注入");
        assert!(arr[1].get("reasoning_content").is_none(), "user 不注入");
        assert_eq!(
            arr[2]["reasoning_content"].as_str(),
            Some("思考过程"),
            "assistant 的 reasoning_content 必须回传"
        );
    }

    #[test]
    fn test_inject_reasoning_content_skips_empty() {
        let mut assistant = Message::assistant("42");
        assistant.reasoning_content = Some(String::new());
        let messages = vec![assistant];
        let converted = convert_messages(&messages);
        let mut request_body = serde_json::json!({ "messages": converted });
        inject_reasoning_content(
            &mut request_body,
            &messages,
            ThinkingField::ReasoningContent,
        );
        let arr = request_body["messages"].as_array().unwrap();
        assert!(
            arr[0].get("reasoning_content").is_none(),
            "空 reasoning 不注入"
        );
    }

    #[test]
    fn test_inject_reasoning_content_multi_assistant_index_alignment() {
        // 多条 assistant 消息：索引必须一一对应（convert_messages 每条 Message 生成一条请求消息）
        let mut a1 = Message::assistant("first");
        a1.reasoning_content = Some("思考一".to_string());
        let mut a2 = Message::assistant("second");
        a2.reasoning_content = Some("思考二".to_string());
        let messages = vec![a1, a2];
        let converted = convert_messages(&messages);
        let mut request_body = serde_json::json!({ "messages": converted });
        inject_reasoning_content(
            &mut request_body,
            &messages,
            ThinkingField::ReasoningContent,
        );
        let arr = request_body["messages"].as_array().unwrap();
        assert_eq!(arr[0]["reasoning_content"].as_str(), Some("思考一"));
        assert_eq!(arr[1]["reasoning_content"].as_str(), Some("思考二"));
    }

    /// ollama 方言：历史思考必须写在 `reasoning` 上（而非 `reasoning_content`）。
    ///
    /// 回归锁定：ollama 的 OpenAI 兼容层只解析 `reasoning`，`reasoning_content`
    /// 会被静默丢弃（实测：prompt_tokens 零增长；类型探针 400 仅对 reasoning 报错）。
    #[test]
    fn test_inject_reasoning_content_ollama_dialect() {
        let mut assistant = Message::assistant("42");
        assistant.reasoning_content = Some("思考过程".to_string());
        let messages = vec![
            Message::system("You are helpful"),
            Message::user("What is 6*7?"),
            assistant,
        ];
        let converted = convert_messages(&messages);
        let mut request_body = serde_json::json!({ "messages": converted });
        inject_reasoning_content(&mut request_body, &messages, ThinkingField::Ollama);
        let arr = request_body["messages"].as_array().unwrap();
        assert_eq!(
            arr[2]["reasoning"].as_str(),
            Some("思考过程"),
            "ollama 方言必须写 reasoning 字段"
        );
        assert!(
            arr[2].get("reasoning_content").is_none(),
            "ollama 方言不得再写 reasoning_content（避免双写）"
        );
    }

    /// 两个方言互斥：同一条消息在两个方言下写不同字段名，不双写。
    #[test]
    fn test_inject_reasoning_content_dialects_are_exclusive() {
        let mut assistant = Message::assistant("42");
        assistant.reasoning_content = Some("x".to_string());
        let messages = vec![assistant];
        let converted = convert_messages(&messages);

        let mut ds = serde_json::json!({ "messages": converted.clone() });
        inject_reasoning_content(&mut ds, &messages, ThinkingField::ReasoningContent);
        assert!(ds["messages"][0].get("reasoning_content").is_some());
        assert!(ds["messages"][0].get("reasoning").is_none());

        let mut ol = serde_json::json!({ "messages": converted });
        inject_reasoning_content(&mut ol, &messages, ThinkingField::Ollama);
        assert!(ol["messages"][0].get("reasoning").is_some());
        assert!(ol["messages"][0].get("reasoning_content").is_none());
    }

    #[test]
    fn test_stream_options_include_usage_in_request() {
        // 流式请求必须携带 stream_options.include_usage=true（OpenAI 规范：
        // 流式默认不返回 usage，除非显式请求）——否则 ollama 等规范网关
        // 永远不返回 usage，上下文占用只能走估算。
        //
        // 断言的是 `build_body`——`chat_completion_stream` 的真实生产者。阶段 1
        // 去掉 builder 中间层后请求体就是 JSON，所以这里守的是 wire 字节本身。
        let request =
            ChatCompletionRequest::new("test-model", vec![Message::user("hi")]).with_stream(true);
        let body = ChatClient::build_body(
            ProviderDialect::for_preset(tianyan::config::DialectPreset::OpenAiCompatible),
            &request,
            true,
        );
        assert_eq!(
            body["stream_options"]["include_usage"],
            serde_json::json!(true),
            "流式请求必须显式请求 usage"
        );
        assert_eq!(body["stream"], serde_json::json!(true));
        assert_eq!(body["model"], serde_json::json!("test-model"));
    }
}

/// 阶段0 基线：方言相关纯函数的现状行为锁定（D2 缓存字段探测 / D3 思考参数映射）。
///
/// 这些测试是「方言单点化」重构的**行为等价基线**：重构只改变判定的来源
/// （散落的模型名嗅探 → [`ProviderDialect`] 单点），不改变任何一条已实测的
/// wire 行为。重构后这些断言必须原样通过，否则即为行为漂移。
///
/// 注（2026-09-29）：`apply_thinking_params_*` 中关于 `enable_thinking` 的断言是
/// **有意行为修复**（发送面收窄至 DashScope 方言；opencode glm 400 事故），非重构漂移。
#[cfg(test)]
mod dialect_baseline_tests {
    use super::*;
    use tianyan::config::DialectPreset;

    /// 预设 → 方言（测试便捷入口）。
    fn dialect(preset: DialectPreset) -> ProviderDialect {
        ProviderDialect::for_preset(preset)
    }

    // ── D2：缓存命中的三种提供商标识（声明优先 + 探测兜底）──

    #[test]
    fn test_extract_cache_tokens_deepseek_shape() {
        let usage = serde_json::json!({
            "prompt_tokens": 100,
            "prompt_cache_hit_tokens": 30,
            "total_tokens": 100
        });
        let mut out = TokenUsage::default();
        extract_cache_tokens(&usage, &mut out, dialect(DialectPreset::DeepSeek));
        assert_eq!(out.cache_read, 30, "DeepSeek：顶层 prompt_cache_hit_tokens");
    }

    #[test]
    fn test_extract_cache_tokens_dashscope_shape() {
        let usage = serde_json::json!({
            "prompt_tokens": 100,
            "input_cache_hit_tokens": 40,
            "total_tokens": 100
        });
        let mut out = TokenUsage::default();
        extract_cache_tokens(&usage, &mut out, dialect(DialectPreset::DashScope));
        assert_eq!(out.cache_read, 40, "DashScope：顶层 input_cache_hit_tokens");
    }

    #[test]
    fn test_extract_cache_tokens_openai_shape() {
        let usage = serde_json::json!({
            "prompt_tokens": 100,
            "total_tokens": 100,
            "prompt_tokens_details": { "cached_tokens": 50 }
        });
        let mut out = TokenUsage::default();
        extract_cache_tokens(&usage, &mut out, dialect(DialectPreset::OpenAiCompatible));
        assert_eq!(
            out.cache_read, 50,
            "OpenAI 标准：prompt_tokens_details.cached_tokens"
        );
    }

    #[test]
    fn test_extract_cache_tokens_absent_stays_zero() {
        // 无任何缓存字段（多数网关）→ 保持 0，不得误填
        let usage = serde_json::json!({ "prompt_tokens": 10, "total_tokens": 10 });
        let mut out = TokenUsage::default();
        extract_cache_tokens(&usage, &mut out, dialect(DialectPreset::OpenAiCompatible));
        assert_eq!(out.cache_read, 0);
    }

    // ── D3：思考强度的两种参数方言（模型名嗅探）──

    #[test]
    fn test_apply_thinking_params_qwen_uses_thinking_budget() {
        let mut body = serde_json::json!({});
        apply_thinking_params(
            &mut body,
            "high",
            ThinkingParam::sniff("qwen3-thinking-max"),
        );
        assert_eq!(body["enable_thinking"], serde_json::json!(true));
        assert_eq!(body["thinking_budget"], serde_json::json!(16384));
        assert!(
            body.get("reasoning_effort").is_none(),
            "Qwen 族不应附 reasoning_effort"
        );
    }

    #[test]
    fn test_apply_thinking_params_default_uses_reasoning_effort() {
        let mut body = serde_json::json!({});
        apply_thinking_params(&mut body, "high", ThinkingParam::sniff("deepseek-v4-flash"));
        assert_eq!(body["reasoning_effort"], serde_json::json!("high"));
        assert!(
            body.get("thinking_budget").is_none(),
            "非 Qwen 族不应附 thinking_budget"
        );
        assert!(
            body.get("enable_thinking").is_none(),
            "非 Qwen 族不应附 enable_thinking（2026-09-29 修复：严格上游会 400 拒绝）"
        );
    }

    #[test]
    fn test_apply_thinking_params_reasoning_effort_has_no_enable_thinking() {
        // 回归（2026-09-29 opencode glm 400 事故）：`enable_thinking` 仅属 DashScope 方言；
        // OpenAI 兼容族（DeepSeek / GLM / MiMo…）只发标准 `reasoning_effort`。
        // 判别力：旧实现无条件写入 `enable_thinking` —— 本测试必红。
        let mut body = serde_json::json!({});
        apply_thinking_params(&mut body, "max", ThinkingParam::ReasoningEffort);
        assert_eq!(body["reasoning_effort"], serde_json::json!("max"));
        assert!(
            body.get("enable_thinking").is_none(),
            "ReasoningEffort 族不得携带 enable_thinking（opencode glm 上游会拒绝整个请求）"
        );
    }

    #[test]
    fn test_apply_thinking_params_qwen_unknown_effort_falls_back() {
        // 未知档位 → 默认预算 4096（档位值由模型自己声明，可能超出已知映射）
        let mut body = serde_json::json!({});
        apply_thinking_params(&mut body, "extra-max", ThinkingParam::sniff("qwen3-max"));
        assert_eq!(body["thinking_budget"], serde_json::json!(4096));
    }

    #[test]
    fn test_apply_thinking_params_model_detection_case_insensitive() {
        for model in ["Qwen3-Max", "QWEN3-MAX", "qwen3-max"] {
            let mut body = serde_json::json!({});
            apply_thinking_params(&mut body, "low", ThinkingParam::sniff(model));
            assert_eq!(
                body["thinking_budget"],
                serde_json::json!(1024),
                "模型名大小写不敏感：{model}"
            );
        }
    }

    #[test]
    fn test_cache_tokens_declared_miss_falls_back_to_probe() {
        // 声明优先但非唯一：方言指向 OpenAI 字段，响应却是 DeepSeek 形状
        // → 必须回落探测链命中（消除「配错 dialect 就静默丢统计」的新失败模式）
        let usage = serde_json::json!({
            "prompt_cache_hit_tokens": 30,
            "total_tokens": 100
        });
        let mut out = TokenUsage::default();
        extract_cache_tokens(&usage, &mut out, dialect(DialectPreset::OpenAiCompatible));
        assert_eq!(out.cache_read, 30, "声明未命中 → 探测链兜底");
    }

    #[test]
    fn test_apply_thinking_params_respects_resolved_param() {
        // 形态由调用方单点解析后传入（client.thinking_param_for）：显式 ThinkingBudget
        // 时即使模型名不含 qwen 也走预算分支——本函数不再自行嗅探。
        let mut body = serde_json::json!({});
        apply_thinking_params(&mut body, "medium", ThinkingParam::ThinkingBudget);
        assert_eq!(body["thinking_budget"], serde_json::json!(4096));
        assert!(body.get("reasoning_effort").is_none());
    }
}

/// 流式健壮性回归（0.2.3 会话静默中断排查）：
/// 1) 非标准 finish_reason（网关 network_error）显式上抛而非静默丢弃；
/// 2) 流空闲超时：相邻块静默超过 provider timeout → Err，活跃流不受限。
#[cfg(test)]
mod stream_robustness_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::time::Duration;

    /// 本地 mock SSE 服务器：返回给定 body（Content-Length 框架），
    /// 可选在 body 后保持静默（不关闭连接）。返回实际监听地址。
    fn spawn_mock_sse(
        listener: std::net::TcpListener,
        body: String,
        stall_after_body: bool,
    ) -> std::net::SocketAddr {
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(body.as_bytes()).unwrap();
            stream.flush().unwrap();
            if stall_after_body {
                // 模拟网关停滞：body 之后持续沉默，不关闭连接
                std::thread::sleep(Duration::from_millis(2500));
            }
        });
        addr
    }

    /// 变体：调用方自带完整响应头（可用超量 Content-Length 模拟网关停滞）。
    #[allow(dead_code)]
    fn spawn_mock_sse_with_body(
        listener: std::net::TcpListener,
        header: String,
        body: String,
        stall_after_body: bool,
    ) -> std::net::SocketAddr {
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(body.as_bytes()).unwrap();
            stream.flush().unwrap();
            if stall_after_body {
                std::thread::sleep(Duration::from_millis(2500));
            }
        });
        addr
    }

    #[tokio::test]
    async fn test_unknown_finish_reason_surfaces_as_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = spawn_mock_sse(
            listener,
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"network_error\"}]}\n\n".to_string(),
            false,
        );
        let client = ChatClient::new("mock", format!("http://{}/v1", addr), "", 1, 300).unwrap();

        let mut rx = client
            .create_raw_stream(serde_json::json!({"model": "m", "stream": true}), None)
            .await
            .unwrap();

        // 非标准 finish_reason 必须显式上抛（携带原始原因），而非静默丢弃
        let item = rx.recv().await.expect("应有错误项");
        let err = item.expect_err("network_error finish_reason 应转为错误");
        assert!(
            err.to_string().contains("network_error"),
            "应携带原始原因: {err}"
        );
    }

    #[tokio::test]
    async fn test_stream_idle_timeout_fires_on_stall() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        // 声明超量 Content-Length（200）但只发一个合法块，随后网关持续停滞：
        // 1s 空闲超时必须触发 Err，而不是无限等待剩余字节
        let chunk: String = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n".to_string();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
            chunk.len() + 200
        );
        let addr = spawn_mock_sse_with_body(listener, header, chunk, true);
        let client = ChatClient::new("mock", format!("http://{}/v1", addr), "", 1, 300).unwrap();

        let started = std::time::Instant::now();
        let mut rx = client
            .create_raw_stream(serde_json::json!({"model": "m", "stream": true}), None)
            .await
            .unwrap();

        let first = rx.recv().await.expect("第一块应正常送达");
        assert!(first.is_ok(), "第一块应解析成功: {first:?}");

        let second = rx.recv().await.expect("停滞超时后应有错误项");
        let elapsed = started.elapsed();
        assert!(second.is_err(), "停滞流应转为错误: {second:?}");
        assert!(
            elapsed < Duration::from_millis(1900),
            "应在 1s 空闲超时附近失败（实际 {elapsed:?}），而不是等到服务端关闭"
        );
    }

    /// 双预算（ADR-044）：**首包预算**独立于块间空闲——大上下文预填充 / 弱网下
    /// 首个字节可能远晚于 read_idle 到达，那是正常计算而非挂死。
    /// 判别力：本测试在旧实现（统一预算）下必红——首块 1.5s > read_idle 1s。
    #[tokio::test]
    async fn test_first_token_budget_tolerates_slow_prefill() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let chunk: String = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n".to_string();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
            chunk.len()
        );
        let body = chunk.clone();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // 先收请求（读一次）——不读就写响应会让客户端在发送阶段失败
            // （reqwest `error sending request`，重试 31s 后才暴露）
            let mut req_buf = [0u8; 8192];
            let _ = stream.read(&mut req_buf);
            // 模拟预填充：1.5s 后才有首个字节（> read_idle=1s，< first_token=4s）
            std::thread::sleep(Duration::from_millis(1500));
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
            let mut buf = [0u8; 64];
            let _ = stream.read(&mut buf);
            std::thread::sleep(Duration::from_secs(3));
        });
        // read_idle = 1s（旧实现必杀）；首包预算 = 4s
        let client = ChatClient::new("mock", format!("http://{}/v1", addr), "", 1, 4).unwrap();
        let mut rx = client
            .create_raw_stream(serde_json::json!({"model": "m", "stream": true}), None)
            .await
            .unwrap();

        let first = rx.recv().await.expect("首块应在首包预算内送达");
        let chunk = first.expect("首块应解析成功");
        assert_eq!(chunk.choices.len(), 1, "预填充后的首块内容应完整");
    }

    /// 首包预算超时：错误消息必须明确指向「首 token 超时」——区别于块间空闲
    /// （让用户能分辨「预填充超预算/不可达」与「流中途挂死」）。
    #[tokio::test]
    async fn test_first_token_timeout_message_distinct_from_idle() {
        use std::io::Write as _;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // 先收请求（读一次）——不读就写响应会让客户端在发送阶段失败
            let mut req_buf = [0u8; 8192];
            let _ = stream.read(&mut req_buf);
            // 返回响应头后**永不发送任何字节**（模拟服务端挂死 / 不可达）
            let header =
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 100\r\n\r\n";
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.flush();
            std::thread::sleep(Duration::from_secs(5));
        });
        // read_idle = 30s（若误用空闲预算，测试会明显变慢）；首包预算 = 1s
        let client = ChatClient::new("mock", format!("http://{}/v1", addr), "", 30, 1).unwrap();

        let started = std::time::Instant::now();
        let mut rx = client
            .create_raw_stream(serde_json::json!({"model": "m", "stream": true}), None)
            .await
            .unwrap();

        let item = rx.recv().await.expect("应有错误项");
        let err = item.expect_err("首包超时应报错");
        assert!(
            err.to_string().contains("首 token 超时"),
            "错误消息应指向首 token 预算: {err}"
        );
        assert!(
            started.elapsed() < Duration::from_millis(2500),
            "应在首包预算（1s）附近失败，而非空闲预算（30s）：{:?}",
            started.elapsed()
        );
    }

    /// include_usage=true 的流尾：`{"choices":[],"usage":{...}}` 块（OpenAI 规范
    /// 附加在 [DONE] 之前）——usage 必须保留下发（上下文占用/校准依赖它），
    /// 不能被当作"正常流尾"静默丢弃（曾导致 ollama 等规范网关流式 usage 永远丢失）。
    #[tokio::test]
    async fn test_usage_only_tail_block_is_forwarded() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let body = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[],\"usage\":{\"prompt_tokens\":123,\"completion_tokens\":45,\"total_tokens\":168}}\n\ndata: [DONE]\n\n".to_string();
        let addr = spawn_mock_sse(listener, body, false);
        let client = ChatClient::new("mock", format!("http://{}/v1", addr), "", 1, 300).unwrap();

        let mut rx = client
            .create_raw_stream(serde_json::json!({"model": "m", "stream": true}), None)
            .await
            .unwrap();

        // 第一块：正文 delta
        let first = rx.recv().await.expect("第一块应送达");
        let chunk = first.expect("第一块应解析成功");
        assert_eq!(chunk.choices.len(), 1);

        // 第二块：usage-only 尾块——usage 必须保留（不再静默丢弃）
        let second = rx.recv().await.expect("usage-only 尾块应送达");
        let tail = second.expect("usage-only 尾块应解析为 Ok");
        assert!(
            tail.choices.is_empty(),
            "usage-only 尾块 choices 应为空，实际: {:?}",
            tail.choices.len()
        );
        let usage = tail.usage.expect("usage-only 尾块必须携带 usage");
        assert_eq!(usage.prompt_tokens, 123);
        assert_eq!(usage.completion_tokens, 45);
        assert_eq!(usage.total_tokens, 168);
    }

    /// 纯 cost 块（兼容层 `{"choices":[],"cost":"..."}`，无 usage）：
    /// 正常静默跳过，不产生错误、不产生空 usage 块。
    #[tokio::test]
    async fn test_cost_only_block_skipped() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let body = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[],\"cost\":\"0.001\"}\n\ndata: [DONE]\n\n".to_string();
        let addr = spawn_mock_sse(listener, body, false);
        let client = ChatClient::new("mock", format!("http://{}/v1", addr), "", 1, 300).unwrap();

        let mut rx = client
            .create_raw_stream(serde_json::json!({"model": "m", "stream": true}), None)
            .await
            .unwrap();

        // 无合法块：流应直接结束（cost 块被跳过），rx.recv() 返回 None
        assert!(
            rx.recv().await.is_none(),
            "仅 cost 块时流应正常结束（无输出、无错误）"
        );
    }

    /// 取消传导（结构性取消最后一环）：接收端 drop 后，producer 必须立即
    /// 退出并断开连接——不等下一块数据或空闲超时（旧实现：producer 挂在
    /// `timeout(read_idle)` 上，静默流要等 read_idle 才退出）。
    #[tokio::test]
    async fn test_raw_stream_producer_disconnects_on_receiver_drop() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let disconnected = Arc::new(AtomicBool::new(false));
        let disconnected_srv = disconnected.clone();
        // mock 服务器：发一块数据后保持静默；随后阻塞在 read——客户端断开
        // （FIN）时 read 返回，置位 disconnected。
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            // Content-Length 大于实际发送量：模拟"流未结束"（客户端不知道
            // 后续还有没有数据——旧实现将挂到空闲超时）。
            let chunk = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n";
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
                chunk.len() + 100
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(chunk.as_bytes()).unwrap();
            stream.flush().unwrap();
            let mut buf2 = [0u8; 64];
            let _ = stream.read(&mut buf2);
            disconnected_srv.store(true, Ordering::SeqCst);
        });

        // read_idle 放大到 30s：确保"旧实现等待空闲超时"的场景在测试窗口
        // （2s）内不会自然退出（本测试在旧实现下必红）。
        let client = ChatClient::new("mock", format!("http://{}/v1", addr), "", 30, 300).unwrap();
        let mut rx = client
            .create_raw_stream(serde_json::json!({"model": "m", "stream": true}), None)
            .await
            .unwrap();

        // 收到第一块后模拟消费者取消：drop 接收端（结构性取消）。
        let first = rx.recv().await.expect("应收到第一块");
        assert!(first.is_ok());
        drop(rx);

        // 断言：连接在 ~1s 内断开（producer 退出 → response drop → 连接关闭）。
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !disconnected.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            disconnected.load(Ordering::SeqCst),
            "接收端 drop 后 producer 应立即退出并断开连接（旧实现挂在 read_idle=30s）"
        );
    }

    /// 错误语义分类测试（ADR-014：KIND 前缀可被消费端谓词判定）。
    ///
    /// 锁定 HTTP 状态 → KIND 的映射契约（消费端谓词判定的根基；
    /// ADR-046 后流式 / 非流式 / VLM 三路径共用此单点）。
    #[test]
    fn test_classify_http_status_maps_semantic_kinds() {
        let raw = "模型服务错误：流式请求被拒绝（HTTP 401）：invalid api key";
        let s = classify_http_status(reqwest::StatusCode::UNAUTHORIZED, raw);
        assert!(s.starts_with("操作不被允许："), "KIND 前缀应在最前: {s}");
        assert!(TianyanError::Custom(s.clone()).is_permission(), "{s}");

        let s = classify_http_status(reqwest::StatusCode::FORBIDDEN, raw);
        assert!(TianyanError::Custom(s.clone()).is_permission(), "{s}");

        let s = classify_http_status(reqwest::StatusCode::NOT_FOUND, raw);
        assert!(TianyanError::Custom(s.clone()).is_not_found(), "{s}");

        let s = classify_http_status(reqwest::StatusCode::GATEWAY_TIMEOUT, raw);
        assert!(TianyanError::Custom(s.clone()).is_timeout(), "{s}");

        // 其它状态：原文返回（Custom 语义，不改写分类）
        let s = classify_http_status(reqwest::StatusCode::BAD_REQUEST, raw);
        assert_eq!(s, raw);
    }

    /// 捕获请求报文的 mock：读取首个请求报文写入 captured，再返回给定响应
    /// （用于断言请求头——UA / 动态会话头，ADR-046）。
    fn spawn_mock_capture(
        listener: std::net::TcpListener,
        response_header: String,
        body: String,
    ) -> (
        std::net::SocketAddr,
        std::sync::Arc<std::sync::Mutex<String>>,
    ) {
        let addr = listener.local_addr().unwrap();
        let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let captured_srv = std::sync::Arc::clone(&captured);
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            if let Ok(mut guard) = captured_srv.lock() {
                *guard = String::from_utf8_lossy(&buf[..n]).to_string();
            }
            let _ = stream.write_all(response_header.as_bytes());
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
        });
        (addr, captured)
    }

    /// ADR-046：预置命中的 provider（opencode）注入 `x-opencode-session`
    /// （值 = session_id）与专属 UA（tianyan/{VERSION}，非 reqwest 默认）。
    #[tokio::test]
    async fn test_dynamic_session_header_and_user_agent_injected() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let chunk: String = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n".to_string();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
            chunk.len()
        );
        let (addr, captured) = spawn_mock_capture(listener, header, chunk);

        let provider = tianyan::config::ProviderConfig {
            name: "opencode".to_string(),
            endpoint: format!("http://{addr}/v1"),
            api_key: Some("k".to_string()),
            ..Default::default()
        };
        let client = ChatClient::from_provider(&provider).unwrap();

        let mut rx = client
            .create_raw_stream(
                serde_json::json!({"model": "m", "stream": true}),
                Some("session-abc-123"),
            )
            .await
            .unwrap();
        while let Some(_item) = rx.recv().await {}

        let text = captured.lock().unwrap().clone();
        assert!(
            text.to_ascii_lowercase()
                .contains("x-opencode-session: session-abc-123"),
            "请求应携带会话头（值 = session_id）: {text}"
        );
        assert!(
            text.contains(&format!("tianyan/{}", tianyan::VERSION)),
            "请求应携带专属 UA（非 reqwest 默认）: {text}"
        );
    }

    /// ADR-046：非会话请求（session_id = None）使用稳定兜底值，不失败。
    ///
    /// 回归保护：网关（opencode）对缺失该头的请求直接 400（MissingSessionID）——
    /// 「跳过」策略会让压缩 / judge / 后台摘要等整类非对话请求静默失败
    /// （历史事故：上下文超阈值但压缩一直不生效）。
    #[tokio::test]
    async fn test_dynamic_session_header_fallback_without_session() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let chunk: String = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n".to_string();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
            chunk.len()
        );
        let (addr, captured) = spawn_mock_capture(listener, header, chunk);

        let provider = tianyan::config::ProviderConfig {
            name: "opencode".to_string(),
            endpoint: format!("http://{addr}/v1"),
            api_key: Some("k".to_string()),
            ..Default::default()
        };
        let client = ChatClient::from_provider(&provider).unwrap();

        let mut rx = client
            .create_raw_stream(serde_json::json!({"model": "m", "stream": true}), None)
            .await
            .unwrap();
        while let Some(_item) = rx.recv().await {}

        let text = captured.lock().unwrap().clone().to_ascii_lowercase();
        assert!(
            text.contains("x-opencode-session: tianyan-background"),
            "无 session_id 时应注入稳定兜底值: {text}"
        );
    }

    /// ADR-046：非流式路径与流式同构——同样携带动态会话头（发送单点）。
    #[tokio::test]
    async fn test_non_streaming_path_carries_session_header() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let body = r#"{"id":"c1","object":"chat.completion","created":1,"model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#.to_string();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let (addr, captured) = spawn_mock_capture(listener, header, body);

        let provider = tianyan::config::ProviderConfig {
            name: "opencode".to_string(),
            endpoint: format!("http://{addr}/v1"),
            api_key: Some("k".to_string()),
            ..Default::default()
        };
        let client = ChatClient::from_provider(&provider).unwrap();

        let request = ChatCompletionRequest::new("m", vec![Message::user("hi")])
            .with_session_id("session-nonstream-1");
        let resp = client.chat_completion(request).await.unwrap();
        assert_eq!(resp.choices[0].message.content, "hi");

        let text = captured.lock().unwrap().clone().to_ascii_lowercase();
        assert!(
            text.contains("x-opencode-session: session-nonstream-1"),
            "非流式路径应携带会话头（统一发送单点）: {text}"
        );
    }
}

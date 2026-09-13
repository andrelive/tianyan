use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::common::error::{Result, TianyanError};
use crate::common::types::{Embedding, Message, TokenUsage};
use crate::model::traits::{ChatService, EmbeddingService, VlmService};
use crate::model::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, EmbeddingRequest,
    EmbeddingResponse, VisionRequest, VisionResponse,
};

/// 带日志记录的聊天服务包装。
pub struct LoggedService<T: ChatService>(pub T);

#[async_trait]
impl<T: ChatService> ChatService for LoggedService<T> {
    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        info!(
            model = %request.model,
            "chat_completion called"
        );
        let result = self.0.chat_completion(request).await;
        match &result {
            Ok(resp) => info!(
                model = %resp.model,
                choices = resp.choices.len(),
                "chat_completion succeeded"
            ),
            Err(e) => error!(
                error = %e,
                "chat_completion failed"
            ),
        }
        result
    }

    async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<mpsc::Receiver<Result<ChatCompletionChunk>>> {
        info!(
            model = %request.model,
            "chat_completion_stream called"
        );
        self.0.chat_completion_stream(request).await
    }

    async fn chat(&self, model: &str, messages: Vec<Message>) -> Result<String> {
        info!(
            model = %model,
            "chat called"
        );
        self.0.chat(model, messages).await
    }
}

/// 嵌入用量上报槽位类型（装配层后置注入的可选句柄）。
///
/// 用共享 `Arc<Mutex<Option<...>>>` 而不是构造期参数：装配层（server）在
/// 创建模型服务之后才拿到 `UsageLog`，后置注入不必改 `from_config` 签名。
pub type EmbeddingUsageHandle =
    std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<dyn EmbeddingUsageSink>>>>;

/// 嵌入调用用量上报契约（model 层定义契约，实现由装配层注入）。
///
/// 为什么需要：嵌入调用**不走 AgentLoop**，其 token 用量此前完全不入账
/// （`usage_logs` 只有 chat provider）——账单与应用内用量统计都看不到嵌入消耗。
///
/// 实现必须**非阻塞**（例如 `tokio::spawn` 异步落库）：本方法在检索热路径上
/// 被调用，不得等待 I/O。
pub trait EmbeddingUsageSink: Send + Sync {
    /// 记录一次嵌入调用；`calls` 为本次文本条数（批量 > 1）。
    fn record(&self, model: &str, usage: &TokenUsage, calls: usize);
}

/// 查询嵌入缓存容量。
///
/// 实测（日志）：同一 query 在一次检索里被嵌入 2 次（链路多处调用点），
/// 缓存可直接省掉重复调用。
const QUERY_EMBED_CACHE_CAPACITY: usize = 256;

/// 查询嵌入缓存：键 `model|dimensions|text` → 向量。
///
/// FIFO 淘汰（不做真 LRU：对"短时间重复 query"命中率已足够，实现更简单）。
#[derive(Default)]
struct QueryEmbedCache {
    map: std::collections::HashMap<String, Vec<f32>>,
    order: std::collections::VecDeque<String>,
}

impl QueryEmbedCache {
    fn get(&self, key: &str) -> Option<Vec<f32>> {
        self.map.get(key).cloned()
    }

    fn put(&mut self, key: String, vector: Vec<f32>) {
        if self.map.contains_key(&key) {
            return;
        }
        if self.order.len() >= QUERY_EMBED_CACHE_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            }
        }
        self.map.insert(key.clone(), vector);
        self.order.push_back(key);
    }

    /// 缓存条目数（测试用）。
    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.len()
    }
}

/// 带日志 + 用量上报 + 查询缓存的嵌入服务包装。
pub struct LoggedEmbeddingService<T: EmbeddingService> {
    inner: T,
    usage_sink: EmbeddingUsageHandle,
    cache: std::sync::Mutex<QueryEmbedCache>,
}

impl<T: EmbeddingService> LoggedEmbeddingService<T> {
    /// 构造包装（`usage_sink` 为装配层后置注入的共享槽位）。
    pub fn new(inner: T, usage_sink: EmbeddingUsageHandle) -> Self {
        Self {
            inner,
            usage_sink,
            cache: std::sync::Mutex::new(QueryEmbedCache::default()),
        }
    }

    /// 当前缓存条目数（测试用）。
    #[cfg(test)]
    pub fn cache_len(&self) -> usize {
        self.cache.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// 上报一次嵌入用量（未注入 sink 时静默——嵌入照常工作）。
    fn report(&self, model: &str, usage: &TokenUsage, calls: usize) {
        let sink = self
            .usage_sink
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(sink) = sink {
            sink.record(model, usage, calls);
        }
    }

    /// 单文本嵌入（缓存 → provider → 上报用量）。
    ///
    /// 空文本直接返回错误：既无意义又白消耗一次调用额度（日志实测出现过
    /// `text_len=0` 的调用）。
    async fn embed_cached(
        &self,
        model: &str,
        text: &str,
        dimensions: Option<usize>,
    ) -> Result<Embedding> {
        if text.trim().is_empty() {
            return Err(TianyanError::Custom(
                "嵌入服务错误：文本为空，跳过无意义嵌入调用".to_string(),
            ));
        }
        let key = format!("{model}|{}|{text}", dimensions.unwrap_or(0));
        if let Some(vector) = self
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&key)
        {
            info!(model = %model, text_len = text.len(), "embed 命中缓存（跳过重复调用）");
            return Ok(Embedding::new(vector));
        }

        let mut request = EmbeddingRequest::new(model, text);
        if let Some(dim) = dimensions {
            request = request.with_dimensions(dim);
        }
        let response = self.inner.embed(request).await?;
        self.report(model, &response.usage, 1);
        let embedding = response
            .data
            .first()
            .map(|d| Embedding::new(d.embedding.clone()))
            .ok_or_else(|| TianyanError::Custom("嵌入服务错误：未返回嵌入向量".to_string()))?;
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .put(key, embedding.vector.clone());
        Ok(embedding)
    }
}

#[async_trait]
impl<T: EmbeddingService> EmbeddingService for LoggedEmbeddingService<T> {
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse> {
        info!(model = %request.model, "embed called");
        let result = self.inner.embed(request).await;
        match &result {
            Ok(resp) => {
                info!(model = %resp.model, count = resp.data.len(), "embed succeeded");
                self.report(&resp.model, &resp.usage, resp.data.len());
            }
            Err(e) => error!(error = %e, "embed failed"),
        }
        result
    }

    async fn embed_single(&self, model: &str, text: &str) -> Result<Embedding> {
        info!(model = %model, text_len = text.len(), "embed_single called");
        let result = self.embed_cached(model, text, None).await;
        match &result {
            Ok(_) => info!(model = %model, "embed_single succeeded"),
            Err(e) => error!(error = %e, "embed_single failed"),
        }
        result
    }

    async fn embed_single_with_dimensions(
        &self,
        model: &str,
        text: &str,
        dimensions: usize,
    ) -> Result<Embedding> {
        info!(
            model = %model,
            dimensions = dimensions,
            text_len = text.len(),
            "embed_single_with_dimensions called"
        );
        let result = self.embed_cached(model, text, Some(dimensions)).await;
        match &result {
            Ok(_) => info!(model = %model, "embed_single_with_dimensions succeeded"),
            Err(e) => error!(error = %e, "embed_single_with_dimensions failed"),
        }
        result
    }

    async fn embed_batch(&self, model: &str, texts: Vec<String>) -> Result<Vec<Embedding>> {
        info!(model = %model, count = texts.len(), "embed_batch called");
        let request = EmbeddingRequest::new_batch(model, texts);
        match self.inner.embed(request).await {
            Ok(resp) => {
                self.report(&resp.model, &resp.usage, resp.data.len());
                info!(model = %model, count = resp.data.len(), "embed_batch succeeded");
                Ok(resp
                    .data
                    .iter()
                    .map(|d| Embedding::new(d.embedding.clone()))
                    .collect())
            }
            Err(e) => {
                error!(error = %e, "embed_batch failed");
                Err(e)
            }
        }
    }

    fn embedding_dimension(&self, model: &str) -> usize {
        self.inner.embedding_dimension(model)
    }

    async fn embed_image(&self, model: &str, image_data: &[u8]) -> Result<Embedding> {
        info!(model = %model, image_len = image_data.len(), "embed_image called");
        // 图像走底层默认实现（base64 data URL）；字节内容不做查询缓存
        let result = self.inner.embed_image(model, image_data).await;
        match &result {
            Ok(_) => info!(model = %model, "embed_image succeeded"),
            Err(e) => error!(error = %e, "embed_image failed"),
        }
        result
    }
}

/// 带日志记录的 VLM 服务包装。
pub struct LoggedVlmService<T: VlmService>(pub T);

#[async_trait]
impl<T: VlmService> VlmService for LoggedVlmService<T> {
    async fn analyze_image(&self, request: VisionRequest) -> Result<VisionResponse> {
        info!(
            model = %request.model,
            "analyze_image called"
        );
        let result = self.0.analyze_image(request).await;
        match &result {
            Ok(resp) => info!(
                model = %resp.model,
                choices = resp.choices.len(),
                "analyze_image succeeded"
            ),
            Err(e) => error!(
                error = %e,
                "analyze_image failed"
            ),
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::types::EmbeddingData;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// 计数 Mock：记录 embed 被真实调用的次数（验证缓存是否省掉重复调用）。
    struct CountingEmbedding {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl EmbeddingService for CountingEmbedding {
        async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(EmbeddingResponse {
                object: "list".to_string(),
                data: vec![EmbeddingData {
                    index: 0,
                    embedding: vec![0.1, 0.2],
                    object: "embedding".to_string(),
                }],
                model: request.model.clone(),
                usage: TokenUsage::new(7, 0),
            })
        }

        fn embedding_dimension(&self, _model: &str) -> usize {
            2
        }
    }

    /// 记录上报内容：(model, prompt_tokens, calls)。
    #[derive(Default)]
    struct RecordingSink {
        records: Mutex<Vec<(String, usize, usize)>>,
    }

    impl EmbeddingUsageSink for RecordingSink {
        fn record(&self, model: &str, usage: &TokenUsage, calls: usize) {
            self.records
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push((model.to_string(), usage.prompt_tokens, calls));
        }
    }

    fn sink_handle() -> EmbeddingUsageHandle {
        Arc::new(Mutex::new(None))
    }

    fn service_with(
        calls: &Arc<AtomicUsize>,
        handle: EmbeddingUsageHandle,
    ) -> LoggedEmbeddingService<CountingEmbedding> {
        let _ = calls;
        LoggedEmbeddingService::new(
            CountingEmbedding {
                calls: AtomicUsize::new(0),
            },
            handle,
        )
    }

    /// 回归测试：同一 query 重复嵌入**命中缓存**——只调 provider 一次、只上报一次。
    ///
    /// 实测背景：一次检索里同一 query 被嵌入 2 次（检索链路多处调用点），
    /// 且重复调用会产生真实的 API 计费。
    #[tokio::test]
    async fn test_query_embed_cache_hits_and_reports_once() {
        let handle = sink_handle();
        let sink = Arc::new(RecordingSink::default());
        *handle.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(sink.clone() as Arc<dyn EmbeddingUsageSink>);
        let service = service_with(&Arc::new(AtomicUsize::new(0)), handle);

        let first = service
            .embed_single("text-embedding-v4", "同一个查询")
            .await
            .expect("首次嵌入应成功");
        let second = service
            .embed_single("text-embedding-v4", "同一个查询")
            .await
            .expect("二次嵌入应命中文档缓存");

        assert_eq!(first.vector, second.vector, "命中缓存应返回同一向量");
        assert_eq!(
            service.inner.calls.load(Ordering::SeqCst),
            1,
            "第二次应命中缓存，不得再调 provider"
        );
        assert_eq!(service.cache_len(), 1, "缓存应记录 1 条");
        let records = sink
            .records
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        assert_eq!(records.len(), 1, "缓存命中不应重复上报用量");
        assert_eq!(records[0], ("text-embedding-v4".to_string(), 7, 1));
    }

    /// 回归测试：**维度不同 = 不同缓存键**（不同维度的向量不可复用）。
    #[tokio::test]
    async fn test_query_embed_cache_key_includes_dimensions() {
        let service = service_with(&Arc::new(AtomicUsize::new(0)), sink_handle());
        service
            .embed_single_with_dimensions("text-embedding-v4", "文本", 1024)
            .await
            .expect("首次应成功");
        service
            .embed_single_with_dimensions("text-embedding-v4", "文本", 512)
            .await
            .expect("二次应成功");
        assert_eq!(
            service.inner.calls.load(Ordering::SeqCst),
            2,
            "维度不同不得复用缓存"
        );
        assert_eq!(service.cache_len(), 2);
    }

    /// 回归测试：**空文本短路**——不发嵌入请求（日志实测出现过 text_len=0 的调用）。
    #[tokio::test]
    async fn test_empty_text_short_circuits_without_call() {
        let service = service_with(&Arc::new(AtomicUsize::new(0)), sink_handle());
        let err = service
            .embed_single("text-embedding-v4", "   ")
            .await
            .expect_err("空文本应返回错误");
        assert!(
            err.to_string().contains("文本为空"),
            "错误应说明空文本：{err}"
        );
        assert_eq!(
            service.inner.calls.load(Ordering::SeqCst),
            0,
            "空文本不得发起嵌入请求"
        );
        assert_eq!(service.cache_len(), 0, "空文本不进缓存");
    }

    /// 未注入 sink 时上报静默（嵌入照常工作，不 panic）。
    #[tokio::test]
    async fn test_report_is_silent_without_sink() {
        let service = service_with(&Arc::new(AtomicUsize::new(0)), sink_handle());
        assert!(service.embed_single("m", "x").await.is_ok());
    }
}

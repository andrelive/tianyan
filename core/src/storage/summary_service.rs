//! 摘要服务配置和管理。
//!
//! 本模块提供摘要服务的配置和管理功能，支持定时扫描和自动摘要生成。

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::model::ChatService;
use crate::storage::types::ContextEntry;
use crate::storage::{
    ExtractionConfig, MemoryExtractionService, MemoryExtractionTrait, SummaryEngine,
    VirtualFileSystem, ABSTRACT_TOKEN_LIMIT,
};

/// 最大缓存条目数
const MAX_CACHE_ENTRIES: usize = 10_000;
/// 缓存条目最大存活时间
const CACHE_MAX_AGE_SECS: u64 = 3600;

/// 已处理 URI 缓存，带容量上限和时间淘汰。
#[derive(Debug)]
pub(crate) struct ProcessedCache {
    inner: std::collections::HashMap<String, Instant>,
}

impl ProcessedCache {
    fn new() -> Self {
        Self {
            inner: std::collections::HashMap::new(),
        }
    }

    fn contains_key(&self, uri: &str) -> bool {
        self.inner.contains_key(uri)
    }

    fn insert(&mut self, uri: String, instant: Instant) {
        if self.inner.len() >= MAX_CACHE_ENTRIES {
            // 先淘汰过期条目
            let max_age = Duration::from_secs(CACHE_MAX_AGE_SECS);
            let now = Instant::now();
            self.inner.retain(|_, t| now.duration_since(*t) < max_age);
            // 如果仍然超限，随机删除一半条目（避免 O(n log n) 排序）
            if self.inner.len() >= MAX_CACHE_ENTRIES {
                let target = MAX_CACHE_ENTRIES / 2;
                let mut to_remove = self.inner.len() - target;
                self.inner.retain(|_, _| {
                    if to_remove > 0 {
                        to_remove -= 1;
                        false
                    } else {
                        true
                    }
                });
            }
        }
        self.inner.insert(uri, instant);
    }

    fn cleanup(&mut self, max_age: Duration) {
        let now = Instant::now();
        self.inner.retain(|_, t| now.duration_since(*t) < max_age);
    }
}

/// 摘要服务配置。
///
/// 此结构体定义了摘要服务的行为参数，包括扫描间隔、启动时扫描开关和处理的类别。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryServiceConfig {
    /// 扫描间隔（秒）。
    pub scan_interval_secs: u64,
    /// 是否在启动时扫描。
    pub scan_on_startup: bool,
    /// 要处理的上下文类别列表。
    pub categories: Vec<ContextNamespace>,
}

impl Default for SummaryServiceConfig {
    fn default() -> Self {
        Self {
            scan_interval_secs: 300,
            scan_on_startup: true,
            categories: vec![
                ContextNamespace::User,
                ContextNamespace::Session,
                ContextNamespace::Memory,
                ContextNamespace::Knowledge,
                ContextNamespace::Agent,
                ContextNamespace::Skill,
            ],
        }
    }
}

impl SummaryServiceConfig {
    /// 验证配置参数。
    pub fn validate(&self) -> crate::common::error::Result<()> {
        if self.scan_interval_secs == 0 {
            return Err(crate::common::error::TianyanError::Config(
                "扫描间隔不能为 0".to_string(),
            ));
        }
        if self.categories.is_empty() {
            return Err(crate::common::error::TianyanError::Config(
                "categories 不能为空".to_string(),
            ));
        }
        Ok(())
    }
}

/// 摘要服务。
///
/// 此服务负责管理和调度上下文的自动摘要生成任务。
/// 它维护一个待处理 URI 队列，并使用 LRU 缓存跟踪已处理的条目。
pub struct SummaryService {
    /// 虚拟文件系统引用。
    pub vfs: Arc<dyn VirtualFileSystem>,
    /// 摘要引擎引用。
    pub summary_engine: Arc<SummaryEngine>,
    /// 记忆提取器（可选，用于从会话中提取记忆）。
    pub memory_extractor: Option<Arc<dyn MemoryExtractionTrait + Send + Sync>>,
    /// 服务配置。
    pub config: SummaryServiceConfig,
    /// 待处理 URI 队列。
    pub queue: RwLock<VecDeque<TianyanUri>>,
    /// 已处理 URI 缓存（URI 字符串 -> 处理时间）。
    pub(crate) processed_cache: RwLock<ProcessedCache>,
    /// 服务运行状态标志。
    pub running: AtomicBool,
}

impl SummaryService {
    /// 创建新的摘要服务。
    ///
    /// # 参数
    /// - `vfs`: 虚拟文件系统引用
    /// - `summary_engine`: 摘要引擎引用
    /// - `config`: 服务配置
    ///
    /// # 返回
    /// 新的摘要服务实例
    pub fn new(
        vfs: Arc<dyn VirtualFileSystem>,
        summary_engine: Arc<SummaryEngine>,
        config: SummaryServiceConfig,
    ) -> Self {
        Self {
            vfs,
            summary_engine,
            memory_extractor: None,
            config,
            queue: RwLock::new(VecDeque::new()),
            processed_cache: RwLock::new(ProcessedCache::new()),
            running: AtomicBool::new(false),
        }
    }

    /// 设置记忆提取器。
    ///
    /// # 参数
    /// - `memory_extractor`: 记忆提取器实例
    ///
    /// # 返回
    /// 修改后的服务实例（用于链式调用）
    pub fn with_memory_extractor(
        mut self,
        memory_extractor: Arc<dyn MemoryExtractionTrait + Send + Sync>,
    ) -> Self {
        self.memory_extractor = Some(memory_extractor);
        self
    }

    /// 从模型服务创建并设置记忆提取器。
    ///
    /// # 参数
    /// - `model_service`: 模型服务引用
    ///
    /// # 返回
    /// 修改后的服务实例（用于链式调用）
    pub fn with_memory_extractor_from_model(mut self, model_service: Arc<dyn ChatService>) -> Self {
        let extractor = Arc::new(MemoryExtractionService::new(
            model_service,
            self.vfs.clone(),
            ExtractionConfig::default(),
        ));
        self.memory_extractor = Some(extractor);
        self
    }

    /// 检查服务是否正在运行。
    ///
    /// # 返回
    /// - `true`: 服务正在运行
    /// - `false`: 服务已停止
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// 请求立即处理（插入队头，插队）。
    ///
    /// # 参数
    /// - `uri`: 要处理的 URI
    pub fn request_immediate(&self, uri: TianyanUri) -> crate::common::error::Result<()> {
        let mut queue = self.queue.write().map_err(|e| {
            crate::common::error::TianyanError::Internal(format!("写入队列锁失败：{}", e))
        })?;
        queue.push_front(uri);
        Ok(())
    }

    /// 后台任务主循环。
    ///
    /// 此方法运行一个异步循环，定期扫描 VFS 并处理队列中的任务。
    /// 使用 `tokio::select!` 实现定时扫描和队列处理的并发。
    async fn run_worker(self: Arc<Self>) {
        let mut interval =
            tokio::time::interval(Duration::from_secs(self.config.scan_interval_secs));

        while self.running.load(Ordering::SeqCst) {
            tokio::select! {
                biased;

                // 定时扫描
                _ = interval.tick() => {
                    // 定期清理过期缓存
                    self.cleanup_cache(Duration::from_secs(CACHE_MAX_AGE_SECS));

                    match self.scan_for_missing_summaries().await {
                        Ok(uris) if !uris.is_empty() => {
                            tracing::debug!("扫描到 {} 个需要摘要的条目", uris.len());
                            // 加入队尾
                            for uri in uris {
                                if self.enqueue(uri).is_ok() {
                                    // 成功入队
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!("扫描失败：{}", e);
                        }
                    }
                }

                // 默认分支：处理队列
                else => {
                    // 从队头取出一项
                    let Ok(Some(uri)) = self.dequeue() else {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    };

                    // 顺序处理（不 spawn），失败时重新入队以便重试
                    if let Err(e) = self.process_uri(&uri).await {
                        tracing::warn!("处理失败：{} - {}，将重新入队", uri, e);
                        if let Err(enqueue_err) = self.enqueue(uri.clone()) {
                            tracing::error!("重新入队失败：{} - {}", uri, enqueue_err);
                        }
                    }
                }
            }
        }
    }

    /// 启动服务。
    ///
    /// 此方法启动后台 worker 任务，开始定期扫描和处理队列。
    /// 如果服务已经在运行，则不执行任何操作。
    ///
    /// # Errors
    /// 当启动扫描失败时返回错误。
    pub async fn start(self: Arc<Self>) -> crate::common::error::Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            tracing::warn!("摘要服务已在运行");
            return Ok(());
        }

        tracing::info!("摘要服务启动...");

        // 启动时扫描
        if self.config.scan_on_startup {
            match self.scan_for_missing_summaries().await {
                Ok(uris) => {
                    tracing::info!("启动扫描发现 {} 个需要摘要的条目", uris.len());
                    for uri in uris {
                        let _ = self.enqueue(uri);
                    }
                }
                Err(e) => {
                    tracing::error!("启动扫描失败：{}", e);
                }
            }
        }

        // 启动 worker
        tokio::spawn(self.clone().run_worker());

        tracing::info!("摘要服务已启动");
        Ok(())
    }

    /// 停止服务。
    ///
    /// 此方法停止后台 worker 任务。
    /// 注意：此方法不会等待队列清空，立即停止。
    ///
    /// # Errors
    /// 当停止失败时返回错误。
    pub async fn stop(&self) -> crate::common::error::Result<()> {
        if !self.running.swap(false, Ordering::SeqCst) {
            return Ok(());
        }

        tracing::info!("摘要服务停止...");

        // 等待队列清空（可选）
        while self.queue_len() > 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tracing::info!("摘要服务已停止");
        Ok(())
    }

    /// 将 URI 添加到处理队列。
    ///
    /// # 参数
    /// - `uri`: 要处理的 URI
    ///
    /// # 返回
    /// - `Ok(true)`: URI 已成功添加到队列
    /// - `Ok(false)`: URI 已在队列中或已被处理
    /// - `Err`: 操作失败
    pub fn enqueue(&self, uri: TianyanUri) -> crate::common::error::Result<bool> {
        let uri_str = uri.to_string();

        let cache = self.processed_cache.read().map_err(|e| {
            crate::common::error::TianyanError::Internal(format!("读取缓存锁失败：{}", e))
        })?;

        if cache.contains_key(&uri_str) {
            return Ok(false);
        }
        drop(cache);

        let mut queue = self.queue.write().map_err(|e| {
            crate::common::error::TianyanError::Internal(format!("写入队列锁失败：{}", e))
        })?;

        if queue.contains(&uri) {
            return Ok(false);
        }

        queue.push_back(uri);
        Ok(true)
    }

    /// 从队列中取出下一个待处理 URI。
    ///
    /// # 返回
    /// - `Ok(Some(uri))`: 成功获取待处理的 URI
    /// - `Ok(None)`: 队列为空
    /// - `Err`: 操作失败
    pub fn dequeue(&self) -> crate::common::error::Result<Option<TianyanUri>> {
        let mut queue = self.queue.write().map_err(|e| {
            crate::common::error::TianyanError::Internal(format!("写入队列锁失败：{}", e))
        })?;
        Ok(queue.pop_front())
    }

    /// 标记 URI 为已处理。
    ///
    /// # 参数
    /// - `uri`: 已处理的 URI
    pub fn mark_processed(&self, uri: &TianyanUri) {
        let uri_str = uri.to_string();
        if let Ok(mut cache) = self.processed_cache.write() {
            cache.insert(uri_str, Instant::now());
        }
    }

    /// 检查 URI 是否已被处理。
    ///
    /// # 参数
    /// - `uri`: 要检查的 URI
    ///
    /// # 返回
    /// - `true`: URI 已被处理
    /// - `false`: URI 未被处理
    pub fn is_processed(&self, uri: &TianyanUri) -> bool {
        let uri_str = uri.to_string();
        if let Ok(cache) = self.processed_cache.read() {
            cache.contains_key(&uri_str)
        } else {
            false
        }
    }

    /// 获取队列长度。
    ///
    /// # 返回
    /// 队列中待处理 URI 的数量
    pub fn queue_len(&self) -> usize {
        if let Ok(queue) = self.queue.read() {
            queue.len()
        } else {
            0
        }
    }

    /// 清理过期的缓存条目。
    ///
    /// # 参数
    /// - `max_age`: 缓存条目的最大存活时间
    pub fn cleanup_cache(&self, max_age: Duration) {
        if let Ok(mut cache) = self.processed_cache.write() {
            cache.cleanup(max_age);
        }
    }

    /// 扫描所有类别，返回缺少摘要的 URI 列表。
    ///
    /// # 返回
    /// - `Ok(Vec<TianyanUri>)`: 缺少摘要的 URI 列表
    /// - `Err`: 扫描失败
    ///
    /// # Errors
    /// 当文件系统访问失败时返回错误。
    pub async fn scan_for_missing_summaries(
        &self,
    ) -> crate::common::error::Result<Vec<TianyanUri>> {
        let mut missing = Vec::new();

        for category in &self.config.categories {
            let root_uri = TianyanUri::new(*category, vec![]);
            self.scan_directory_recursive(&root_uri, &mut missing)
                .await?;
        }

        Ok(missing)
    }

    /// 递归扫描目录，收集缺失摘要的条目。
    ///
    /// # 参数
    /// - `uri`: 当前扫描的目录 URI
    /// - `missing`: 用于收集缺失摘要的 URI 列表
    ///
    /// # Errors
    /// 当文件系统访问失败时返回错误。
    async fn scan_directory_recursive(
        &self,
        uri: &TianyanUri,
        missing: &mut Vec<TianyanUri>,
    ) -> crate::common::error::Result<()> {
        let entries = self.vfs.list(uri).await?;

        for entry in entries {
            if entry.is_directory() {
                Box::pin(self.scan_directory_recursive(entry.uri(), missing)).await?;
            } else if self.needs_summary(&entry).await? {
                missing.push(entry.metadata.uri.clone());
            }
        }

        Ok(())
    }

    /// 检查条目是否需要生成摘要。
    ///
    /// # 参数
    /// - `entry`: 要检查的条目
    ///
    /// # 返回
    /// - `Ok(true)`: 需要生成摘要
    /// - `Ok(false)`: 不需要生成摘要
    /// - `Err`: 检查失败
    ///
    /// # Errors
    /// 当元数据读取失败时返回错误。
    async fn needs_summary(&self, entry: &ContextEntry) -> crate::common::error::Result<bool> {
        if self.is_processed(entry.uri()) {
            return Ok(false);
        }

        let metadata = self.vfs.get_all_content_metadata(entry.uri()).await?;

        let detail_meta = match metadata.get(&ContentLevel::Detail) {
            Some(m) => m,
            _ => return Ok(false),
        };

        let has_abstract = metadata.get(&ContentLevel::Abstract).is_some();
        let has_overview = metadata.get(&ContentLevel::Overview).is_some();

        if !has_abstract || !has_overview {
            return Ok(true);
        }

        let abstract_meta = match metadata.get(&ContentLevel::Abstract) {
            Some(m) => m,
            None => return Ok(true),
        };
        let overview_meta = match metadata.get(&ContentLevel::Overview) {
            Some(m) => m,
            None => return Ok(true),
        };

        if detail_meta.updated_at > abstract_meta.updated_at
            || detail_meta.updated_at > overview_meta.updated_at
        {
            return Ok(true);
        }

        Ok(false)
    }

    /// 处理单个 URI，生成摘要和向量。
    ///
    /// # 参数
    /// - `uri`: 要处理的 URI
    ///
    /// # 返回
    /// - `Ok(())`: 处理成功
    /// - `Err`: 处理失败
    ///
    /// # Errors
    /// 当读取内容、生成摘要或写入失败时返回错误。
    pub async fn process_uri(&self, uri: &TianyanUri) -> crate::common::error::Result<()> {
        let detail_content = self
            .vfs
            .read_content(uri, ContentLevel::Detail)
            .await
            .map_err(|_| {
                crate::common::error::TianyanError::SummaryGeneration(format!(
                    "无法读取 Detail: {}",
                    uri
                ))
            })?;

        let token_count = self
            .summary_engine
            .token_counter()
            .count_tokens(&detail_content);

        let (abstract_content, overview_content) = if token_count < ABSTRACT_TOKEN_LIMIT {
            tracing::debug!("短内容直接用作摘要：{} ({} tokens)", uri, token_count);
            (detail_content.clone(), detail_content)
        } else {
            tracing::debug!("长内容生成摘要：{} ({} tokens)", uri, token_count);
            self.summary_engine
                .generate_summaries(&detail_content)
                .await?
        };

        self.vfs.write_abstract(uri, &abstract_content).await?;
        self.vfs.write_overview(uri, &overview_content).await?;

        self.vfs
            .update_summary_vectors(uri, &abstract_content, &overview_content)
            .await?;

        // 如果是 Session 且配置了记忆提取器，则异步提取记忆
        if let Some(ref extractor) = self.memory_extractor {
            if uri.namespace() == ContextNamespace::Session {
                // 异步提取记忆，不阻塞主流程
                let extractor = extractor.clone();
                let session_uri = uri.clone();
                tokio::spawn(async move {
                    match extractor.extract_from_session(&session_uri).await {
                        Ok(memories) => {
                            tracing::info!("从会话提取 {} 条记忆：{}", memories.len(), session_uri);
                        }
                        Err(e) => {
                            tracing::warn!("从会话提取记忆失败：{} - {}", session_uri, e);
                        }
                    }
                });
            }
        }

        self.mark_processed(uri);

        tracing::info!("摘要生成完成：{} ({} tokens)", uri, token_count);
        Ok(())
    }
}

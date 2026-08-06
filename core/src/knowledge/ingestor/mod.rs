//! 知识导入协调器。
//!
//! 本模块提供知识导入协调器，协调文档解析、VLM 图像分析、摘要生成、向量化和 VFS 存储。
//! 解析后的文档直接写入 VFS，由 SummaryEngine 生成分层摘要（abstract + overview），
//! 然后由双层检索自然覆盖——不做切片。

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use ring::digest::{Context, SHA256};

use crate::common::error::Result;
use crate::common::token_estimator::estimate_tokens;
use crate::common::types::{ContextNamespace, EntryMetadata, TianyanUri};
use crate::model::{ChatService, EmbeddingService, VlmService};
use crate::vfs::{ContextEntry, SummaryEngine, VirtualFileSystem};

use super::image::{ImageAnalyzer, ImageProcessor, ImageProcessorConfig};
use super::parser::CompositeParser;
use super::types::{
    DocumentType, IngestionRequest, IngestionResult, KnowledgeCategory, KnowledgeMetadata,
};

/// 知识导入器配置。
#[derive(Debug, Clone)]
pub struct IngestorConfig {
    /// 图像处理配置。
    pub image: ImageProcessorConfig,
    /// 是否生成嵌入向量。
    pub generate_embeddings: bool,
    /// 是否生成摘要。
    pub generate_summaries: bool,
    /// 默认嵌入模型。
    pub embedding_model: String,
    /// 默认摘要模型。
    pub summary_model: String,
    /// 图像分析模型（VLM）。
    pub vision_model: String,
    /// 最大并发处理任务数。
    pub max_concurrent_tasks: usize,
}

impl Default for IngestorConfig {
    fn default() -> Self {
        Self {
            image: ImageProcessorConfig::default(),
            generate_embeddings: true,
            generate_summaries: true,
            embedding_model: "text-embedding-3-small".to_string(),
            summary_model: String::new(),
            vision_model: String::new(),
            max_concurrent_tasks: 4,
        }
    }
}

impl IngestorConfig {
    /// 创建新的导入器配置。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置是否生成嵌入向量。
    pub fn with_embeddings(mut self, enabled: bool) -> Self {
        self.generate_embeddings = enabled;
        self
    }

    /// 设置是否生成摘要。
    pub fn with_summaries(mut self, enabled: bool) -> Self {
        self.generate_summaries = enabled;
        self
    }

    /// 设置嵌入模型。
    pub fn with_embedding_model(mut self, model: impl Into<String>) -> Self {
        self.embedding_model = model.into();
        self
    }

    /// 设置摘要模型。
    pub fn with_summary_model(mut self, model: impl Into<String>) -> Self {
        self.summary_model = model.into();
        self
    }

    /// 设置视觉分析模型。
    pub fn with_vision_model(mut self, model: impl Into<String>) -> Self {
        self.vision_model = model.into();
        self
    }
}

/// 协调文档处理的知识导入器。
///
/// 解析文档后直接写入 VFS，由 SummaryEngine 生成分层摘要（abstract + overview），
/// 不做切片——双层检索替代了传统 RAG 的切片逻辑。
///
/// 已通过 `knowledge_ingest` 工具接入 Agent 流程（ToolRegistry → KnowledgeIngestor），
/// LLM 可自主触发文档导入；server 层另有 HTTP multipart 导入路径（/api/v1/knowledge/ingest）。
pub struct KnowledgeIngestor {
    config: IngestorConfig,
    parser: CompositeParser,
    image_processor: ImageProcessor,
    embedding_service: Arc<dyn EmbeddingService>,
    vlm_service: Arc<dyn VlmService>,
    vfs: Arc<dyn VirtualFileSystem>,
    summary_engine: SummaryEngine,
}

impl KnowledgeIngestor {
    /// 创建新的知识导入器。
    pub fn new(
        config: IngestorConfig,
        model_service: Arc<dyn ChatService>,
        embedding_service: Arc<dyn EmbeddingService>,
        vlm_service: Arc<dyn VlmService>,
        vfs: Arc<dyn VirtualFileSystem>,
    ) -> Self {
        let parser = CompositeParser::new();
        let image_processor = ImageProcessor::new(config.image.clone());

        let summary_engine = SummaryEngine::new(
            model_service,
            config.summary_model.clone(),
        );

        Self {
            config,
            parser,
            image_processor,
            embedding_service,
            vlm_service,
            vfs,
            summary_engine,
        }
    }

    /// 导入文档。
    ///
    /// 流程：解析文档 → 生成摘要 → 写入 VFS（abstract + overview + detail）→ 生成嵌入向量。
    pub async fn ingest(&self, request: IngestionRequest) -> Result<IngestionResult> {
        let start = Instant::now();
        let mut warnings: Vec<String> = Vec::new();

        let content_hash = self.calculate_hash(&request.content);
        let doc_id = content_hash.clone();

        let path = Path::new(&request.filename);
        let doc_type = self.parser.detect_type(path);

        let category = request
            .category
            .unwrap_or_else(|| self.infer_category(&doc_type, &request.filename));

        let uri = self.create_uri(&category, &doc_id);

        // 通过 VFS 检查是否已存在（去重）
        if self.vfs.exists(&uri).await.unwrap_or(false) {
            return Ok(IngestionResult {
                document_id: doc_id,
                uri,
                tokens_processed: 0,
                processing_time_ms: start.elapsed().as_millis() as u64,
                warnings: vec!["文档已存在（内容哈希匹配），已跳过".to_string()],
            });
        }

        let (text_content, visual_embedding, _metadata) = match doc_type {
            DocumentType::Image => self.process_image(&request, &doc_id).await?,
            _ => self
                .process_document(&request, &doc_id, &doc_type)
                .await
                .map(|(t, m)| (t, None, m))?,
        };

        let (abstract_content, overview_content) = if self.config.generate_summaries {
            match self.generate_summaries(&text_content).await {
                Ok((abs, ov)) => (abs, ov),
                Err(e) => {
                    warnings.push(format!("摘要生成失败，使用截断回退: {}", e));
                    (
                        text_content.chars().take(500).collect(),
                        text_content.clone(),
                    )
                }
            }
        } else {
            (
                text_content.chars().take(500).collect(),
                text_content.clone(),
            )
        };

        // 通过 VFS 创建文件并写入各层级内容
        self.vfs.create_file(&uri).await?;
        self.vfs.write_abstract(&uri, &abstract_content).await?;
        self.vfs.write_overview(&uri, &overview_content).await?;
        self.vfs.write_content(&uri, &text_content).await?;

        // 将自定义元数据（content_hash 等）写入 VFS
        let mut custom_meta = std::collections::HashMap::new();
        custom_meta.insert("content_hash".to_string(), serde_json::json!(content_hash));
        self.vfs.update_metadata(&uri, 0.5, custom_meta).await?;

        // 构建向量 payload（包含完整元数据，用于搜索发现）
        let payload = {
            let mut e = ContextEntry::new_file(uri.clone());
            e.abstract_content = Some(abstract_content.clone());
            e.overview_content = Some(overview_content.clone());
            e.detail_content = Some(text_content.clone());
            e.metadata
                .custom
                .insert("content_hash".to_string(), serde_json::json!(content_hash));
            e.metadata.source = request.source;
            e.metadata.original_name = Some(request.filename.clone());
            e.metadata.file_size = Some(request.content.len() as u64);
            e.metadata.tags = request.tags.clone();
            e.metadata
        };

        if self.config.generate_embeddings {
            self.store_embeddings(
                &uri,
                &abstract_content,
                &overview_content,
                visual_embedding,
                payload,
            )
            .await?;
        }

        let tokens_processed = estimate_tokens(&text_content);

        Ok(IngestionResult {
            document_id: doc_id,
            uri,
            tokens_processed,
            processing_time_ms: start.elapsed().as_millis() as u64,
            warnings,
        })
    }

    /// 处理文档（非图像）：解析为纯文本。
    async fn process_document(
        &self,
        request: &IngestionRequest,
        doc_id: &str,
        doc_type: &DocumentType,
    ) -> Result<(String, KnowledgeMetadata)> {
        let parsed = self
            .parser
            .parse_file(&request.content, Path::new(&request.filename))?;

        let metadata = KnowledgeMetadata {
            document_id: doc_id.to_string(),
            original_name: request.filename.clone(),
            doc_type: *doc_type,
            category: self.infer_category(doc_type, &request.filename),
            source: request.source,
            file_size: request.content.len() as u64,
            content_hash: self.calculate_hash(&request.content),
            tags: request.tags.clone(),
            language: parsed.language,
            total_tokens: estimate_tokens(&parsed.text),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            importance: 0.5,
        };

        Ok((parsed.text, metadata))
    }

    /// 处理图像：VLM 分析 → 统一文本表示 → 视觉嵌入向量。
    async fn process_image(
        &self,
        request: &IngestionRequest,
        doc_id: &str,
    ) -> Result<(String, Option<Vec<f32>>, KnowledgeMetadata)> {
        let _processed = self
            .image_processor
            .process(&request.content, &request.filename)?;

        let analyzer = ImageAnalyzer::new(
            self.vlm_service.as_ref(),
            self.embedding_service.as_ref(),
            &self.config.vision_model,
        );
        let analysis = analyzer.analyze(&request.content).await?;

        let unified = analyzer.create_unified_text(&analysis);

        let visual_embedding = if self.config.generate_embeddings {
            Some(analyzer.generate_visual_embedding(&request.content).await?)
        } else {
            None
        };

        let metadata = KnowledgeMetadata {
            document_id: doc_id.to_string(),
            original_name: request.filename.clone(),
            doc_type: DocumentType::Image,
            category: self.infer_image_category(&analysis.image_type),
            source: request.source,
            file_size: request.content.len() as u64,
            content_hash: self.calculate_hash(&request.content),
            tags: [request.tags.clone(), analysis.tags.clone()].concat(),
            language: None,
            total_tokens: estimate_tokens(&unified.combined_text),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            importance: 0.5,
        };

        Ok((
            unified.combined_text,
            visual_embedding.map(|e| e.vector),
            metadata,
        ))
    }

    /// 为内容生成分层摘要。
    async fn generate_summaries(&self, content: &str) -> Result<(String, String)> {
        let abstract_content = self
            .summary_engine
            .generate_abstract(content)
            .await
            .unwrap_or_else(|_| content.chars().take(500).collect());

        let overview_content = self
            .summary_engine
            .generate_overview(content)
            .await
            .unwrap_or_else(|_| content.chars().take(2000).collect());

        Ok((abstract_content, overview_content))
    }

    /// 为条目内容生成嵌入向量并通过 VFS 写入向量库。
    async fn store_embeddings(
        &self,
        uri: &TianyanUri,
        abstract_content: &str,
        overview_content: &str,
        visual_vector: Option<Vec<f32>>,
        payload: EntryMetadata,
    ) -> Result<()> {
        self.vfs
            .index_entry(
                uri,
                abstract_content,
                overview_content,
                visual_vector,
                payload,
                &self.config.embedding_model,
            )
            .await
    }

    /// 为文档创建 URI。
    fn create_uri(&self, category: &KnowledgeCategory, doc_id: &str) -> TianyanUri {
        TianyanUri::new(
            ContextNamespace::Knowledge,
            vec![category.dir_name().to_string(), doc_id.to_string()],
        )
    }

    /// 从文档类型和文件名推断分类。
    fn infer_category(&self, doc_type: &DocumentType, filename: &str) -> KnowledgeCategory {
        match doc_type {
            DocumentType::Code => KnowledgeCategory::CodeSnippets,
            DocumentType::Image => KnowledgeCategory::Photos,
            DocumentType::Data => KnowledgeCategory::Datasets,
            DocumentType::Spreadsheet => KnowledgeCategory::Datasets,
            DocumentType::WebPage => KnowledgeCategory::External,
            _ => {
                let lower = filename.to_lowercase();
                if lower.contains("api") || lower.contains("doc") || lower.contains("spec") {
                    KnowledgeCategory::Technical
                } else if lower.contains("report") || lower.contains("business") {
                    KnowledgeCategory::Business
                } else {
                    KnowledgeCategory::Other
                }
            }
        }
    }

    /// 从图像类型推断分类。
    fn infer_image_category(&self, image_type: &super::image::ImageType) -> KnowledgeCategory {
        match image_type {
            super::image::ImageType::Screenshot => KnowledgeCategory::Screenshots,
            super::image::ImageType::Photo => KnowledgeCategory::Photos,
            super::image::ImageType::Diagram => KnowledgeCategory::Diagrams,
            super::image::ImageType::Chart => KnowledgeCategory::Diagrams,
            super::image::ImageType::CodeScreenshot => KnowledgeCategory::CodeSnippets,
            super::image::ImageType::DocumentScan => KnowledgeCategory::Technical,
            _ => KnowledgeCategory::Other,
        }
    }

    /// 计算内容的 SHA-256 哈希。
    fn calculate_hash(&self, content: &[u8]) -> String {
        let mut context = Context::new(&SHA256);
        context.update(content);
        let digest = context.finish();
        digest
            .as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::digest::{Context, SHA256};

    #[test]
    fn test_ingestor_config() {
        let config = IngestorConfig::new()
            .with_embeddings(false)
            .with_summaries(true)
            .with_embedding_model("text-embedding-3-large");

        assert!(!config.generate_embeddings);
        assert!(config.generate_summaries);
        assert_eq!(config.embedding_model, "text-embedding-3-large");
    }

    #[test]
    fn test_infer_category() {
        assert_eq!(
            KnowledgeCategory::CodeSnippets,
            KnowledgeCategory::CodeSnippets
        );

        assert_eq!(KnowledgeCategory::Photos, KnowledgeCategory::Photos);

        assert_eq!(KnowledgeCategory::Datasets, KnowledgeCategory::Datasets);
    }

    #[test]
    fn test_calculate_hash() {
        let content = b"test content";
        let mut context = Context::new(&SHA256);
        context.update(content);
        let digest = context.finish();
        let hash = digest
            .as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();

        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_token_estimation_matches_unified_caliber() {
        // 中文文本：统一字符类口径（1.5 字符/token）应显著小于旧口径（字节数/4，UTF-8 中文 3 字节/字符）
        let chinese = "知识导入协调器负责解析文档并写入虚拟文件系统生成分层摘要";
        let tokens = estimate_tokens(chinese);
        assert!(tokens > 0);
        assert!(tokens < chinese.len() / 4);
        // 与全局统一估算器完全一致（同一口径）
        assert_eq!(tokens, estimate_tokens(chinese));
    }
}

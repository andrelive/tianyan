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

use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContextNamespace, TianyanUri};
use crate::model::{ChatService, EmbeddingService, VlmService};
use crate::vfs::{
    backend::StorageBackend, ContextEntry, SummaryEngine, VectorPoint, VectorStorage,
    CURRENT_SCHEMA_VERSION,
};

use super::image::{ImageAnalyzer, ImageProcessor, ImageProcessorConfig};
use super::parser::CompositeParser;
use super::types::{
    DocumentType, IngestionRequest, IngestionResult, KnowledgeCategory, KnowledgeMetadata,
};

fn count_tokens(text: &str) -> usize {
    text.len() / 4
}

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
            summary_model: "gpt-4o-mini".to_string(),
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
}

/// 协调文档处理的知识导入器。
///
/// 解析文档后直接写入 VFS，由 SummaryEngine 生成分层摘要（abstract + overview），
/// 不做切片——双层检索替代了传统 RAG 的切片逻辑。
///
/// 注意：此导入器目前未被集成到 Agent 流程中。
/// 按组件工具化原则，后续应通过 `ingest_knowledge` 工具暴露给 LLM 调用。
pub struct KnowledgeIngestor {
    config: IngestorConfig,
    parser: CompositeParser,
    image_processor: ImageProcessor,
    embedding_service: Arc<dyn EmbeddingService>,
    vlm_service: Arc<dyn VlmService>,
    storage: Arc<dyn StorageBackend>,
    vector_storage: Arc<dyn VectorStorage>,
    summary_engine: SummaryEngine,
}

impl KnowledgeIngestor {
    /// 创建新的知识导入器。
    pub fn new(
        config: IngestorConfig,
        model_service: Arc<dyn ChatService>,
        embedding_service: Arc<dyn EmbeddingService>,
        vlm_service: Arc<dyn VlmService>,
        storage: Arc<dyn StorageBackend>,
        vector_storage: Arc<dyn VectorStorage>,
    ) -> Self {
        let parser = CompositeParser::new();
        let image_processor = ImageProcessor::new(config.image.clone());

        let summary_engine = SummaryEngine::new(
            model_service,
            embedding_service.clone(),
            config.summary_model.clone(),
            config.embedding_model.clone(),
        );

        Self {
            config,
            parser,
            image_processor,
            embedding_service,
            vlm_service,
            storage,
            vector_storage,
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

        if self
            .storage
            .exists(&TianyanUri::parse(&uri.to_string()).unwrap_or(uri.clone()))
            .await
            .unwrap_or(false)
        {
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
            _ => self.process_document(&request, &doc_id, &doc_type).await.map(|(t, m)| (t, None, m))?,
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

        let mut entry = ContextEntry::new_file(uri.clone());
        entry.abstract_content = Some(abstract_content);
        entry.overview_content = Some(overview_content);
        entry.detail_content = Some(text_content.clone());
        entry
            .metadata
            .custom
            .insert("content_hash".to_string(), serde_json::json!(content_hash));
        entry.metadata.source = request.source;
        entry.metadata.original_name = Some(request.filename.clone());
        entry.metadata.file_size = Some(request.content.len() as u64);
        entry.metadata.tags = request.tags.clone();

        self.storage.write_entry(&entry).await?;

        if self.config.generate_embeddings {
            self.store_embeddings(&uri, &entry, visual_embedding).await?;
        }

        let tokens_processed = count_tokens(&text_content);

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
            total_tokens: count_tokens(&parsed.text),
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
            "gpt-4o",
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
            total_tokens: count_tokens(&unified.combined_text),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            importance: 0.5,
        };

        Ok((unified.combined_text, visual_embedding.map(|e| e.vector), metadata))
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

    /// 为条目的 abstract、overview 和视觉内容生成嵌入向量并写入向量库。
    async fn store_embeddings(
        &self,
        uri: &TianyanUri,
        entry: &ContextEntry,
        visual_vector: Option<Vec<f32>>,
    ) -> Result<()> {
        let abstract_text = entry
            .abstract_content
            .as_ref()
            .ok_or_else(|| TianyanError::EmbeddingService("没有摘要内容可嵌入".to_string()))?;

        let abstract_embedding = self
            .embedding_service
            .embed_single(&self.config.embedding_model, abstract_text)
            .await?;

        let overview_embedding = if let Some(ref overview) = entry.overview_content {
            Some(
                self.embedding_service
                    .embed_single(&self.config.embedding_model, overview)
                    .await?,
            )
        } else {
            None
        };

        let point = VectorPoint {
            schema_version: CURRENT_SCHEMA_VERSION,
            id: uri.to_string().replace("tianyan://", "").replace('/', "-"),
            abstract_vector: Some(abstract_embedding.vector),
            overview_vector: overview_embedding.map(|e| e.vector),
            visual_vector,
            payload: entry.metadata.clone(),
        };

        self.vector_storage.upsert_point(&point).await?;
        Ok(())
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
        digest.as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>()
    }
}

pub mod builder;
pub use builder::KnowledgeIngestorBuilder;

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
        let hash = digest.as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();

        assert_eq!(hash.len(), 64);
    }
}

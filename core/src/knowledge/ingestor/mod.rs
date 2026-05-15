//! 知识导入协调器。
//!
//! 本模块提供知识导入协调器，协调文档解析、摘要生成、向量化和存储。

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use ring::digest::{Context, SHA256};

use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContextNamespace, TianyanUri};
use crate::model::{EmbeddingService, ModelService, VisionEncoder, VlmService};
use crate::storage::{
    ContextEntry, StorageBackend, SummaryEngine, TokenCounter, VectorPoint, VectorStorage,
    CURRENT_SCHEMA_VERSION,
};

use super::chunker::{ChunkingConfig, DocumentChunker};
use super::image::{ImageAnalyzer, ImageProcessor, ImageProcessorConfig};
use super::parser::CompositeParser;
use super::types::{
    DocumentType, IngestionRequest, IngestionResult, KnowledgeCategory, KnowledgeMetadata,
};

/// 知识导入器配置。
#[derive(Debug, Clone)]
pub struct IngestorConfig {
    /// 分块配置。
    pub chunking: ChunkingConfig,
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
            chunking: ChunkingConfig::default(),
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
#[allow(dead_code)]
pub struct KnowledgeIngestor<M, E, V, VE, S, VS>
where
    M: ModelService,
    E: EmbeddingService,
    V: VlmService,
    VE: VisionEncoder,
    S: StorageBackend,
    VS: VectorStorage,
{
    config: IngestorConfig,
    parser: CompositeParser,
    chunker: DocumentChunker,
    image_processor: ImageProcessor,
    model_service: Arc<M>,
    embedding_service: Arc<E>,
    vlm_service: V,
    vision_encoder: VE,
    storage: S,
    vector_storage: VS,
    summary_engine: SummaryEngine,
}

impl<M, E, V, VE, S, VS> KnowledgeIngestor<M, E, V, VE, S, VS>
where
    M: ModelService + 'static,
    E: EmbeddingService + 'static,
    V: VlmService,
    VE: VisionEncoder,
    S: StorageBackend,
    VS: VectorStorage,
{
    /// 创建新的知识导入器。
    pub fn new(
        config: IngestorConfig,
        model_service: M,
        embedding_service: E,
        vlm_service: V,
        vision_encoder: VE,
        storage: S,
        vector_storage: VS,
    ) -> Result<Self> {
        let parser = CompositeParser::new();
        let chunker = DocumentChunker::new(config.chunking.clone())?;
        let image_processor = ImageProcessor::new(config.image.clone());

        let model_service_arc = Arc::new(model_service);
        let embedding_service_arc = Arc::new(embedding_service);

        let summary_engine = SummaryEngine::new(
            model_service_arc.clone(),
            embedding_service_arc.clone(),
            config.summary_model.clone(),
            config.embedding_model.clone(),
        )?;

        Ok(Self {
            config,
            parser,
            chunker,
            image_processor,
            model_service: model_service_arc,
            embedding_service: embedding_service_arc,
            vlm_service,
            vision_encoder,
            storage,
            vector_storage,
            summary_engine,
        })
    }

    /// 导入文档。
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
                chunks_created: 0,
                tokens_processed: 0,
                processing_time_ms: start.elapsed().as_millis() as u64,
                warnings: vec!["文档已存在（内容哈希匹配），已跳过".to_string()],
            });
        }

        let (text_content, chunks, _metadata) = match doc_type {
            DocumentType::Image => self.process_image(&request, &doc_id, &uri).await?,
            _ => {
                self.process_document(&request, &doc_id, &uri, &doc_type)
                    .await?
            }
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
            self.store_embeddings(&uri, &entry).await?;
        }

        let chunks_created = chunks.len();
        for (idx, chunk) in chunks.iter().enumerate() {
            let chunk_uri = uri.append(&format!("chunk_{}", idx));
            let mut chunk_entry = ContextEntry::new_file(chunk_uri.clone());
            chunk_entry.abstract_content = Some(chunk.text.chars().take(200).collect());
            chunk_entry.detail_content = Some(chunk.text.clone());
            chunk_entry.metadata.tags = chunk.section_path.clone();

            self.storage.write_entry(&chunk_entry).await?;

            if self.config.generate_embeddings {
                let embedding = self
                    .embedding_service
                    .embed_single(&self.config.embedding_model, &chunk.text)
                    .await?;

                let point = VectorPoint {
                    schema_version: CURRENT_SCHEMA_VERSION,
                    id: format!("{}-chunk-{}", doc_id, idx),
                    abstract_vector: Some(embedding.vector.clone()),
                    overview_vector: None,
                    visual_vector: None,
                    payload: chunk_entry.metadata.clone(),
                };

                self.vector_storage.upsert_point(&point).await?;
            }
        }

        let tokens_processed = self.chunker.token_counter().count_tokens(&text_content);

        Ok(IngestionResult {
            document_id: doc_id,
            uri,
            chunks_created,
            tokens_processed,
            processing_time_ms: start.elapsed().as_millis() as u64,
            warnings,
        })
    }

    /// 处理文档（非图像）。
    async fn process_document(
        &self,
        request: &IngestionRequest,
        doc_id: &str,
        _uri: &TianyanUri,
        doc_type: &DocumentType,
    ) -> Result<(
        String,
        Vec<super::chunker::DocumentChunk>,
        KnowledgeMetadata,
    )> {
        let parsed = self
            .parser
            .parse_file(&request.content, Path::new(&request.filename))?;

        let chunks = if request.chunk_long_documents {
            self.chunker.chunk_document(&parsed, doc_id)?
        } else {
            vec![]
        };

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
            chunk_count: if chunks.is_empty() {
                None
            } else {
                Some(chunks.len())
            },
            total_tokens: self.chunker.token_counter().count_tokens(&parsed.text),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            importance: 0.5,
        };

        Ok((parsed.text, chunks, metadata))
    }

    /// 处理图像。
    async fn process_image(
        &self,
        request: &IngestionRequest,
        doc_id: &str,
        _uri: &TianyanUri,
    ) -> Result<(
        String,
        Vec<super::chunker::DocumentChunk>,
        KnowledgeMetadata,
    )> {
        let _processed = self
            .image_processor
            .process(&request.content, &request.filename)?;

        let analyzer = ImageAnalyzer::new(&self.vlm_service, &self.vision_encoder, "gpt-4o");
        let analysis = analyzer.analyze(&request.content).await?;

        let unified = analyzer.create_unified_text(&analysis);

        let _visual_embedding = if self.config.generate_embeddings {
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
            chunk_count: None,
            total_tokens: self
                .chunker
                .token_counter()
                .count_tokens(&unified.combined_text),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            importance: 0.5,
        };

        Ok((unified.combined_text, vec![], metadata))
    }

    /// 为内容生成摘要。
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

    /// 为条目存储嵌入向量。
    async fn store_embeddings(&self, uri: &TianyanUri, entry: &ContextEntry) -> Result<()> {
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
            visual_vector: None,
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
        hex::encode(digest.as_ref())
    }

    /// 获取 token 计数器。
    pub fn token_counter(&self) -> &TokenCounter {
        self.chunker.token_counter()
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
        let hash = hex::encode(digest.as_ref());

        assert_eq!(hash.len(), 64);
    }
}

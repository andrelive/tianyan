# Rust本地智能体开发规划 - 代码示例集

> 本文档是 [rust_local_agent_plan.md](./rust_local_agent_plan.md) 的配套代码示例文档，包含方案中所有核心模块的详细代码实现。

## 目录

- [1. 核心数据结构定义](#1-核心数据结构定义)
  - [1.1 内容层级与上下文条目](#11-内容层级与上下文条目)
  - [1.2 检索轨迹结构](#12-检索轨迹结构)
- [2. 统一上下文存储模块](#2-统一上下文存储模块)
  - [2.1 存储配置与URI映射](#21-存储配置与uri映射)
  - [2.2 存储后端实现](#22-存储后端实现)
  - [2.3 Qdrant向量存储集成](#23-qdrant向量存储集成)
  - [2.4 双层检索协调器](#24-双层检索协调器)
- [3. 模型服务接口](#3-模型服务接口)
  - [3.1 核心服务Trait定义](#31-核心服务trait定义)
  - [3.2 VLM分析结果结构](#32-vlm分析结果结构)
- [4. 统一上下文存储实现](#4-统一上下文存储实现)
  - [4.1 URI解析与路由](#41-uri解析与路由)
  - [4.2 虚拟文件系统](#42-虚拟文件系统)
  - [4.3 分层摘要生成](#43-分层摘要生成)
- [5. 双层向量检索使用](#5-双层向量检索使用)
- [6. 记忆自迭代实现](#6-记忆自迭代实现)
- [7. 文件与图片处理实现](#7-文件与图片处理实现)
  - [7.1 文件上传处理器](#71-文件上传处理器)
  - [7.2 图片处理器](#72-图片处理器)
  - [7.3 多模态检索器](#73-多模态检索器)
- [8. 技能系统集成](#8-技能系统集成)

---

## 1. 核心数据结构定义

> **对应方案章节**: [3.2 上下文条目结构](./rust_local_agent_plan.md#32-上下文条目结构)

### 1.1 内容层级与上下文条目

```rust
/// 内容层级枚举 - 对应三层摘要机制
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentLevel {
    Abstract,   // L0层：~100 tokens，用于向量检索和快速过滤
    Overview,   // L1层：~2K tokens，用于内容导航和重排序
    Detail,     // L2层：完整内容，按需加载
}

struct ContextEntry {
    uri: String,                    // 唯一标识符 tianyan://...
    
    // 类型标识
    is_directory: bool,             // 是否为目录
    
    // 三层摘要（Option类型：新条目可能尚未生成摘要）
    abstract_content: Option<String>,  // L0: ~100 tokens
    overview_content: Option<String>,  // L1: ~2K tokens
    detail_content: Option<String>,    // L2: 完整内容
    
    // 元数据
    metadata: EntryMetadata,
    
    // 向量表示（仅内存中临时存储，实际向量持久化在Qdrant中）
    // 这些字段在序列化时会被忽略，仅用于内存中的快速访问
    // 设计说明：向量持久化在Qdrant，内存中临时持有用于减少重复查询开销
    #[serde(skip)]
    abstract_embedding: Vec<f32>,   // L0向量
    #[serde(skip)]
    overview_embedding: Vec<f32>,   // L1向量
    
    // 访问统计
    created_at: DateTime,
    updated_at: DateTime,
    last_accessed: DateTime,
    access_count: usize,
    
    // 重要性评分
    importance: f32,
}

struct EntryMetadata {
    category: String,               // user/memory/knowledge/agent
    sub_category: String,           // profile/sessions/documents等
    tags: Vec<String>,
    source: String,                 // 来源描述
    content_type: ContentType,      // text/markdown/json/code
}

// ContextEntry 的默认实现
impl Default for ContextEntry {
    fn default() -> Self {
        Self {
            uri: String::new(),
            is_directory: false,        // 默认为文件（非目录）
            abstract_content: None,
            overview_content: None,
            detail_content: None,
            metadata: EntryMetadata::default(),
            abstract_embedding: Vec::new(),
            overview_embedding: Vec::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_accessed: chrono::Utc::now(),
            access_count: 0,
            importance: 0.5,             // 默认中等重要性
        }
    }
}

impl Default for EntryMetadata {
    fn default() -> Self {
        Self {
            category: String::new(),
            sub_category: String::new(),
            tags: Vec::new(),
            source: String::new(),
            content_type: ContentType::Text,
        }
    }
}
```

### 1.2 检索轨迹结构

> **对应方案章节**: [3.4 检索轨迹可视化](./rust_local_agent_plan.md#34-检索轨迹可视化)

```rust
struct RetrievalTrace {
    query: String,                  // 原始查询
    steps: Vec<RetrievalStep>,      // 检索步骤
    final_results: Vec<String>,     // 最终结果URI
    total_tokens_used: usize,       // Token消耗
    duration_ms: u64,               // 检索耗时
}

struct RetrievalStep {
    step_type: StepType,            // IntentAnalysis/VectorSearch/OverviewScan等
    target_uri: String,             // 目标URI
    score: f32,                     // 相关性评分
    tokens_consumed: usize,         // 消耗Token
    timestamp: DateTime,
}
```

---

## 2. 统一上下文存储模块

> **对应方案章节**: [4.1.1 虚拟文件系统与真实文件系统映射](./rust_local_agent_plan.md#411-虚拟文件系统与真实文件系统映射)

### 2.1 存储配置与URI映射

```rust
struct StorageConfig {
    root_path: PathBuf,              // 真实文件系统根目录
    max_file_size: usize,            // 单文件最大大小
    compression_enabled: bool,       // 是否启用压缩
}

impl StorageConfig {
    fn default() -> Self {
        Self {
            root_path: dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".tianyan"),
            max_file_size: 10 * 1024 * 1024,  // 10MB
            compression_enabled: true,
        }
    }
    
    fn from_env() -> Self {
        let mut config = Self::default();
        if let Ok(path) = env::var("TIANYAN_DATA_DIR") {
            config.root_path = PathBuf::from(path);
        }
        config
    }
}

struct UriMapper {
    config: StorageConfig,
}

impl UriMapper {
    fn uri_to_path(&self, uri: &Uri) -> PathBuf {
        // tianyan://user/profile/basic_info
        // → ~/.tianyan/user/profile/basic_info.md
        let mut path = self.config.root_path.clone();
        for segment in &uri.path {
            path.push(segment);
        }
        path
    }
    
    fn path_to_uri(&self, path: &Path) -> Result<Uri> {
        // ~/.tianyan/user/profile/basic_info.md
        // → tianyan://user/profile/basic_info
        let relative = path.strip_prefix(&self.config.root_path)?;
        let segments: Vec<String> = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        
        // 移除文件扩展名（如果有）
        if let Some(last) = segments.last() {
            if last.contains('.') {
                let name = last.rsplit_once('.').map(|(n, _)| n).unwrap_or(last);
                let mut segments = segments;
                segments.pop();
                segments.push(name.to_string());
                return Ok(Uri { scheme: "tianyan".to_string(), path: segments });
            }
        }
        
        Ok(Uri { scheme: "tianyan".to_string(), path: segments })
    }
    
    fn get_abstract_path(&self, uri: &Uri) -> PathBuf {
        self.uri_to_path(uri).parent().unwrap().join(".abstract.md")
    }
    
    fn get_overview_path(&self, uri: &Uri) -> PathBuf {
        self.uri_to_path(uri).parent().unwrap().join(".overview.md")
    }
}
```

### 2.2 存储后端实现

```rust
trait StorageBackend: Send + Sync {
    async fn read(&self, path: &Path) -> Result<Vec<u8>>;
    async fn read_content(&self, uri: &Uri) -> Result<String>;  // 读取L2完整内容
    async fn write(&self, path: &Path, data: &[u8]) -> Result<()>;
    async fn delete(&self, path: &Path) -> Result<()>;
    async fn exists(&self, path: &Path) -> bool;
    async fn list_dir(&self, path: &Path) -> Result<Vec<String>>;
}

struct LocalFileStorage {
    config: StorageConfig,
}

impl LocalFileStorage {
    fn ensure_dir(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }
}

impl StorageBackend for LocalFileStorage {
    async fn read(&self, path: &Path) -> Result<Vec<u8>> {
        Ok(fs::read(path)?)
    }
    
    async fn write(&self, path: &Path, data: &[u8]) -> Result<()> {
        self.ensure_dir(path)?;
        fs::write(path, data)?;
        Ok(())
    }
    
    async fn delete(&self, path: &Path) -> Result<()> {
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
        Ok(())
    }
    
    async fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    
    async fn list_dir(&self, path: &Path) -> Result<Vec<String>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            entries.push(entry.file_name().to_string_lossy().to_string());
        }
        Ok(entries)
    }
}
```

### 2.3 Qdrant向量存储集成

```rust
use qdrant_client::prelude::*;

/// Qdrant原始检索结果（不含内容）
/// 用于向量检索的中间结果，仅包含URI和元数据
struct SearchResult {
    uri: String,                    // 条目URI
    score: f32,                     // 相似度分数
    category: Option<String>,       // 分类（user/memory/knowledge/agent）
}

/// 完整检索结果（含加载的内容）
/// 用于最终返回给调用方，包含已加载的内容
struct RetrievalResult {
    uri: String,                    // 条目URI
    score: f32,                     // 相似度分数
    content: String,                // 已加载的内容（根据分数选择L0/L1/L2层级）
}

struct QdrantVectorStore {
    client: QdrantClient,
    collection_name: String,
    embedding_dim: usize,
}

impl QdrantVectorStore {
    async fn initialize(&self) -> Result<()> {
        // 创建Collection，支持多向量（abstract + overview + visual）
        let vectors_config = HashMap::from([
            ("abstract".to_string(), VectorParams {
                size: self.embedding_dim as u64,
                distance: Distance::Cosine as i32,
                ..Default::default()
            }.into()),
            ("overview".to_string(), VectorParams {
                size: self.embedding_dim as u64,
                distance: Distance::Cosine as i32,
                ..Default::default()
            }.into()),
            ("visual".to_string(), VectorParams {
                size: 512,  // CLIP向量维度，用于以图搜图
                distance: Distance::Cosine as i32,
                ..Default::default()
            }.into()),
        ]);
        
        self.client.create_collection(&CreateCollection {
            collection_name: self.collection_name.clone(),
            vectors_config: Some(VectorsConfig {
                config: Some(VectorsConfigDiff::Map(vectors_config)),
            }),
            ..Default::default()
        }).await?;
        
        Ok(())
    }
    
    async fn upsert_context(&self, entry: &ContextEntry) -> Result<()> {
        let point = PointStruct {
            id: Some(entry.uri.clone().into()),
            vectors: Some(Vectors::from(HashMap::from([
                ("abstract".to_string(), entry.abstract_embedding.clone()),
                ("overview".to_string(), entry.overview_embedding.clone()),
            ]))),
            payload: HashMap::from([
                ("uri", entry.uri.clone().into()),
                ("category", entry.metadata.category.clone().into()),
                ("sub_category", entry.metadata.sub_category.clone().into()),
                ("type", if entry.is_directory { "directory" } else { "file" }.into()),
                ("importance", entry.importance.into()),
                ("tags", entry.metadata.tags.clone().into()),
            ]),
        };
        
        self.client.upsert_points(&UpsertPoints {
            collection_name: self.collection_name.clone(),
            points: vec![point],
            ..Default::default()
        }).await?;
        
        Ok(())
    }
    
    async fn search_by_abstract(&self, query_vector: Vec<f32>, top_k: usize) -> Result<Vec<SearchResult>> {
        let results = self.client.search_points(&SearchPoints {
            collection_name: self.collection_name.clone(),
            vector: query_vector,
            vector_name: Some("abstract".to_string()),
            limit: top_k as u64,
            with_payload: Some(true.into()),
            ..Default::default()
        }).await?;
        
        Ok(results.result.into_iter().map(|r| SearchResult {
            uri: r.payload.get("uri").unwrap().to_string(),
            score: r.score,
            category: r.payload.get("category").map(|v| v.to_string()),
        }).collect())
    }
    
    async fn search_by_overview(&self, query_vector: Vec<f32>, uris: Vec<String>) -> Result<Vec<SearchResult>> {
        // 在指定URI范围内进行L1检索
        let results = self.client.search_points(&SearchPoints {
            collection_name: self.collection_name.clone(),
            vector: query_vector,
            vector_name: Some("overview".to_string()),
            limit: uris.len() as u64,
            filter: Some(Filter {
                should: uris.iter().map(|uri| Condition {
                    field: Some(FieldCondition {
                        key: "uri".to_string(),
                        r#match: Some(Match::Value(Value::String(uri.clone()))),
                        ..Default::default()
                    }.into()),
                    ..Default::default()
                }).collect(),
                ..Default::default()
            }),
            with_payload: Some(true.into()),
            ..Default::default()
        }).await?;
        
        Ok(results.result.into_iter().map(|r| SearchResult {
            uri: r.payload.get("uri").unwrap().to_string(),
            score: r.score,
            category: r.payload.get("category").map(|v| v.to_string()),
        }).collect())
    }
    
    async fn delete_context(&self, uri: &str) -> Result<()> {
        self.client.delete_points(&DeletePoints {
            collection_name: self.collection_name.clone(),
            points: vec![uri.into()],
            ..Default::default()
        }).await?;
        
        Ok(())
    }
    
    async fn search_by_visual(&self, query_vector: Vec<f32>, top_k: usize) -> Result<Vec<SearchResult>> {
        // 在视觉向量空间中检索（以图搜图）
        let results = self.client.search_points(&SearchPoints {
            collection_name: self.collection_name.clone(),
            vector: query_vector,
            vector_name: Some("visual".to_string()),
            limit: top_k as u64,
            with_payload: Some(true.into()),
            ..Default::default()
        }).await?;
        
        Ok(results.result.into_iter().map(|r| SearchResult {
            uri: r.payload.get("uri").unwrap().to_string(),
            score: r.score,
            category: r.payload.get("category").map(|v| v.to_string()),
        }).collect())
    }
    
    async fn search_by_visual_with_filter(
        &self, 
        query_vector: Vec<f32>, 
        filter: Option<Filter>, 
        top_k: usize
    ) -> Result<Vec<SearchResult>> {
        // 在视觉向量空间中检索，支持URI前缀过滤（以图搜图）
        let results = self.client.search_points(&SearchPoints {
            collection_name: self.collection_name.clone(),
            vector: query_vector,
            vector_name: Some("visual".to_string()),
            limit: top_k as u64,
            filter,
            with_payload: Some(true.into()),
            ..Default::default()
        }).await?;
        
        Ok(results.result.into_iter().map(|r| SearchResult {
            uri: r.payload.get("uri").unwrap().to_string(),
            score: r.score,
            category: r.payload.get("category").map(|v| v.to_string()),
        }).collect())
    }
    
    async fn upsert_visual_embedding(&self, uri: &Uri, visual_embedding: &[f32]) -> Result<()> {
        // 更新条目的视觉向量
        self.client.update_vectors(&UpdatePointVectors {
            collection_name: self.collection_name.clone(),
            points: vec![PointVectors {
                id: Some(uri.to_string().into()),
                vectors: Some(Vectors::from(HashMap::from([
                    ("visual".to_string(), visual_embedding.to_vec()),
                ]))),
            }],
            ..Default::default()
        }).await?;
        
        Ok(())
    }
}
```

### 2.4 双层检索协调器

```rust
/// 内容加载器trait，用于解耦DualLayerRetriever与VirtualFileSystem
/// 避免循环依赖：DualLayerRetriever不直接持有VirtualFileSystem
trait ContentLoader: Send + Sync {
    async fn load_content(&self, uri: &Uri, level: ContentLevel) -> Result<String>;
}

struct DualLayerRetriever {
    vector_store: Arc<QdrantVectorStore>,
    embedding_service: Arc<dyn EmbeddingService>,
    content_loader: Arc<dyn ContentLoader>,  // 通过trait解耦，避免循环依赖
}

impl DualLayerRetriever {
    async fn retrieve(&self, query: &str, top_k: usize) -> Result<Vec<RetrievalResult>> {
        // Step 1: 查询向量化
        let query_vector = self.embedding_service.embed(query).await?;
        
        // Step 2: L0检索 - 快速过滤（返回较多候选）
        let candidates = self.vector_store.search_by_abstract(
            query_vector.clone(),
            top_k * 3,  // 返回3倍候选
        ).await?;
        
        // Step 3: L1检索 - 精确重排序
        let candidate_uris: Vec<String> = candidates.iter().map(|c| c.uri.clone()).collect();
        let refined = self.vector_store.search_by_overview(
            query_vector,
            candidate_uris,
        ).await?;
        
        // Step 4: 加载内容（根据相关性分数选择加载层级）
        let mut results = Vec::new();
        for result in refined.into_iter().take(top_k) {
            let content = if result.score > 0.85 {
                // 高相关性：加载完整内容
                self.content_loader.load_content(&Uri::parse(&result.uri)?, ContentLevel::Detail).await?
            } else if result.score > 0.6 {
                // 中相关性：加载概览
                self.content_loader.load_content(&Uri::parse(&result.uri)?, ContentLevel::Overview).await?
            } else {
                // 低相关性：仅返回摘要
                self.content_loader.load_content(&Uri::parse(&result.uri)?, ContentLevel::Abstract).await?
            };
            
            results.push(RetrievalResult {
                uri: result.uri,
                score: result.score,
                content,
            });
        }
        
        Ok(results)
    }
    
    /// 公开的内容加载方法，供 MultiModalRetriever 等外部调用者使用
    async fn load_content(&self, uri: &str) -> Result<String> {
        // 默认加载概览级别内容
        self.content_loader.load_content(&Uri::parse(uri)?, ContentLevel::Overview).await
    }
}
```

---

## 3. 模型服务接口

> **对应方案章节**: [5.3 模型服务选型](./rust_local_agent_plan.md#53-模型服务选型)

### 3.1 核心服务Trait定义

```rust
/// 大模型服务 trait - 核心推理接口
/// 用于摘要生成、意图分析、记忆提取等核心功能
trait ModelService: Send + Sync {
    /// 完成文本生成
    async fn complete(&self, prompt: String) -> Result<String>;
    
    /// 带系统提示的完成
    async fn complete_with_system(&self, system: &str, prompt: &str) -> Result<String>;
}

/// 文本嵌入服务 trait - 用于文本向量化
/// 用于L0/L1摘要向量化，支持双层向量检索
trait EmbeddingService: Send + Sync {
    /// 将文本转换为向量
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

/// 视觉语言模型服务 trait - 用于图片理解
/// 实现可对接 GPT-4 Vision、Claude 3 Vision、Qwen-VL 等
trait VlmService: Send + Sync {
    /// 分析图片，返回描述、元素、OCR文本等
    async fn analyze_image(&self, image: &[u8]) -> Result<VlmResult>;
}

/// 视觉编码器 trait - 用于图片向量化（以图搜图）
/// 实现可对接 CLIP ViT 等视觉模型
trait VisionEncoder: Send + Sync {
    /// 将图片编码为视觉向量
    /// 返回512维CLIP向量，用于以图搜图功能
    async fn encode(&self, image: &[u8]) -> Result<Vec<f32>>;
}
```

### 3.2 VLM分析结果结构

```rust
/// VLM分析结果
struct VlmResult {
    description: String,              // 图片整体描述
    elements: Vec<DetectedElement>,   // 识别的元素列表
    ocr_text: Option<String>,         // OCR提取的文本
    tags: Vec<String>,                // 标签
}

/// 检测到的元素
struct DetectedElement {
    label: String,        // 元素标签
    confidence: f32,      // 置信度 (0.0-1.0)
}
```

---

## 4. 统一上下文存储实现

> **对应方案章节**: [7.1 统一上下文存储实现](./rust_local_agent_plan.md#71-统一上下文存储实现)

### 4.1 URI解析与路由

```rust
#[derive(Debug, Clone)]
struct Uri {
    scheme: String,         // "tianyan"
    path: Vec<String>,      // ["user", "profile", "basic_info"]
}

impl Uri {
    fn parse(uri_str: &str) -> Result<Self> {
        // tianyan://user/profile/basic_info
        let parts: Vec<&str> = uri_str.strip_prefix("tianyan://")
            .ok_or(Error::InvalidUri)?
            .split('/')
            .collect();
        Ok(Self {
            scheme: "tianyan".to_string(),
            path: parts.iter().map(|s| s.to_string()).collect(),
        })
    }
    
    fn parent(&self) -> Option<Self> {
        if self.path.len() > 1 {
            let mut parent = self.clone();
            parent.path.pop();
            Some(parent)
        } else {
            None
        }
    }
}
```

### 4.2 虚拟文件系统

```rust
struct VirtualFileSystem {
    root: Arc<RwLock<Directory>>,
    storage: Box<dyn StorageBackend>,
    vector_store: Arc<QdrantVectorStore>,  // Qdrant向量存储
}

// VirtualFileSystem 实现 ContentLoader trait，供 DualLayerRetriever 使用
impl ContentLoader for VirtualFileSystem {
    async fn load_content(&self, uri: &Uri, level: ContentLevel) -> Result<String> {
        self.read(uri, level).await
    }
}

struct Directory {
    uri: Uri,
    entries: HashMap<String, Entry>,
    subdirectories: HashMap<String, Arc<RwLock<Directory>>>,
    abstract_content: Option<String>,
    overview_content: Option<String>,
}

enum Entry {
    File(ContextEntry),
    Directory(Directory),
}

impl VirtualFileSystem {
    async fn read(&self, uri: &Uri, level: ContentLevel) -> Result<String> {
        let dir = self.navigate_to(uri).await?;
        match level {
            ContentLevel::Abstract => dir.abstract_content.clone()
                .ok_or(Error::ContentNotFound("abstract content not available".into())),
            ContentLevel::Overview => dir.overview_content.clone()
                .ok_or(Error::ContentNotFound("overview content not available".into())),
            ContentLevel::Detail => self.storage.read_content(uri).await,
        }
    }
    
    async fn ls(&self, uri: &Uri) -> Result<Vec<String>> {
        let dir = self.navigate_to(uri).await?;
        Ok(dir.entries.keys().cloned().collect())
    }
    
    async fn find_by_visual_embedding(
        &self,
        query_embedding: &[f32],
        scope: Option<&Uri>,
        top_k: usize,
    ) -> Result<Vec<SearchResult>> {
        // 使用Qdrant进行视觉向量检索（以图搜图）
        // 如果指定了scope，则构建URI前缀过滤器
        let filter = scope.map(|uri| {
            // 构建URI前缀过滤条件，限制搜索范围
            Filter {
                should: vec![Condition {
                    field: Some(FieldCondition {
                        key: "uri".to_string(),
                        r#match: Some(Match::Value(Value::String(format!("{}*", uri.to_string())))),
                        ..Default::default()
                    }.into()),
                    ..Default::default()
                }],
                ..Default::default()
            }
        });
        self.vector_store.search_by_visual_with_filter(query_embedding.to_vec(), filter, top_k).await
    }
}
```

### 4.3 分层摘要生成

```rust
struct SummaryEngine {
    model_service: Arc<dyn ModelService>,
    tokenizer: Arc<dyn Tokenizer>,
}

impl SummaryEngine {
    async fn generate_abstract(&self, content: &str) -> Result<String> {
        let prompt = format!(
            "请为以下内容生成一个简洁的摘要，限制在100个token以内：\n\n{}",
            content
        );
        self.model_service.complete(prompt).await
    }
    
    async fn generate_overview(&self, content: &str) -> Result<String> {
        let prompt = format!(
            "请为以下内容生成一个详细的概览，限制在2000个token以内，包含主要结构和关键点：\n\n{}",
            content
        );
        self.model_service.complete(prompt).await
    }
    
    async fn update_parent_summaries(&self, fs: &VirtualFileSystem, uri: &Uri) -> Result<()> {
        let mut current = uri.clone();
        while let Some(parent) = current.parent() {
            let children = fs.ls(&current).await?;
            let combined = self.combine_children_summaries(&children).await?;
            
            let abstract_content = self.generate_abstract(&combined).await?;
            let overview_content = self.generate_overview(&combined).await?;
            
            fs.update_summary(&parent, abstract_content, overview_content).await?;
            current = parent;
        }
        Ok(())
    }
}
```

---

## 5. 双层向量检索使用

> **对应方案章节**: [7.2 双层向量检索使用](./rust_local_agent_plan.md#72-双层向量检索使用)

```rust
// 使用示例：创建检索器并执行双层检索
async fn search_knowledge(
    fs: &Arc<VirtualFileSystem>,
    vector_store: &Arc<QdrantVectorStore>,
    embedding_service: &Arc<dyn EmbeddingService>,
    query: &str
) -> Result<Vec<RetrievalResult>> {
    // 创建DualLayerRetriever，传入ContentLoader实现
    let retriever = DualLayerRetriever {
        vector_store: vector_store.clone(),
        embedding_service: embedding_service.clone(),
        content_loader: fs.clone() as Arc<dyn ContentLoader>,  // VirtualFileSystem实现了ContentLoader
    };
    
    // 执行双层检索：L0检索 → L1重排序 → 内容加载
    retriever.retrieve(query, 10).await
}

// 使用 MultiModalRetriever 作为统一检索入口（推荐方式）
async fn search_via_multimodal(
    multimodal_retriever: &MultiModalRetriever,
    query: &str
) -> Result<Vec<RetrievalResult>> {
    // 通过门面层统一调用，支持文本和图片查询
    multimodal_retriever.search(QueryType::Text(query.to_string()), 10).await
}
```

---

## 6. 记忆自迭代实现

> **对应方案章节**: [7.3 记忆自迭代实现](./rust_local_agent_plan.md#73-记忆自迭代实现)

```rust
struct MemoryIterator {
    fs: Arc<VirtualFileSystem>,
    model_service: Arc<dyn ModelService>,
    summary_engine: Arc<SummaryEngine>,
}

impl MemoryIterator {
    async fn process_session(&self, session_uri: &Uri) -> Result<()> {
        let session_content = self.fs.read(session_uri, ContentLevel::Detail).await?;
        
        // 使用大模型分析会话内容
        let analysis = self.analyze_session(&session_content).await?;
        
        // 提取并存储各类记忆
        for preference in analysis.preferences {
            self.store_preference(&preference).await?;
        }
        
        for event in analysis.events {
            self.store_event(&event).await?;
        }
        
        for case in analysis.cases {
            self.store_case(&case).await?;
        }
        
        for pattern in analysis.patterns {
            self.store_pattern(&pattern).await?;
        }
        
        // 更新会话摘要
        self.summary_engine.update_parent_summaries(&self.fs, session_uri).await?;
        
        Ok(())
    }
    
    async fn analyze_session(&self, content: &str) -> Result<SessionAnalysis> {
        let prompt = format!(
            r#"分析以下会话内容，提取需要存储的信息：

会话内容：
{}

请以JSON格式返回：
{{
    "preferences": [{{"key": "偏好类型", "value": "偏好值", "confidence": 0.0-1.0}}],
    "events": [{{"type": "事件类型", "description": "事件描述", "importance": 0.0-1.0}}],
    "cases": [{{"type": "success/failed", "task": "任务描述", "outcome": "结果", "lessons": "教训"}}],
    "patterns": [{{"pattern": "模式描述", "applicability": "适用场景"}}]
}}"#,
            content
        );
        
        let response = self.model_service.complete(prompt).await?;
        serde_json::from_str(&response)
    }
    
    async fn store_preference(&self, preference: &Preference) -> Result<()> {
        let uri = Uri::parse(&format!(
            "tianyan://user/preferences/{}",
            preference.key
        ))?;
        
        let entry = ContextEntry {
            uri: uri.to_string(),
            abstract_content: format!("{}: {}", preference.key, preference.value),
            overview_content: preference.value.clone(),
            detail_content: preference.value.clone(),
            metadata: EntryMetadata {
                category: "user".to_string(),
                sub_category: "preferences".to_string(),
                tags: vec![preference.key.clone()],
                source: "memory_iteration".to_string(),
                content_type: ContentType::Text,
            },
            importance: preference.confidence,
            ..Default::default()
        };
        
        self.fs.write(&uri, entry).await
    }
}
```

---

## 7. 文件与图片处理实现

> **对应方案章节**: [7.4 文件与图片处理实现](./rust_local_agent_plan.md#74-文件与图片处理实现)

### 7.1 文件上传处理器

```rust
struct FileUploadHandler {
    fs: Arc<VirtualFileSystem>,
    summary_engine: Arc<SummaryEngine>,
    embedding_service: Arc<dyn EmbeddingService>,
    extractors: HashMap<FileType, Box<dyn ContentExtractor>>,
}

#[derive(Debug, Clone)]
enum FileType {
    Document(DocumentFormat),
    Code(CodeLanguage),
    Data(DataFormat),
    Image(ImageFormat),
}

impl FileUploadHandler {
    async fn process_upload(&self, file: UploadedFile) -> Result<Uri> {
        // Step 1: 识别文件类型
        let file_type = self.detect_file_type(&file)?;
        
        // Step 2: 提取内容
        let extracted = self.extract_content(&file, &file_type).await?;
        
        // Step 3: 生成分层摘要
        let abstract_content = self.summary_engine.generate_abstract(&extracted.text).await?;
        let overview_content = self.summary_engine.generate_overview(&extracted.text).await?;
        
        // Step 4: 向量化（L0和L1都需要向量化）
        let abstract_embedding = self.embedding_service.embed(&abstract_content).await?;
        let overview_embedding = self.embedding_service.embed(&overview_content).await?;
        
        // Step 5: 分配URI并存储
        let uri = self.generate_uri(&file_type, &file.name)?;
        
        let entry = ContextEntry {
            uri: uri.to_string(),
            abstract_content,
            overview_content,
            detail_content: extracted.text.clone(),
            abstract_embedding,
            overview_embedding,
            metadata: EntryMetadata {
                category: "knowledge".to_string(),
                sub_category: "documents".to_string(),
                tags: extracted.tags,
                source: file.name.clone(),
                content_type: ContentType::Markdown,
            },
            ..Default::default()
        };
        
        self.fs.write(&uri, entry).await?;
        
        // Step 6: 如果是超长文档，进行智能结构化分片
        if self.is_long_document(&extracted.text).await? {
            self.intelligent_chunk_and_store(&uri, &extracted.text).await?;
        }
        
        Ok(uri)
    }
    
    async fn is_long_document(&self, text: &str) -> Result<bool> {
        // 判断是否需要智能分片
        // 阈值说明：
        // - 8000 tokens ≈ 12000 中文字 ≈ 24000 英文字
        // - 典型长文档场景：技术书籍、大型报告、完整API文档、论文
        // - 普通文档（无需分片）：单篇文章、配置文件、代码片段
        let token_count = self.summary_engine.tokenizer.count_tokens(text);
        Ok(token_count > 8000)  // 超过8000 tokens视为长文档，需智能分片
    }
    
    async fn intelligent_chunk_and_store(&self, parent_uri: &Uri, text: &str) -> Result<()> {
        // 使用大模型分析文档结构，智能确定分片边界
        let structure = self.analyze_document_structure(text).await?;
        
        // 按章节/段落边界分片存储
        for (idx, chunk) in structure.chunks.iter().enumerate() {
            let chunk_uri = parent_uri.child(&format!("chunk_{:03}", idx));
            
            let abstract_content = self.summary_engine.generate_abstract(&chunk.content).await?;
            let overview_content = self.summary_engine.generate_overview(&chunk.content).await?;
            let abstract_embedding = self.embedding_service.embed(&abstract_content).await?;
            let overview_embedding = self.embedding_service.embed(&overview_content).await?;
            
            let entry = ContextEntry {
                uri: chunk_uri.to_string(),
                abstract_content,
                overview_content,
                detail_content: chunk.content.clone(),
                abstract_embedding,
                overview_embedding,
                metadata: EntryMetadata {
                    category: "knowledge".to_string(),
                    sub_category: "chunks".to_string(),
                    tags: chunk.tags.clone(),
                    source: format!("chunk_{}", idx),
                    content_type: ContentType::Markdown,
                },
                ..Default::default()
            };
            
            self.fs.write(&chunk_uri, entry).await?;
        }
        
        Ok(())
    }
    
    async fn analyze_document_structure(&self, text: &str) -> Result<DocumentStructure> {
        let prompt = format!(
            r#"分析以下文档的结构，确定最佳分片边界：

文档内容：
{}

请以JSON格式返回：
{{
    "chunks": [
        {{
            "title": "章节标题",
            "content": "章节内容",
            "tags": ["标签1", "标签2"],
            "start_line": 起始行号,
            "end_line": 结束行号
        }}
    ],
    "outline": "文档大纲"
}}

分片原则：
1. 按章节、段落、主题边界分片，保持语义完整
2. 每个分片应独立可理解
3. 避免在代码块、表格中间分片"#,
            text
        );
        
        let response = self.model_service.complete(prompt).await?;
        serde_json::from_str(&response)
    }
    
    async fn extract_content(&self, file: &UploadedFile, file_type: &FileType) -> Result<ExtractedContent> {
        match file_type {
            FileType::Document(format) => {
                let extractor = self.extractors.get(&FileType::Document(format.clone())).unwrap();
                extractor.extract(&file.data).await
            }
            FileType::Code(lang) => {
                self.extract_code(&file.data, lang).await
            }
            FileType::Data(format) => {
                self.extract_data(&file.data, format).await
            }
            FileType::Image(format) => {
                // 图片由专门的ImageProcessor处理
                Err(Error::WrongHandler)
            }
        }
    }
}

// PDF提取器示例
struct PdfExtractor;

impl ContentExtractor for PdfExtractor {
    async fn extract(&self, data: &[u8]) -> Result<ExtractedContent> {
        let text = pdf_extract::extract_text_from_mem(data)?;
        Ok(ExtractedContent {
            text,
            tags: vec!["pdf".to_string(), "document".to_string()],
            metadata: json!({}),
        })
    }
}
```

### 7.2 图片处理器

```rust
struct ImageProcessor {
    fs: Arc<VirtualFileSystem>,
    vlm_service: Arc<dyn VlmService>,
    embedding_service: Arc<dyn EmbeddingService>,
    summary_engine: Arc<SummaryEngine>,
    vision_encoder: Arc<dyn VisionEncoder>,  // 视觉编码器，用于以图搜图
    vector_store: Arc<QdrantVectorStore>,    // Qdrant向量存储
}

struct ProcessedImage {
    unified_text: String,          // 统一文本（VLM描述+OCR+标签）
    visual_embedding: Vec<f32>,    // 视觉向量（用于以图搜图）
    text_embedding: Vec<f32>,      // 统一文本向量
}

impl ImageProcessor {
    async fn process_image(&self, image: &UploadedImage) -> Result<Uri> {
        // Step 1: 图片预处理
        let processed = self.preprocess_image(&image.data).await?;
        
        // Step 2: VLM理解（包含OCR）
        let vlm_result = self.vlm_service.analyze_image(&processed).await?;
        
        // Step 3: 构建统一文本
        let unified_text = self.build_unified_text(&vlm_result);
        
        // Step 4: 生成分层摘要
        let abstract_content = self.generate_image_abstract(&unified_text).await?;
        let overview_content = self.generate_image_overview(&vlm_result).await?;
        
        // Step 5: 生成向量
        let abstract_embedding = self.embedding_service.embed(&abstract_content).await?;
        let overview_embedding = self.embedding_service.embed(&overview_content).await?;
        
        // Step 6: 存储到统一上下文系统
        let uri = self.generate_image_uri(&image)?;
        
        // 存储图片文件
        let image_uri = uri.child("original.png");
        self.fs.write_binary(&image_uri, &processed).await?;
        
        // 存储缩略图
        let thumbnail = self.create_thumbnail(&processed)?;
        let thumb_uri = uri.child("thumbnail.jpg");
        self.fs.write_binary(&thumb_uri, &thumbnail).await?;
        
        // 存储统一文本
        let text_uri = uri.child("unified_text.md");
        self.fs.write(&text_uri, &unified_text).await?;
        
        // 存储上下文条目
        let entry = ContextEntry {
            uri: uri.to_string(),
            is_directory: false,
            abstract_content: abstract_content.clone(),
            overview_content: overview_content.clone(),
            detail_content: unified_text.clone(),
            abstract_embedding: abstract_embedding.clone(),
            overview_embedding: overview_embedding.clone(),
            metadata: EntryMetadata {
                category: "knowledge".to_string(),
                sub_category: "images".to_string(),
                tags: vlm_result.tags.clone(),
                source: image.name.clone(),
                content_type: ContentType::Image,
            },
            importance: 1.0,
            ..Default::default()
        };
        
        self.fs.write(&uri, entry).await?;
        
        // 存储向量到Qdrant
        self.vector_store.upsert_context(&entry).await?;
        
        // 生成并存储视觉向量（用于以图搜图）
        let visual_embedding = self.vision_encoder.encode(&processed).await?;
        self.vector_store.upsert_visual_embedding(&uri, &visual_embedding).await?;
        
        Ok(uri)
    }
    
    fn build_unified_text(&self, result: &VlmResult) -> String {
        let mut text = result.description.clone();
        
        // 添加识别的元素标签
        if !result.elements.is_empty() {
            text.push_str("\n包含元素：");
            for elem in &result.elements {
                text.push_str(&format!("{}、", elem.label));
            }
            text.pop(); // 移除最后的顿号
        }
        
        // 添加OCR提取的文本
        if let Some(ocr) = &result.ocr_text {
            if !ocr.is_empty() {
                text.push_str(&format!("\n图中文字：{}", ocr));
            }
        }
        
        text
    }
    
    async fn generate_image_abstract(&self, unified_text: &str) -> Result<String> {
        let prompt = format!(
            "请用一句话（不超过50字）概括这张图片：\n{}",
            unified_text
        );
        self.summary_engine.model_service.complete(prompt).await
    }
    
    async fn generate_image_overview(&self, result: &VlmResult) -> Result<String> {
        let mut overview = format!("## 图片描述\n{}\n\n", result.description);
        
        if !result.elements.is_empty() {
            overview.push_str("## 识别元素\n");
            for elem in &result.elements {
                overview.push_str(&format!("- {} (置信度: {:.1}%)\n", elem.label, elem.confidence * 100.0));
            }
            overview.push_str("\n");
        }
        
        if let Some(text) = &result.ocr_text {
            if !text.is_empty() {
                overview.push_str(&format!("## 图中文字\n```\n{}\n```\n", text));
            }
        }
        
        Ok(overview)
    }
}
```

### 7.3 多模态检索器

```rust
/// 多模态检索器 - 作为检索门面层
/// 
/// 职责：
/// - 统一检索入口，对外提供简洁的 search() 接口
/// - 多模态查询路由（文本查询 vs 图片查询）
/// - 组合 DualLayerRetriever（文本检索）和 VirtualFileSystem（视觉检索）
struct MultiModalRetriever {
    text_retriever: Arc<DualLayerRetriever>,  // 文本检索器（L0→L1双层检索）
    fs: Arc<VirtualFileSystem>,                // 用于视觉检索和内容加载
    vision_encoder: Arc<dyn VisionEncoder>,    // 视觉编码器，用于以图搜图
}

enum QueryType {
    Text(String),
    Image(Vec<u8>),
}

impl MultiModalRetriever {
    async fn search(&self, query: QueryType, top_k: usize) -> Result<Vec<RetrievalResult>> {
        match query {
            QueryType::Text(text) => {
                // 文本查询：通过 DualLayerRetriever 进行双层检索
                // 自动完成 L0检索 → L1重排序 → 内容加载
                self.search_by_text(&text, top_k).await
            }
            QueryType::Image(image) => {
                // 图片查询：以图搜图
                self.search_by_image(&image, top_k).await
            }
        }
    }
    
    async fn search_by_text(&self, text: &str, top_k: usize) -> Result<Vec<RetrievalResult>> {
        // 直接调用 DualLayerRetriever 进行双层检索
        // 文档和图片都通过统一文本向量检索，无需区分
        self.text_retriever.retrieve(text, top_k).await
    }
    
    async fn search_by_image(&self, image: &[u8], top_k: usize) -> Result<Vec<RetrievalResult>> {
        // 生成查询图片的视觉向量
        let query_embedding = self.vision_encoder.encode(image).await?;
        
        // 在图片目录中搜索相似图片（通过视觉向量）
        let search_results = self.fs.find_by_visual_embedding(
            &query_embedding,
            Some(&Uri::parse("tianyan://knowledge/images")?),
            top_k,
        ).await?;
        
        // 将 SearchResult 转换为 RetrievalResult（加载内容）
        let mut results = Vec::new();
        for result in search_results {
            let content = self.text_retriever.load_content(&result.uri).await?;
            results.push(RetrievalResult {
                uri: result.uri,
                score: result.score,
                content,
            });
        }
        
        Ok(results)
    }
}
```

---

## 8. 技能系统集成

> **对应方案章节**: [7.5 技能系统集成](./rust_local_agent_plan.md#75-技能系统集成)

```rust
struct SkillManager {
    fs: Arc<VirtualFileSystem>,
    retriever: Arc<DualLayerRetriever>,  // 用于技能检索
    executors: HashMap<String, Box<dyn SkillExecutor>>,
}

impl SkillManager {
    async fn register_skill(&self, skill: Skill) -> Result<()> {
        let uri = Uri::parse(&format!(
            "tianyan://agent/skills/{}/definition",
            skill.name
        ))?;
        
        // 存储技能定义
        let definition = serde_json::to_string_pretty(&skill)?;
        let entry = ContextEntry {
            uri: uri.to_string(),
            abstract_content: skill.description.clone(),
            overview_content: format!(
                "技能: {}\n描述: {}\n参数: {}",
                skill.name,
                skill.description,
                serde_json::to_string(&skill.parameters)?
            ),
            detail_content: definition,
            metadata: EntryMetadata {
                category: "agent".to_string(),
                sub_category: "skills".to_string(),
                tags: skill.tags.clone(),
                source: "skill_registration".to_string(),
                content_type: ContentType::Json,
            },
            importance: 1.0,
            ..Default::default()
        };
        
        self.fs.write(&uri, entry).await?;
        
        // 注册执行器
        self.executors.insert(skill.name.clone(), skill.executor);
        
        Ok(())
    }
    
    async fn find_relevant_skills(&self, query: &str, top_k: usize) -> Result<Vec<SkillMatch>> {
        // 通过 DualLayerRetriever 进行双层检索
        let results = self.retriever.retrieve(query, top_k).await?;
        
        let mut matches = Vec::new();
        for result in results {
            // 从URI中提取技能名称
            if let Some(skill_name) = result.uri.split('/').nth(4) {
                if let Some(executor) = self.executors.get(skill_name) {
                    matches.push(SkillMatch {
                        name: skill_name.to_string(),
                        score: result.score,
                        definition: result.content,
                        executor: executor.as_ref(),
                    });
                }
            }
        }
        
        Ok(matches)
    }
}
```

---

## 附录：检索架构职责划分

```
┌─────────────────────────────────────────────────────────────────┐
│  第1层：存储层                                                   │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  VirtualFileSystem                                         │  │
│  │  - 职责：内容存储、URI映射、内容加载                        │  │
│  │  - 实现 ContentLoader trait，供 DualLayerRetriever 调用    │  │
│  │  - 提供 find_by_visual_embedding() 用于视觉检索            │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              ▲
                              │ ContentLoader trait
                              │
┌─────────────────────────────────────────────────────────────────┐
│  第2层：检索层                                                   │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  DualLayerRetriever                                        │  │
│  │  - 职责：L0/L1双层向量检索、内容加载协调                    │  │
│  │  - 依赖：ContentLoader trait（解耦，不直接依赖VFS）         │  │
│  │  - 提供 retrieve() 和 load_content() 方法                  │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              ▲
                              │ 组合
                              │
┌─────────────────────────────────────────────────────────────────┐
│  第3层：门面层                                                   │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  MultiModalRetriever                                       │  │
│  │  - 职责：统一检索入口、多模态查询路由                        │  │
│  │  - 组合：DualLayerRetriever（文本）+ VFS（视觉）            │  │
│  │  - 对外提供简洁的 search() 接口                             │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

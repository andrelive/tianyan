//! 文档分块的数据类型和配置。

use std::collections::HashMap;

/// 默认分块大小（token 数）。
pub const DEFAULT_CHUNK_SIZE: usize = 1000;

/// 默认分块重叠（token 数）。
pub const DEFAULT_CHUNK_OVERLAP: usize = 100;

/// 文档分块。
#[derive(Debug, Clone)]
pub struct DocumentChunk {
    /// 分块 ID。
    pub id: String,
    /// 文档中的分块索引。
    pub index: usize,
    /// 分块文本内容。
    pub text: String,
    /// 此分块的 token 数量。
    pub token_count: usize,
    /// 原始文档中的起始位置。
    pub start_position: usize,
    /// 原始文档中的结束位置。
    pub end_position: usize,
    /// 章节/章节信息（如果适用）。
    pub section: Option<String>,
    /// 父章节路径。
    pub section_path: Vec<String>,
    /// 分块元数据。
    pub metadata: ChunkMetadata,
}

/// 文档分块的元数据。
#[derive(Debug, Clone, Default)]
pub struct ChunkMetadata {
    /// 是否为章节的第一个分块。
    pub is_section_start: bool,
    /// 是否为章节的最后一个分块。
    pub is_section_end: bool,
    /// 自定义元数据字段。
    pub custom: HashMap<String, String>,
}

/// 文档分块配置。
#[derive(Debug, Clone)]
pub struct ChunkingConfig {
    /// 最大分块大小（token 数）。
    pub chunk_size: usize,
    /// 分块间的重叠（token 数）。
    pub chunk_overlap: usize,
    /// 是否尊重句子边界。
    pub respect_sentence_boundaries: bool,
    /// 是否尊重段落边界。
    pub respect_paragraph_boundaries: bool,
    /// 是否使用语义分块（基于目录）。
    pub use_semantic_chunking: bool,
    /// 最小分块大小（token 数）。
    pub min_chunk_size: usize,
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            chunk_overlap: DEFAULT_CHUNK_OVERLAP,
            respect_sentence_boundaries: true,
            respect_paragraph_boundaries: true,
            use_semantic_chunking: true,
            min_chunk_size: 100,
        }
    }
}

impl ChunkingConfig {
    /// 创建新的分块配置。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置分块大小。
    pub fn with_chunk_size(mut self, size: usize) -> Self {
        self.chunk_size = size;
        self
    }

    /// 设置分块重叠。
    pub fn with_overlap(mut self, overlap: usize) -> Self {
        self.chunk_overlap = overlap;
        self
    }

    /// 启用或禁用语义分块。
    pub fn with_semantic_chunking(mut self, enabled: bool) -> Self {
        self.use_semantic_chunking = enabled;
        self
    }
}

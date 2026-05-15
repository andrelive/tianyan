//! 智能文档分割的文档分块模块。
//!
//! 本模块提供智能文档分块功能，保留语义边界并维护分块间的上下文。

pub mod types;
pub use types::*;

use std::collections::HashMap;

use crate::common::error::Result;
use crate::storage::TokenCounter;

use super::parser::ParsedDocument;

/// 文档分块器，用于将文档分割成可管理的片段。
pub struct DocumentChunker {
    config: ChunkingConfig,
    token_counter: TokenCounter,
}

impl DocumentChunker {
    /// 创建新的文档分块器。
    pub fn new(config: ChunkingConfig) -> Result<Self> {
        let token_counter = TokenCounter::new()?;
        Ok(Self {
            config,
            token_counter,
        })
    }

    /// 使用默认配置创建分块器。
    pub fn with_defaults() -> Result<Self> {
        Self::new(ChunkingConfig::default())
    }

    /// 获取 token 计数器。
    pub fn token_counter(&self) -> &TokenCounter {
        &self.token_counter
    }

    /// 对解析后的文档进行分块。
    pub fn chunk_document(&self, doc: &ParsedDocument, doc_id: &str) -> Result<Vec<DocumentChunk>> {
        let total_tokens = self.token_counter.count_tokens(&doc.text);

        if total_tokens <= self.config.chunk_size {
            return Ok(vec![DocumentChunk {
                id: format!("{}-0", doc_id),
                index: 0,
                text: doc.text.clone(),
                token_count: total_tokens,
                start_position: 0,
                end_position: doc.text.len(),
                section: None,
                section_path: Vec::new(),
                metadata: ChunkMetadata {
                    is_section_start: true,
                    is_section_end: true,
                    custom: HashMap::new(),
                },
            }]);
        }

        if self.config.use_semantic_chunking && !doc.toc.is_empty() {
            self.semantic_chunk(doc, doc_id)
        } else {
            self.sliding_window_chunk(doc, doc_id)
        }
    }

    /// 基于文档结构的语义分块。
    fn semantic_chunk(&self, doc: &ParsedDocument, doc_id: &str) -> Result<Vec<DocumentChunk>> {
        let mut chunks = Vec::new();
        let text = &doc.text;

        let sections = self.build_section_boundaries(doc);

        if sections.is_empty() {
            return self.sliding_window_chunk(doc, doc_id);
        }

        let mut chunk_index = 0;

        for (section_idx, section) in sections.iter().enumerate() {
            let section_text = if section_idx + 1 < sections.len() {
                &text[section.start..sections[section_idx + 1].start]
            } else {
                &text[section.start..]
            };

            let section_tokens = self.token_counter.count_tokens(section_text);

            if section_tokens <= self.config.chunk_size {
                chunks.push(DocumentChunk {
                    id: format!("{}-{}", doc_id, chunk_index),
                    index: chunk_index,
                    text: section_text.to_string(),
                    token_count: section_tokens,
                    start_position: section.start,
                    end_position: section.start + section_text.len(),
                    section: Some(section.title.clone()),
                    section_path: section.path.clone(),
                    metadata: ChunkMetadata {
                        is_section_start: true,
                        is_section_end: true,
                        custom: HashMap::new(),
                    },
                });
                chunk_index += 1;
            } else {
                let section_chunks = self.chunk_section(
                    section_text,
                    section.start,
                    doc_id,
                    chunk_index,
                    &section.title,
                    &section.path,
                )?;
                chunk_index += section_chunks.len();
                chunks.extend(section_chunks);
            }
        }

        Ok(chunks)
    }

    /// 从目录构建章节边界。
    fn build_section_boundaries(&self, doc: &ParsedDocument) -> Vec<SectionBoundary> {
        let mut sections = Vec::new();
        let text = &doc.text;

        for entry in &doc.toc {
            if let Some(pos) = self.find_heading_position(text, &entry.title) {
                let mut path = Vec::new();

                for prev_entry in doc.toc.iter().take_while(|e| e.level < entry.level) {
                    if prev_entry.level < entry.level {
                        path.push(prev_entry.title.clone());
                    }
                }
                path.push(entry.title.clone());

                sections.push(SectionBoundary {
                    title: entry.title.clone(),
                    level: entry.level,
                    start: pos,
                    path,
                });
            }
        }

        sections.sort_by_key(|s| s.start);
        sections
    }

    /// 在文本中查找标题的位置。
    fn find_heading_position(&self, text: &str, heading: &str) -> Option<usize> {
        for prefix in &["# ", "## ", "### ", "#### ", "##### ", "###### "] {
            let search = format!("{}{}", prefix, heading);
            if let Some(pos) = text.find(&search) {
                return Some(pos);
            }
        }
        text.find(heading)
    }

    /// 对超过分块大小的章节进行分块。
    fn chunk_section(
        &self,
        text: &str,
        base_position: usize,
        doc_id: &str,
        start_index: usize,
        section_title: &str,
        section_path: &[String],
    ) -> Result<Vec<DocumentChunk>> {
        let mut chunks = Vec::new();
        let paragraphs: Vec<&str> = text.split("\n\n").collect();

        let mut current_chunk = String::new();
        let mut current_start = 0;
        let mut chunk_index = start_index;
        let mut paragraph_start = 0;

        for (i, paragraph) in paragraphs.iter().enumerate() {
            let paragraph_tokens = self.token_counter.count_tokens(paragraph);
            let current_tokens = self.token_counter.count_tokens(&current_chunk);

            if current_tokens + paragraph_tokens > self.config.chunk_size
                && !current_chunk.is_empty()
            {
                let trimmed = current_chunk.trim();
                chunks.push(DocumentChunk {
                    id: format!("{}-{}", doc_id, chunk_index),
                    index: chunk_index,
                    text: trimmed.to_string(),
                    token_count: self.token_counter.count_tokens(trimmed),
                    start_position: base_position + current_start,
                    end_position: base_position + current_start + trimmed.len(),
                    section: Some(section_title.to_string()),
                    section_path: section_path.to_vec(),
                    metadata: ChunkMetadata {
                        is_section_start: chunk_index == start_index,
                        is_section_end: false,
                        custom: HashMap::new(),
                    },
                });
                chunk_index += 1;

                current_start = paragraph_start;
                current_chunk = String::new();
            }

            if !current_chunk.is_empty() {
                current_chunk.push_str("\n\n");
            }
            current_chunk.push_str(paragraph);
            paragraph_start += if i > 0 { 2 } else { 0 }
                + paragraphs.get(i.saturating_sub(1)).map_or(0, |p| p.len());
        }

        if !current_chunk.is_empty() {
            let trimmed = current_chunk.trim();
            let is_last = true;
            chunks.push(DocumentChunk {
                id: format!("{}-{}", doc_id, chunk_index),
                index: chunk_index,
                text: trimmed.to_string(),
                token_count: self.token_counter.count_tokens(trimmed),
                start_position: base_position + current_start,
                end_position: base_position + current_start + trimmed.len(),
                section: Some(section_title.to_string()),
                section_path: section_path.to_vec(),
                metadata: ChunkMetadata {
                    is_section_start: chunk_index == start_index,
                    is_section_end: is_last,
                    custom: HashMap::new(),
                },
            });
        }

        Ok(chunks)
    }

    /// 无结构文档的滑动窗口分块。
    fn sliding_window_chunk(
        &self,
        doc: &ParsedDocument,
        doc_id: &str,
    ) -> Result<Vec<DocumentChunk>> {
        let mut chunks = Vec::new();
        let text = &doc.text;
        let total_len = text.len();

        let sentences = self.split_into_sentences(text);

        let mut current_chunk = String::new();
        let mut current_start = 0;
        let mut chunk_index = 0;
        let mut char_position = 0;

        for sentence in sentences {
            let sentence_tokens = self.token_counter.count_tokens(sentence);
            let current_tokens = self.token_counter.count_tokens(&current_chunk);

            if current_tokens + sentence_tokens > self.config.chunk_size
                && !current_chunk.is_empty()
            {
                let trimmed = current_chunk.trim();
                chunks.push(DocumentChunk {
                    id: format!("{}-{}", doc_id, chunk_index),
                    index: chunk_index,
                    text: trimmed.to_string(),
                    token_count: self.token_counter.count_tokens(trimmed),
                    start_position: current_start,
                    end_position: current_start + trimmed.len(),
                    section: None,
                    section_path: Vec::new(),
                    metadata: ChunkMetadata {
                        is_section_start: chunk_index == 0,
                        is_section_end: false,
                        custom: HashMap::new(),
                    },
                });
                chunk_index += 1;

                current_start = char_position;
                current_chunk = String::new();
            }

            if !current_chunk.is_empty() {
                current_chunk.push(' ');
            }
            current_chunk.push_str(sentence);
            char_position += sentence.len() + 1;
        }

        if !current_chunk.is_empty() {
            let trimmed = current_chunk.trim();
            chunks.push(DocumentChunk {
                id: format!("{}-{}", doc_id, chunk_index),
                index: chunk_index,
                text: trimmed.to_string(),
                token_count: self.token_counter.count_tokens(trimmed),
                start_position: current_start,
                end_position: (current_start + trimmed.len()).min(total_len),
                section: None,
                section_path: Vec::new(),
                metadata: ChunkMetadata {
                    is_section_start: chunk_index == 0,
                    is_section_end: true,
                    custom: HashMap::new(),
                },
            });
        }

        Ok(chunks)
    }

    /// 将文本分割成句子。
    fn split_into_sentences<'a>(&self, text: &'a str) -> Vec<&'a str> {
        let mut sentences = Vec::new();
        let mut start = 0;
        let chars: Vec<char> = text.chars().collect();

        for (i, &c) in chars.iter().enumerate() {
            if c == '.' || c == '!' || c == '?' || c == '\n' {
                let next_char = chars.get(i + 1);
                if next_char.is_none_or(|&nc| nc.is_whitespace() || nc == '\n') {
                    sentences.push(&text[start..=i]);
                    start = i + 1;
                }
            }
        }

        if start < text.len() {
            sentences.push(&text[start..]);
        }

        sentences
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect()
    }
}

/// 章节边界信息。
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct SectionBoundary {
    title: String,
    level: usize,
    start: usize,
    path: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{DocumentMetadata, DocumentType, TocEntry};

    fn create_test_document(text: &str, toc: Vec<TocEntry>) -> ParsedDocument {
        ParsedDocument {
            text: text.to_string(),
            doc_type: DocumentType::Markdown,
            page_count: None,
            language: None,
            toc,
            metadata: DocumentMetadata::default(),
        }
    }

    #[test]
    fn test_small_document_single_chunk() {
        let chunker = DocumentChunker::with_defaults().unwrap();
        let doc = create_test_document("This is a small document.", vec![]);
        let chunks = chunker.chunk_document(&doc, "test").unwrap();

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "This is a small document.");
    }

    #[test]
    fn test_chunking_config() {
        let config = ChunkingConfig::new()
            .with_chunk_size(500)
            .with_overlap(50)
            .with_semantic_chunking(false);

        assert_eq!(config.chunk_size, 500);
        assert_eq!(config.chunk_overlap, 50);
        assert!(!config.use_semantic_chunking);
    }

    #[test]
    fn test_sliding_window_chunking() {
        let config = ChunkingConfig::new()
            .with_chunk_size(20)
            .with_semantic_chunking(false);
        let chunker = DocumentChunker::new(config).unwrap();

        let long_text = "This is sentence one. This is sentence two. This is sentence three. This is sentence four. This is sentence five. This is sentence six. This is sentence seven. This is sentence eight. This is sentence nine. This is sentence ten.";
        let doc = create_test_document(long_text, vec![]);
        let chunks = chunker.chunk_document(&doc, "test").unwrap();

        assert!(
            chunks.len() >= 1,
            "Expected at least 1 chunk, got {}",
            chunks.len()
        );

        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.index, i);
        }
    }

    #[test]
    fn test_semantic_chunking_with_toc() {
        let config = ChunkingConfig::new()
            .with_chunk_size(50)
            .with_semantic_chunking(true);
        let chunker = DocumentChunker::new(config).unwrap();

        let text = "# Chapter 1\n\nThis is the first chapter with some content.\n\n# Chapter 2\n\nThis is the second chapter with more content.";
        let toc = vec![
            TocEntry {
                title: "Chapter 1".to_string(),
                level: 1,
                page: None,
                anchor: None,
            },
            TocEntry {
                title: "Chapter 2".to_string(),
                level: 1,
                page: None,
                anchor: None,
            },
        ];
        let doc = create_test_document(text, toc);
        let chunks = chunker.chunk_document(&doc, "test").unwrap();

        assert!(!chunks.is_empty(), "Expected at least 1 chunk");

        let total_text: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert!(
            total_text.contains("Chapter 1")
                || total_text.contains("Chapter 2")
                || total_text.contains("content")
        );
    }

    #[test]
    fn test_chunk_positions() {
        let chunker = DocumentChunker::with_defaults().unwrap();
        let doc = create_test_document("Hello world", vec![]);
        let chunks = chunker.chunk_document(&doc, "test").unwrap();

        assert_eq!(chunks[0].start_position, 0);
        assert!(chunks[0].end_position > 0);
    }
}

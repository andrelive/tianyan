//! 用于生成分层摘要的摘要引擎。

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::common::error::Result;
use crate::common::token_estimator::estimate_tokens;
use crate::model::ChatService;

/// 不同内容级别的 token 限制。
pub const ABSTRACT_TOKEN_LIMIT: usize = 100;
/// Overview 级别 token 限制。
pub const OVERVIEW_TOKEN_LIMIT: usize = 2000;

/// 摘要级别枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryLevel {
    /// L0: 摘要级别，约 100 个 token。
    Abstract,
    /// L1: 概览级别，约 2K 个 token。
    Overview,
}

impl SummaryLevel {
    /// 获取此级别的 token 限制。
    pub fn token_limit(&self) -> usize {
        match self {
            SummaryLevel::Abstract => ABSTRACT_TOKEN_LIMIT,
            SummaryLevel::Overview => OVERVIEW_TOKEN_LIMIT,
        }
    }

    /// 获取此级别的名称。
    pub fn name(&self) -> &'static str {
        match self {
            SummaryLevel::Abstract => "abstract",
            SummaryLevel::Overview => "overview",
        }
    }
}

/// 摘要生成服务 seam。
///
/// 生产实现 [`SummaryEngine`]（LLM 生成）；测试实现 `MockSummaryEngine`
/// 为第二适配器（项目 seam 原则：两个适配器 = 真实 seam）。
/// 消费方（KnowledgeIngestor、SummaryTask）经 `Arc<dyn SummaryService>`
/// 依赖此接口，可直接注入 mock 测试摘要路径。
#[async_trait]
pub trait SummaryService: Send + Sync {
    /// 生成摘要（L0）。
    async fn generate_abstract(&self, content: &str) -> Result<String>;

    /// 生成概览（L1）。
    async fn generate_overview(&self, content: &str) -> Result<String>;

    /// 同时生成摘要和概览。
    async fn generate_summaries(&self, content: &str) -> Result<(String, String)> {
        let abstract_content = self.generate_abstract(content).await?;
        let overview_content = self.generate_overview(content).await?;
        Ok((abstract_content, overview_content))
    }

    /// 生成摘要，失败时使用截断回退（L0 = 前 500 字符，L1 = 全文）。
    ///
    /// 统一 knowledge 摄入与定时摘要任务的失败降级语义：
    /// LLM 摘要不可用时内容仍可检索，不阻塞写路径。
    async fn generate_or_fallback(&self, content: &str) -> (String, String) {
        match self.generate_summaries(content).await {
            Ok((abs, ov)) => (abs, ov),
            Err(e) => {
                tracing::warn!(error = %e, "摘要生成失败，使用截断回退");
                (content.chars().take(500).collect(), content.to_string())
            }
        }
    }
}

/// 文档目录（L1 的结构化形态；ADR-001 修订 2026-09-22）。
///
/// 叶节点的 L1 = 章节目录（本结构）；目录节点不生成（消费时 `vfs_list` 现遍历）。
/// 序列化为 JSON 存入 L1；消费侧经 [`parse_doc_index`] 识别（解析失败按旧语义：
/// 概览文本 / 全文直用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocIndex {
    /// 形态标记（恒为 `index`；LLM 省略时按 index 处理）。
    #[serde(default = "default_index_kind")]
    pub kind: String,
    /// 章节条目。
    pub sections: Vec<IndexSection>,
}

fn default_index_kind() -> String {
    "index".to_string()
}

/// 目录中的一个章节条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSection {
    /// 章节标题（原文标题原样 / LLM 归纳）。
    pub title: String,
    /// 一句话摘要（该章节讲什么）。
    pub summary: String,
    /// 起始锚点（原文逐字片段；供程序定位——**LLM 不报行号**）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// 起始行号（1 起始；程序回填；定位失败缺省）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<usize>,
    /// 结束行号（1 起始、含；程序按下一节起点回填）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
}

/// 解析文本中的文档目录（JSON）。非目录 / 解析失败 / sections 为空 → `None`
/// （调用方按旧语义处理：概览文本或全文直用）。
pub fn parse_doc_index(text: &str) -> Option<DocIndex> {
    let value = crate::common::llm_judge::parse_llm_json(text)?;
    let index: DocIndex = serde_json::from_value(value).ok()?;
    if index.sections.is_empty() {
        return None;
    }
    Some(index)
}

/// 将目录渲染为 markdown 列表（注入/展示用；附行号供按需取段）。
pub fn render_doc_index(index: &DocIndex) -> String {
    let mut out = String::new();
    for section in &index.sections {
        match (section.start_line, section.end_line) {
            (Some(start), Some(end)) => {
                out.push_str(&format!(
                    "- {}（行 {}-{}）：{}\n",
                    section.title, start, end, section.summary
                ));
            }
            _ => {
                out.push_str(&format!("- {}：{}\n", section.title, section.summary));
            }
        }
    }
    out
}

/// 程序侧定位校准：按 title / anchor 在原文定位各章节起始行，回填
/// `start_line` / `end_line`（end = 下一节起点 − 1；末节 = 文档末行）。
///
/// **位置由程序算**——LLM 只给标题与锚点（报行号会数错/幻觉）；定位失败的
/// 章节留空（位置未知，内容仍可读）。
pub fn calibrate_index_lines(index: &mut DocIndex, content: &str) {
    let total_lines = content.lines().count();
    let starts: Vec<Option<usize>> = index
        .sections
        .iter()
        .map(|s| locate_section_start(content, &s.title, s.anchor.as_deref()))
        .collect();
    for (i, section) in index.sections.iter_mut().enumerate() {
        section.start_line = starts[i];
        section.end_line = match starts[i] {
            Some(start) => {
                let next_start = starts[i + 1..].iter().flatten().next().copied();
                let end = next_start
                    .filter(|&n| n > start)
                    .map(|n| n - 1)
                    .unwrap_or(total_lines);
                Some(end.max(start))
            }
            None => None,
        };
    }
}

/// 在原文中定位章节起始行（1 起始）。
///
/// 顺序：① 标题行精确匹配（归一化 `#` 前缀与空白后相等）；② 行包含标题；
/// ③ anchor 首行片段匹配。都失败 → `None`。
fn locate_section_start(content: &str, title: &str, anchor: Option<&str>) -> Option<usize> {
    let title_clean = normalize_heading(title);
    if !title_clean.is_empty() {
        for (idx, line) in content.lines().enumerate() {
            if normalize_heading(line) == title_clean {
                return Some(idx + 1);
            }
        }
        for (idx, line) in content.lines().enumerate() {
            if normalize_heading(line).contains(&title_clean) {
                return Some(idx + 1);
            }
        }
    }
    if let Some(anchor) = anchor {
        let first = anchor.lines().next().unwrap_or("").trim();
        if !first.is_empty() {
            if let Some(pos) = content.find(first) {
                return Some(content[..pos].matches('\n').count() + 1);
            }
        }
    }
    None
}

/// 标题归一：去 markdown `#` 前缀与首尾空白（定位比较用）。
fn normalize_heading(s: &str) -> String {
    s.trim().trim_start_matches('#').trim().to_string()
}

/// 用于生成分层摘要的摘要引擎。
pub struct SummaryEngine {
    model_service: Arc<dyn ChatService>,
    model_name: String,
}

impl SummaryEngine {
    /// 创建新的摘要引擎。
    pub fn new(model_service: Arc<dyn ChatService>, model_name: impl Into<String>) -> Self {
        Self {
            model_service,
            model_name: model_name.into(),
        }
    }

    /// 为图片描述生成摘要。
    pub async fn generate_image_abstract(&self, unified_text: &str) -> Result<String> {
        let prompt = format!(
            r#"请用一句话（不超过 50 字）概括这张图片。

{}

请直接输出概括，不要包含其他说明。"#,
            unified_text
        );

        let response = self
            .model_service
            .chat(
                &self.model_name,
                vec![crate::common::types::Message::user(&prompt)],
            )
            .await?;

        Ok(response)
    }

    /// 为图片生成概览。
    pub async fn generate_image_overview(
        &self,
        description: &str,
        elements: &[String],
        ocr_text: Option<&str>,
    ) -> Result<String> {
        let mut overview = format!("## 图片描述\n{}\n\n", description);

        if !elements.is_empty() {
            overview.push_str("## 识别元素\n");
            for elem in elements {
                overview.push_str(&format!("- {}\n", elem));
            }
            overview.push('\n');
        }

        if let Some(text) = ocr_text {
            if !text.is_empty() {
                overview.push_str(&format!("## 图中文字\n```\n{}\n```\n", text));
            }
        }

        Ok(overview)
    }
}

#[async_trait]
impl SummaryService for SummaryEngine {
    /// 生成摘要（L0）。
    ///
    /// 摘要是约 100 个 token 的简洁摘要，
    /// 用于向量搜索和快速过滤。
    async fn generate_abstract(&self, content: &str) -> Result<String> {
        let prompt = format!(
            r#"请为以下内容生成一个简洁的摘要，限制在 100 个 token 以内。
摘要应该：
1. 突出主要内容
2. 包含关键信息
3. 便于快速理解

内容：
{}

请直接输出摘要，不要包含其他说明。"#,
            content
        );

        let response = self
            .model_service
            .chat(
                &self.model_name,
                vec![crate::common::types::Message::user(&prompt)],
            )
            .await?;

        Ok(response)
    }

    /// 生成概览（L1）。
    ///
    /// 概览（L1）= **目录**：便于读者定位章节（ADR-001 修订 2026-09-22）。
    ///
    /// **压缩契约**（沿用 + 扩充）：
    /// - 内容不超过概览容量（[`OVERVIEW_TOKEN_LIMIT`]）时直接复用原文——全文
    ///   即内容，目录无导航收益，LLM 生成只会引入扩写/脑补风险；
    /// - 超容量时 LLM 生成：优先输出**结构化目录**（JSON，见 [`DocIndex`]：
    ///   分节 + 每节一句话摘要 + 锚点）；内容不适合分节（脚本/表格等）时输出
    ///   精炼概览（纯文本，沿用旧语义）；
    /// - **位置由程序回填**（[`calibrate_index_lines`]）——LLM 不报行号；
    /// - 长度守卫：输出不得长于原文（防扩写回归），超限回退原文。
    async fn generate_overview(&self, content: &str) -> Result<String> {
        // 短内容直用：全文即内容（目录无语义压缩空间）
        if estimate_tokens(content) <= OVERVIEW_TOKEN_LIMIT {
            return Ok(content.to_string());
        }

        let prompt = format!(
            r#"请为以下内容生成"目录"（供读者定位章节用，不要复述内容）。

要求：
1. 把内容分成若干部分，每部分给出：
   - title：该部分标题（原文有标题时**必须逐字原样使用**）
   - summary：一句话说明该部分讲什么（不超过 60 字）
   - anchor：该部分起始处的**逐字连续原文片段**（10~30 字，与原文完全一致，用于定位）
2. **默认输出目录**——内容含多个主题/多条要点/多个步骤时都必须分节；仅当内容确实是代码/脚本/数据表等无法分节的形态时，才输出一段精炼概览（**纯文本**，不要 JSON）。
3. 忠实原文——只包含原文出现的信息，不得引入原文之外的内容。

目录输出格式（直接输出 JSON，不要其他说明）：
{{"kind":"index","sections":[{{"title":"...","summary":"...","anchor":"..."}}]}}

内容：
{content}"#
        );

        let response = self
            .model_service
            .chat(
                &self.model_name,
                vec![crate::common::types::Message::user(&prompt)],
            )
            .await?;

        // 目录路径：解析成功 → 程序定位校准后回填行号
        if let Some(mut index) = parse_doc_index(&response) {
            calibrate_index_lines(&mut index, content);
            if let Ok(json) = serde_json::to_string(&index) {
                // 长度守卫（防扩写回归）：目录 JSON 不得长于原文
                if json.chars().count() <= content.chars().count() {
                    return Ok(json);
                }
                tracing::warn!(
                    original_chars = content.chars().count(),
                    index_chars = json.chars().count(),
                    "目录生成超过原文长度，回退为原文"
                );
                return Ok(content.to_string());
            }
        }

        // 非目录（概览文本 / 解析失败）——沿用旧语义与守卫
        if response.chars().count() > content.chars().count() {
            tracing::warn!(
                original_chars = content.chars().count(),
                overview_chars = response.chars().count(),
                "概览生成超过原文长度，回退为原文"
            );
            return Ok(content.to_string());
        }

        Ok(response)
    }
}

/// 用于测试的模拟摘要引擎（`SummaryService` 第二适配器）。
#[cfg(test)]
pub struct MockSummaryEngine;

#[cfg(test)]
impl Default for MockSummaryEngine {
    fn default() -> Self {
        Self
    }
}

#[cfg(test)]
#[async_trait]
impl SummaryService for MockSummaryEngine {
    async fn generate_abstract(&self, content: &str) -> Result<String> {
        Ok(content.to_string())
    }

    async fn generate_overview(&self, content: &str) -> Result<String> {
        Ok(content.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::error::TianyanError;
    use crate::common::types::TokenUsage;
    use crate::model::types::{
        ChatChoice, ChatCompletionResponse, EmbeddingRequest, EmbeddingResponse,
    };
    use crate::model::{EmbeddingService, MockChatService};
    use async_trait::async_trait;

    /// Helper to build a ChatCompletionResponse from a content string.
    fn mock_chat_response(content: &str) -> ChatCompletionResponse {
        ChatCompletionResponse {
            id: "test".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: crate::common::types::Message::assistant(content),
                finish_reason: Some("stop".to_string()),
            }],
            usage: TokenUsage::default(),
        }
    }

    mockall::mock! {
        pub TestEmbeddingService {}
        #[async_trait]
        impl EmbeddingService for TestEmbeddingService {
            async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse>;
            fn embedding_dimension(&self, model: &str) -> usize;
        }
    }

    #[test]
    fn test_summary_level_token_limit() {
        assert_eq!(SummaryLevel::Abstract.token_limit(), 100);
        assert_eq!(SummaryLevel::Overview.token_limit(), 2000);
    }

    #[test]
    fn test_summary_level_variants() {
        let abstract_level = SummaryLevel::Abstract;
        let overview_level = SummaryLevel::Overview;

        assert_ne!(abstract_level.token_limit(), overview_level.token_limit());
        assert!(abstract_level.token_limit() < overview_level.token_limit());
    }

    #[test]
    fn test_mock_summary_engine() {
        let engine = MockSummaryEngine;
        let content = "This is test content for the summary engine.";
        // SummaryService 是 async trait——同步测试只验证 mock 可构造
        let _ = &engine;
        assert!(!content.is_empty());
    }

    #[test]
    fn test_mock_summary_engine_default() {
        let engine = MockSummaryEngine;
        let content = "Test content";
        // SummaryService 是 async trait——同步测试只验证默认构造
        let _ = &engine;
        assert!(!content.is_empty());
    }

    #[tokio::test]
    async fn test_mock_summary_engine_async() {
        let engine = MockSummaryEngine;
        let (abs, ov) = engine.generate_summaries("content").await.unwrap();
        assert_eq!(abs, "content");
        assert_eq!(ov, "content");
    }

    #[tokio::test]
    async fn test_generate_or_fallback_success() {
        let mut chat = MockChatService::new();
        chat.expect_chat_completion()
            .returning(|_| Ok(mock_chat_response("Test abstract summary")));
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");
        let (abs, ov) = engine.generate_or_fallback("Some content").await;
        // L0 走 LLM；L1 短内容直用（不调 LLM）
        assert_eq!(abs, "Test abstract summary");
        assert_eq!(ov, "Some content");
    }

    #[tokio::test]
    async fn test_generate_or_fallback_failure_truncates() {
        let mut chat = MockChatService::new();
        chat.expect_chat_completion()
            .returning(|_| Err(TianyanError::Custom("模拟摘要服务不可用".to_string())));
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");
        // 超过概览容量（≈3000 token）以覆盖概览的 LLM 失败回退路径
        let long = "x".repeat(9000);
        let (abs, ov) = engine.generate_or_fallback(&long).await;
        assert_eq!(abs.chars().count(), 500, "回退摘要截断为 500 字符");
        assert_eq!(ov, long, "回退概览为全文");
    }

    #[test]
    fn test_token_limit_constants() {
        assert_eq!(ABSTRACT_TOKEN_LIMIT, 100);
        assert_eq!(OVERVIEW_TOKEN_LIMIT, 2000);
        const { assert!(ABSTRACT_TOKEN_LIMIT < OVERVIEW_TOKEN_LIMIT) };
    }

    /// ── 真实 SummaryEngine 测试 ─────────────────────────────────────────

    #[test]
    fn test_real_engine_constructor() {
        let chat = MockChatService::new();
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");
        assert_eq!(engine.model_name, "test-model");
    }

    #[tokio::test]
    async fn test_generate_abstract_with_mock() {
        let mut chat = MockChatService::new();
        chat.expect_chat_completion()
            .returning(|_| Ok(mock_chat_response("Test abstract summary")));
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");
        let result = engine.generate_abstract("Some content").await.unwrap();
        assert_eq!(result, "Test abstract summary");
    }

    #[tokio::test]
    async fn test_generate_overview_short_content_reused() {
        // 短内容（≤ 2000 token）无压缩空间：直接复用原文，不调 LLM
        let mut chat = MockChatService::new();
        chat.expect_chat_completion().times(0);
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");
        let result = engine.generate_overview("Some content").await.unwrap();
        assert_eq!(result, "Some content");
    }

    #[tokio::test]
    async fn test_generate_overview_long_content_calls_llm() {
        // 超容量内容（> 2000 token）：经 LLM 生成概览
        let mut chat = MockChatService::new();
        chat.expect_chat_completion()
            .returning(|_| Ok(mock_chat_response("生成的长文档概览")));
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");
        let long = "内容".repeat(1600); // 3200 CJK 字 ≈ 2133 token > 2000
        let result = engine.generate_overview(&long).await.unwrap();
        assert_eq!(result, "生成的长文档概览");
    }

    #[tokio::test]
    async fn test_generate_overview_expansion_guard_falls_back() {
        // 守卫：LLM 输出超过原文（扩写/脑补回归）→ 回退原文
        let mut chat = MockChatService::new();
        let expanded = "内容".repeat(1600) + &"扩".repeat(200);
        chat.expect_chat_completion()
            .returning(move |_| Ok(mock_chat_response(&expanded)));
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");
        let long = "内容".repeat(1600);
        let result = engine.generate_overview(&long).await.unwrap();
        assert_eq!(result, long, "超过原文长度的概览必须回退原文");
    }

    #[tokio::test]
    async fn test_generate_overview_parses_index_and_calibrates_lines() {
        // 目录路径：LLM 输出结构化目录 → 程序按标题/锚点回填行号
        // （LLM 不报行号——位置一律由程序定位）。
        let mut chat = MockChatService::new();
        let index_json = r###"{"kind":"index","sections":[
            {"title":"安装","summary":"如何安装","anchor":"## 安装"},
            {"title":"配置","summary":"如何配置","anchor":"## 配置"}
        ]}"###;
        chat.expect_chat_completion()
            .returning(move |_| Ok(mock_chat_response(index_json)));
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");

        // 超容量内容（>2000 token），含两个标题行与已知行号结构：
        // 行 1 = 前言；行 2..401 = 填充；行 402 = ## 安装；403..802 = 安装细节；
        // 行 803 = ## 配置；804..1203 = 配置细节。
        let mut long = String::from("前言\n");
        for _ in 0..400 {
            long.push_str("填充内容\n");
        }
        long.push_str("## 安装\n");
        for _ in 0..400 {
            long.push_str("安装细节\n");
        }
        long.push_str("## 配置\n");
        for _ in 0..400 {
            long.push_str("配置细节\n");
        }

        let result = engine.generate_overview(&long).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).expect("应输出 JSON 目录");
        assert_eq!(v["kind"].as_str(), Some("index"));
        let sections = v["sections"].as_array().unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0]["start_line"].as_u64(), Some(402));
        assert_eq!(sections[0]["end_line"].as_u64(), Some(802));
        assert_eq!(sections[1]["start_line"].as_u64(), Some(803));
        assert_eq!(sections[1]["end_line"].as_u64(), Some(1203));
    }

    #[tokio::test]
    async fn test_generate_overview_index_with_unlocatable_anchor_leaves_line_empty() {
        // 定位失败（标题/锚点都不在原文）→ 行号缺省（位置未知，章节仍可读）
        let mut chat = MockChatService::new();
        let index_json = r#"{"kind":"index","sections":[{"title":"不存在的标题","summary":"x","anchor":"不存在的片段"}]}"#;
        chat.expect_chat_completion()
            .returning(move |_| Ok(mock_chat_response(index_json)));
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");
        let long = "内容".repeat(1600);
        let result = engine.generate_overview(&long).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(
            v["sections"][0].get("start_line").is_none(),
            "定位失败应缺省行号: {v}"
        );
        assert!(v["sections"][0].get("end_line").is_none());
    }

    #[tokio::test]
    async fn test_generate_overview_index_expansion_guard_falls_back() {
        // 守卫：目录 JSON 长于原文（扩写回归）→ 回退原文
        let mut chat = MockChatService::new();
        let big_index = format!(
            r#"{{"kind":"index","sections":[{{"title":"{}","summary":"{}","anchor":"{}"}}]}}"#,
            "题".repeat(2000),
            "述".repeat(2000),
            "锚".repeat(2000)
        );
        chat.expect_chat_completion()
            .returning(move |_| Ok(mock_chat_response(&big_index)));
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");
        let long = "内容".repeat(1600);
        let result = engine.generate_overview(&long).await.unwrap();
        assert_eq!(result, long, "目录超过原文长度必须回退原文");
    }

    #[tokio::test]
    async fn test_generate_overview_invalid_json_treated_as_prose() {
        // 非目录（无法解析为 JSON 目录）→ 沿用旧语义：文本原样返回
        let mut chat = MockChatService::new();
        chat.expect_chat_completion()
            .returning(|_| Ok(mock_chat_response("{这不是合法 JSON")));
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");
        let long = "内容".repeat(1600);
        let result = engine.generate_overview(&long).await.unwrap();
        assert_eq!(result, "{这不是合法 JSON");
    }

    #[tokio::test]
    async fn test_generate_summaries_with_mock() {
        // L0 走 LLM；L1 短内容直用（不调 LLM）
        let mut chat = MockChatService::new();
        chat.expect_chat_completion()
            .returning(|_| Ok(mock_chat_response("Test abstract summary")));
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");
        let (abstract_result, overview_result) =
            engine.generate_summaries("Some content").await.unwrap();
        assert_eq!(abstract_result, "Test abstract summary");
        assert_eq!(overview_result, "Some content");
    }

    #[tokio::test]
    async fn test_generate_image_abstract_with_mock() {
        let mut chat = MockChatService::new();
        chat.expect_chat_completion()
            .returning(|_| Ok(mock_chat_response("A cat sitting on a chair")));
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");
        let result = engine.generate_image_abstract("Cat image").await.unwrap();
        assert_eq!(result, "A cat sitting on a chair");
    }

    #[tokio::test]
    async fn test_generate_image_overview() {
        let chat = MockChatService::new();
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");

        // description only — no elements, no ocr_text
        let result = engine
            .generate_image_overview("A cat", &[], None)
            .await
            .unwrap();
        assert!(
            result.contains("## 图片描述"),
            "should have description header"
        );
        assert!(result.contains("A cat"), "should contain description text");
        assert!(
            !result.contains("## 识别元素"),
            "should NOT have elements section"
        );
        assert!(
            !result.contains("## 图中文字"),
            "should NOT have OCR section"
        );

        // description + elements, no OCR
        let result = engine
            .generate_image_overview("A cat", &["cat".to_string(), "chair".to_string()], None)
            .await
            .unwrap();
        assert!(result.contains("## 图片描述"));
        assert!(result.contains("## 识别元素"));
        assert!(result.contains("- cat"));
        assert!(result.contains("- chair"));
        assert!(!result.contains("## 图中文字"));

        // description + ocr_text, no elements
        let result = engine
            .generate_image_overview("A sign", &[], Some("Hello World"))
            .await
            .unwrap();
        assert!(result.contains("## 图片描述"));
        assert!(!result.contains("## 识别元素"));
        assert!(result.contains("## 图中文字"));
        assert!(result.contains("Hello World"));
    }

    #[tokio::test]
    async fn test_generate_image_overview_full() {
        let chat = MockChatService::new();
        let engine = SummaryEngine::new(Arc::new(chat), "test-model");

        let result = engine
            .generate_image_overview(
                "A busy street",
                &[
                    "car".to_string(),
                    "pedestrian".to_string(),
                    "traffic light".to_string(),
                ],
                Some("STOP\nWALK"),
            )
            .await
            .unwrap();

        assert!(result.contains("## 图片描述"));
        assert!(result.contains("A busy street"));
        assert!(result.contains("## 识别元素"));
        assert!(result.contains("- car"));
        assert!(result.contains("- pedestrian"));
        assert!(result.contains("- traffic light"));
        assert!(result.contains("## 图中文字"));
        assert!(result.contains("STOP"));
        assert!(result.contains("WALK"));

        // Verify format order: description → elements → OCR
        let desc_pos = result.find("## 图片描述").unwrap();
        let elem_pos = result.find("## 识别元素").unwrap();
        let ocr_pos = result.find("## 图中文字").unwrap();
        assert!(
            desc_pos < elem_pos,
            "description should come before elements"
        );
        assert!(elem_pos < ocr_pos, "elements should come before OCR text");
    }
}

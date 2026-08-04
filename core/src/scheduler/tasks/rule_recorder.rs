//! 规则记录器。
//!
//! Harness Engineering 核心：每次智能体失败时，追加一条规则到 agent/learned/，
//! 确保同一失败永不发生第二次。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{AgentPath, ContentLevel};
use crate::vfs::VirtualFileSystem;

/// 失败类型分类。
///
/// 不同失败应有不同的处理策略：
/// - Transient: 不记录（网络超时等瞬态错误）
/// - Logic: 记录为 learned rule（Planner/Executor 逻辑错误）
/// - Input: 轻量记录（用户输入模糊，标记来源）
/// - System: 记录并告警（系统级故障）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// 瞬态错误（网络超时、临时服务不可用），不应记录为规则。
    Transient,
    /// 逻辑错误（Planner 误判、Executor 执行失败），值得记录为规则。
    Logic,
    /// 用户输入错误（模糊指令、信息不足），记录但不作为失败规则。
    Input,
    /// 系统级错误（配置缺失、存储损坏），记录并升级告警。
    System,
}

impl FailureKind {
    /// 是否应该记录为 learned rule。
    pub fn should_record(&self) -> bool {
        matches!(self, FailureKind::Logic | FailureKind::System)
    }

    /// 获取失败类型的中文标签。
    pub fn label(&self) -> &'static str {
        match self {
            FailureKind::Transient => "瞬态",
            FailureKind::Logic => "逻辑",
            FailureKind::Input => "输入",
            FailureKind::System => "系统",
        }
    }
}

/// 规则记录器。
#[derive(Clone)]
pub struct RuleRecorder {
    vfs: Arc<dyn VirtualFileSystem>,
    /// 生成此规则的模型版本。
    model_version: String,
    /// 生成此规则时的 Pipeline 版本。
    pipeline_version: String,
}

/// 规则记录器默认模型版本标识。
const DEFAULT_MODEL_VERSION: &str = "unknown";
/// 规则记录器默认 Pipeline 版本标识。
const DEFAULT_PIPELINE_VERSION: &str = "1.0";

impl RuleRecorder {
    /// 创建新的规则记录器。
    pub fn new(vfs: Arc<dyn VirtualFileSystem>) -> Self {
        Self {
            vfs,
            model_version: DEFAULT_MODEL_VERSION.to_string(),
            pipeline_version: DEFAULT_PIPELINE_VERSION.to_string(),
        }
    }

    /// 设置模型版本（用于记录规则生成来源）。
    pub fn with_model_version(mut self, version: impl Into<String>) -> Self {
        self.model_version = version.into();
        self
    }

    /// 设置 Pipeline 版本（用于记录规则生成来源）。
    pub fn with_pipeline_version(mut self, version: impl Into<String>) -> Self {
        self.pipeline_version = version.into();
        self
    }

    /// 带失败类型分类的记录。
    ///
    /// Transient 失败不记录。Logic/System 失败记录为 learned rule。
    /// 写入前检查语义重复，避免相同规则多次写入。
    pub async fn record_with_kind(
        &self,
        abstract_text: &str,
        detail_text: &str,
        source_session: &str,
        kind: FailureKind,
    ) -> Result<()> {
        if !kind.should_record() {
            tracing::debug!(failure_kind = kind.label(), "跳过记录（非持久失败类型）");
            return Ok(());
        }

        // 去重检查：避免语义重复的规则重复写入
        if self.has_similar_rule(abstract_text).await? {
            tracing::info!(
                abstract_text = abstract_text,
                failure_kind = kind.label(),
                "跳过记录（已存在相似规则）"
            );
            return Ok(());
        }

        let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let rule_id = format!("rule-{}", ts);

        let uri = AgentPath::Learned.uri().append(&rule_id);

        self.vfs.create_file(&uri).await?;

        let abstract_content = format!(
            "[{}] {} (来源会话: {})",
            kind.label(),
            abstract_text,
            source_session
        );
        self.vfs.write_abstract(&uri, &abstract_content).await?;

        let full_detail = format!(
            "失败类型: {}\n来源会话: {}\n\n详情:\n{}\n\n---\n## 规则元数据\n- 生成模型: {}\n- Pipeline 版本: {}\n- 生成时间: {}\n",
            kind.label(),
            source_session,
            detail_text,
            self.model_version,
            self.pipeline_version,
            chrono::Utc::now().to_rfc3339()
        );
        self.vfs.write_content(&uri, &full_detail).await?;

        tracing::info!(
            rule_id = %uri.to_string(),
            session_id = %source_session,
            failure_kind = kind.label(),
            "已追加学习规则"
        );

        Ok(())
    }

    /// 检查 VFS 中是否已存在与 `abstract_text` 语义相似的规则。
    ///
    /// 通过比较已有规则的 Abstract 摘要与候选摘要的文本重叠度来判断。
    async fn has_similar_rule(&self, candidate_abstract: &str) -> Result<bool> {
        let learned_uri = AgentPath::Learned.uri();
        let entries = match self.vfs.list(&learned_uri).await {
            Ok(e) => e,
            Err(_) => return Ok(false),
        };

        let candidate_lower = candidate_abstract.to_lowercase();

        for entry in &entries {
            if let Ok(existing) = self
                .vfs
                .read_content(entry.uri(), ContentLevel::Abstract)
                .await
            {
                // 去掉类型标签前缀 "[逻辑]" / "[系统]"
                let existing_clean = existing
                    .trim_start_matches("[逻辑] ")
                    .trim_start_matches("[系统] ")
                    .trim_start_matches("[输入] ")
                    .to_lowercase();

                // 简单相似性检查：任一方向的包含关系或高重叠度
                if existing_clean.contains(&candidate_lower)
                    || candidate_lower.contains(&existing_clean)
                {
                    return Ok(true);
                }

                // 字符级 Jaccard 相似度近似
                let overlap = candidate_lower
                    .chars()
                    .filter(|c| existing_clean.contains(*c))
                    .count();
                let total = candidate_lower
                    .chars()
                    .count()
                    .max(existing_clean.chars().count());
                if total > 0 && (overlap as f32 / total as f32) > 0.85 {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use crate::common::types::{ContentLevel, ContextNamespace, SearchResult, TianyanUri};
    use crate::vfs::{ContentMetadata, ContentStore, ContextEntry, VfsCore, VfsSearch};

    /// Spy VFS that tracks create_file/write calls and stores content for list/read.
    struct SpyVfs {
        created_files: Mutex<Vec<String>>,
        written_content: Mutex<Vec<(String, String)>>, // (uri, content)
        content: Mutex<HashMap<(String, ContentLevel), String>>,
        entries: Mutex<HashMap<String, Vec<ContextEntry>>>,
    }

    impl SpyVfs {
        fn new() -> Self {
            Self {
                created_files: Mutex::new(Vec::new()),
                written_content: Mutex::new(Vec::new()),
                content: Mutex::new(HashMap::new()),
                entries: Mutex::new(HashMap::new()),
            }
        }

        fn created_count(&self) -> usize {
            self.created_files.lock().unwrap().len()
        }

        fn written_count(&self) -> usize {
            self.written_content.lock().unwrap().len()
        }

        fn add_content(&self, uri: &TianyanUri, level: ContentLevel, text: &str) {
            self.content
                .lock()
                .unwrap()
                .insert((uri.to_string(), level), text.to_string());
        }
    }

    #[async_trait]
    impl VfsCore for SpyVfs {
        async fn initialize(&self) -> Result<()> {
            Ok(())
        }
        async fn exists(&self, _uri: &TianyanUri) -> Result<bool> {
            Ok(true)
        }
        async fn get_entry(&self, uri: &TianyanUri) -> Result<ContextEntry> {
            Ok(ContextEntry::new_file(uri.clone()))
        }
        async fn create_directory(&self, uri: &TianyanUri) -> Result<ContextEntry> {
            Ok(ContextEntry::new_directory(uri.clone()))
        }
        async fn create_file(&self, uri: &TianyanUri) -> Result<ContextEntry> {
            self.created_files.lock().unwrap().push(uri.to_string());
            Ok(ContextEntry::new_file(uri.clone()))
        }
        async fn delete(&self, _uri: &TianyanUri) -> Result<()> {
            Ok(())
        }
        async fn list(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>> {
            // Include both pre-configured entries and files created via create_file.
            let mut result = self
                .entries
                .lock()
                .unwrap()
                .get(&uri.to_string())
                .cloned()
                .unwrap_or_default();

            // create_file records URIs — treat them as entries under the learned dir.
            let created = self.created_files.lock().unwrap();
            if uri.to_string() == AgentPath::Learned.uri().to_string() {
                for file_uri_str in created.iter() {
                    if let Ok(parsed) = TianyanUri::parse(file_uri_str) {
                        result.push(ContextEntry::new_file(parsed));
                    }
                }
            }

            Ok(result)
        }
        async fn move_entry(&self, _s: &TianyanUri, _d: &TianyanUri) -> Result<()> {
            Ok(())
        }
        async fn update_metadata(
            &self,
            _uri: &TianyanUri,
            _importance: f32,
            _custom: HashMap<String, serde_json::Value>,
        ) -> Result<()> {
            Ok(())
        }
        async fn get_all_content_metadata(
            &self,
            _uri: &TianyanUri,
        ) -> Result<HashMap<ContentLevel, ContentMetadata>> {
            Ok(HashMap::new())
        }
    }

    #[async_trait]
    impl ContentStore for SpyVfs {
        async fn write(&self, uri: &TianyanUri, _level: ContentLevel, content: &str) -> Result<()> {
            self.written_content
                .lock()
                .unwrap()
                .push((uri.to_string(), content.to_string()));
            self.add_content(uri, _level, content);
            Ok(())
        }
        async fn read(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String> {
            self.content
                .lock()
                .unwrap()
                .get(&(uri.to_string(), level))
                .cloned()
                .ok_or_else(|| {
                    crate::common::error::TianyanError::not_found(uri)
                })
        }
        async fn append(&self, _uri: &TianyanUri, _content: &str) -> Result<()> {
            Ok(())
        }
        async fn has_content(&self, _uri: &TianyanUri, _level: ContentLevel) -> Result<bool> {
            Ok(true)
        }
    }

    #[async_trait]
    impl VfsSearch for SpyVfs {
        async fn search(
            &self,
            _q: &str,
            _l: usize,
            _n: Option<ContextNamespace>,
        ) -> Result<Vec<SearchResult>> {
            Ok(vec![])
        }
        async fn search_by_visual(&self, _v: &[f32], _k: usize) -> Result<Vec<SearchResult>> {
            Ok(vec![])
        }
        async fn update_summary_vectors(
            &self,
            _uri: &TianyanUri,
            _a: &str,
            _o: &str,
        ) -> Result<()> {
            Ok(())
        }
    }

    impl VirtualFileSystem for SpyVfs {}

    // ── record_with_kind: Transient / Input should not write ────────

    #[tokio::test]
    async fn test_transient_not_recorded() {
        let vfs = Arc::new(SpyVfs::new());
        let recorder = RuleRecorder::new(vfs.clone());

        recorder
            .record_with_kind("summary", "detail", "s1", FailureKind::Transient)
            .await
            .unwrap();

        assert_eq!(vfs.created_count(), 0, "Transient should not create files");
        assert_eq!(vfs.written_count(), 0, "Transient should not write content");
    }

    #[tokio::test]
    async fn test_input_not_recorded() {
        let vfs = Arc::new(SpyVfs::new());
        let recorder = RuleRecorder::new(vfs.clone());

        recorder
            .record_with_kind("summary", "detail", "s1", FailureKind::Input)
            .await
            .unwrap();

        assert_eq!(vfs.created_count(), 0);
        assert_eq!(vfs.written_count(), 0);
    }

    // ── record_with_kind: Logic should write ────────────────────────

    #[tokio::test]
    async fn test_logic_recorded() {
        let vfs = Arc::new(SpyVfs::new());
        let recorder = RuleRecorder::new(vfs.clone());

        recorder
            .record_with_kind(
                "Always check file existence first.",
                "detail text",
                "s1",
                FailureKind::Logic,
            )
            .await
            .unwrap();

        assert!(
            vfs.created_count() > 0,
            "Logic failure should create a rule file"
        );
        assert!(
            vfs.written_count() > 0,
            "Logic failure should write rule content"
        );
    }

    // ── Dedup: exact abstraction should not duplicate ───────────────

    #[tokio::test]
    async fn test_duplicate_rule_not_recorded() {
        let vfs = Arc::new(SpyVfs::new());
        let recorder = RuleRecorder::new(vfs.clone());

        // Record first
        recorder
            .record_with_kind(
                "Check auth before accessing data.",
                "detail",
                "s1",
                FailureKind::Logic,
            )
            .await
            .unwrap();
        let count_after_first = vfs.created_count();

        // Try recording the same rule again
        recorder
            .record_with_kind(
                "Check auth before accessing data.",
                "detail2",
                "s2",
                FailureKind::Logic,
            )
            .await
            .unwrap();

        // No additional files created (dedup should catch it via list + read)
        // The first record created a file which appears in entries listing
        assert_eq!(
            vfs.created_count(),
            count_after_first,
            "Duplicate rule should not create a new file"
        );
    }

    // ── Dedup: substring containment ────────────────────────────────

    #[tokio::test]
    async fn test_similar_by_containment_not_recorded() {
        let vfs = Arc::new(SpyVfs::new());
        let recorder = RuleRecorder::new(vfs.clone());

        // Record a long rule
        recorder
            .record_with_kind(
                "Validate user input before processing any request.",
                "detail",
                "s1",
                FailureKind::Logic,
            )
            .await
            .unwrap();
        let count = vfs.created_count();

        // Try recording a shorter rule that is a substring of the first
        recorder
            .record_with_kind("Validate user input", "detail2", "s2", FailureKind::Logic)
            .await
            .unwrap();

        // The shorter text is contained in the existing rule, so it should be dedupped
        assert_eq!(
            vfs.created_count(),
            count,
            "Substring-contained rule should not duplicate"
        );
    }

    // ── Dedup: no match should create a new file ────────────────────

    #[tokio::test]
    async fn test_different_rule_recorded() {
        let vfs = Arc::new(SpyVfs::new());
        let recorder = RuleRecorder::new(vfs.clone());

        recorder
            .record_with_kind(
                "Close file handles after use.",
                "detail",
                "s1",
                FailureKind::Logic,
            )
            .await
            .unwrap();

        // Different rule
        recorder
            .record_with_kind(
                "Always validate network input.",
                "detail2",
                "s2",
                FailureKind::Logic,
            )
            .await
            .unwrap();

        // Both should be recorded (different topics)
        assert!(
            vfs.created_count() >= 2,
            "Different rules should each create a file"
        );
    }
}

//! 规则记录器。
//!
//! Harness Engineering 核心：每次智能体失败时，**按主题聚合**写入一条规则到
//! `agent/learned/`，确保同一失败永不发生第二次。
//!
//! **聚合口径**（2026-11 修）：规则按 `(工具, 失败类别)` 聚合成**一条**。同一主题
//! 反复失败时只累加计数并追加「最近失败」记录，不再每失败一次写一条新条目。
//!
//! **主题键编码在 URI 文件名里**（`rule-{tool}-{class}-{ts}`），刻意不放在 `abstract`：
//! `abstract` 会被 `SummaryTask` 用 LLM 重写（实测库里 84 条由本记录器写入的规则
//! **100% 被改写**成叙述长文），所以任何「以 abstract 文本相似度判重」的做法都只在
//! 下次摘要任务跑之前有效——跨天的同类失败永远判不出重复，规则库于是按失败次数
//! 线性膨胀（实测 84 条碎片按工具只有 18 类，`write_file` 10 条里 9 条同因）。
//! URI 文件名不会被任何摘要/改写流程触碰，判据因此稳定，且只是 O(条目数) 的
//! 纯字符串前缀比较——不需要读取任何条目内容。
//!
//! **累计计数的权威位置是 `detail`（`累计次数:` 行）**，不是 abstract——后者随时会被
//! 摘要任务改写，只有 detail 是可靠的。

use std::sync::Arc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{AgentPath, ContentLevel, TianyanUri};
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

/// 失败类别——规则的聚合维度之二（之一是工具名）。
///
/// **分类口径只有一处**：[`TianyanError`] 的语义谓词（ADR-014）。构造时传入错误对象，
/// 这里只做映射，**不在这里也不在调用方做 `contains` / `starts_with` 判定**——
/// 错误分类契约只住在 `common::error`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// 权限拒绝 / 安全策略阻止。
    Permission,
    /// 目标不存在（条目 / 目录 / 文件）。
    NotFound,
    /// 冲突（语义编辑锚点不匹配、目标状态过期）。
    Conflict,
    /// 无效输入（参数缺失 / 格式错误）。
    InvalidInput,
    /// 操作超时。
    Timeout,
    /// 操作已取消。
    Cancelled,
    /// 配置错误。
    Config,
    /// 其它（外层包装未携带语义前缀，无法归类）。
    Other,
}

impl FailureClass {
    /// 从错误对象派生类别（顺序无影响：语义前缀互斥）。
    pub fn of(err: &TianyanError) -> Self {
        if err.is_permission() {
            Self::Permission
        } else if err.is_not_found() {
            Self::NotFound
        } else if err.is_conflict() {
            Self::Conflict
        } else if err.is_invalid_input() {
            Self::InvalidInput
        } else if err.is_timeout() {
            Self::Timeout
        } else if err.is_cancelled() {
            Self::Cancelled
        } else if err.is_config() {
            Self::Config
        } else {
            Self::Other
        }
    }

    /// 进 URI 文件名的短标识（只含小写字母与连字符，无分隔符歧义）。
    pub fn slug(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::NotFound => "not-found",
            Self::Conflict => "conflict",
            Self::InvalidInput => "invalid-input",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Config => "config",
            Self::Other => "other",
        }
    }

    /// 写进 `abstract` 与 `detail` 的中文标签。
    pub fn label(self) -> &'static str {
        match self {
            Self::Permission => "权限拒绝",
            Self::NotFound => "目标不存在",
            Self::Conflict => "定位冲突",
            Self::InvalidInput => "参数无效",
            Self::Timeout => "执行超时",
            Self::Cancelled => "操作已取消",
            Self::Config => "配置错误",
            Self::Other => "执行失败",
        }
    }

    /// 从 label 反推类别（运维脚本按 `detail` 里落下的 label 重建摘要时用）。
    ///
    /// label 是写入时落下的**唯一真相**（`主题: tool / label`），比从错误行重新分类
    /// 更无损；反向映射因此留在本模块——脚本不自己 match 中文。
    pub fn from_label(label: &str) -> Option<Self> {
        let label = label.trim();
        [
            Self::Permission,
            Self::NotFound,
            Self::Conflict,
            Self::InvalidInput,
            Self::Timeout,
            Self::Cancelled,
            Self::Config,
            Self::Other,
        ]
        .into_iter()
        .find(|class| class.label() == label)
    }
}

/// 规则的聚合主题：`(工具, 失败类别)`。
///
/// 同主题的失败合并成一条规则；主题键的**唯一载体是 URI 文件名**，因为 `abstract`
/// 会被 `SummaryTask` 用 LLM 改写（见模块文档），拿它做判重会在摘要任务跑过之后失效。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleTopic {
    tool: String,
    class: FailureClass,
}

impl RuleTopic {
    /// 构造主题。工具名会被规整为 URI 安全片段（小写、非 `[a-z0-9_]` 换 `_`）。
    pub fn new(tool: &str, class: FailureClass) -> Self {
        let mut sanitized = String::with_capacity(tool.len());
        for ch in tool.chars() {
            if ch.is_ascii_alphanumeric() {
                sanitized.push(ch.to_ascii_lowercase());
            } else {
                sanitized.push('_');
            }
        }
        if sanitized.is_empty() {
            sanitized.push_str("unknown");
        }
        Self {
            tool: sanitized,
            class,
        }
    }

    /// 工具名（已规整）。
    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// 失败类别。
    pub fn class(&self) -> FailureClass {
        self.class
    }

    /// 规则文件名前缀（含开头 `rule-` 与结尾 `-`，用于同主题前缀匹配）。
    pub fn file_prefix(&self) -> String {
        format!("rule-{}-{}-", self.tool, self.class.slug())
    }

    /// 规则文件名（带写入时间戳，字典序即时间序）。
    fn file_name(&self, ts: &str) -> String {
        format!("rule-{}-{}-{}", self.tool, self.class.slug(), ts)
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

    /// 带失败类型分类的记录（**按主题聚合**）。
    ///
    /// `Transient` / `Input` 失败不记录；`Logic` / `System` 失败按 `topic` 落地：
    ///
    /// - 同主题已有条目 → 累加 `累计次数` 并追加一条「最近失败」记录（不新建文件）；
    /// - 无同主题条目 → 新建一条（命名 `rule-{tool}-{class}-{ts}`）。
    ///
    /// 判据是**文件名前缀**，不读任何条目内容，也不依赖 `abstract`（后者会被
    /// `SummaryTask` 改写，见模块文档）。
    pub async fn record_with_kind(
        &self,
        topic: &RuleTopic,
        abstract_text: &str,
        detail_text: &str,
        source_session: &str,
        kind: FailureKind,
    ) -> Result<()> {
        if !kind.should_record() {
            tracing::debug!(failure_kind = kind.label(), "跳过记录（非持久失败类型）");
            return Ok(());
        }

        let now = chrono::Utc::now();
        let now_rfc = now.to_rfc3339();
        let ts = now.format("%Y%m%d-%H%M%S").to_string();

        match self.find_topic_entry(topic).await? {
            Some(uri) => {
                self.append_occurrence(&uri, topic, detail_text, source_session, &now_rfc)
                    .await?;
                tracing::info!(
                    rule_id = %uri.to_string(),
                    session_id = %source_session,
                    failure_kind = kind.label(),
                    topic = %format!("{}/{}", topic.tool(), topic.class().slug()),
                    "已聚合到同主题规则（累加计数，未新建条目）"
                );
            }
            None => {
                self.create_rule(
                    topic,
                    abstract_text,
                    detail_text,
                    source_session,
                    &ts,
                    &now_rfc,
                    kind,
                )
                .await?;
                tracing::info!(
                    session_id = %source_session,
                    failure_kind = kind.label(),
                    topic = %format!("{}/{}", topic.tool(), topic.class().slug()),
                    "已新建主题规则（同主题首条）"
                );
            }
        }

        Ok(())
    }

    /// 找同主题下**最新**的规则条目（纯文件名前缀匹配，不读条目内容）。
    ///
    /// 文件名末尾是 `%Y%m%d-%H%M%S`，字典序即时间序，故取前缀命中里最大的那个。
    async fn find_topic_entry(&self, topic: &RuleTopic) -> Result<Option<TianyanUri>> {
        let dir = AgentPath::Learned.uri();
        let entries = match self.vfs.list(&dir).await {
            Ok(entries) => entries,
            // 目录尚未建立（首次写入）→ 视作「无同主题条目」
            Err(_) => return Ok(None),
        };

        let prefix = topic.file_prefix();
        let mut newest: Option<(String, TianyanUri)> = None;
        for entry in &entries {
            let uri = entry.uri();
            let Some(name) = uri.path().last().cloned() else {
                continue;
            };
            if !name.starts_with(&prefix) {
                continue;
            }
            if newest.as_ref().is_none_or(|(best, _)| name > *best) {
                newest = Some((name, uri.clone()));
            }
        }

        Ok(newest.map(|(_, uri)| uri))
    }

    /// 新建一条主题规则。
    #[allow(clippy::too_many_arguments)]
    async fn create_rule(
        &self,
        topic: &RuleTopic,
        abstract_text: &str,
        detail_text: &str,
        source_session: &str,
        ts: &str,
        now: &str,
        kind: FailureKind,
    ) -> Result<()> {
        let uri = AgentPath::Learned.uri().append(&topic.file_name(ts));
        self.vfs.create_file(&uri).await?;

        // abstract 统一走「场景 → 动作」口径（[`rule_abstract`]）：它会被 SummaryTask
        // 改写，也是检索命中的唯一依据；调用方给的 `abstract_text` 是首错摘要，降级
        // 进 detail 的「首次失败摘要」段——信息不丢，但不再污染检索。
        self.vfs
            .write_abstract(&uri, &rule_abstract(topic.tool(), topic.class(), 1))
            .await?;

        let full_detail = format!(
            "失败类型: {}\n工具: {}\n主题: {} / {}\n累计次数: 1\n最近发生: {}\n\n\
             ## 首次失败摘要\n{}\n\n\
             ## 最近失败\n{}\n\n\
             ## 首次失败详情\n{}\n\n\
             ---\n## 规则元数据\n- 生成模型: {}\n- Pipeline 版本: {}\n- 生成时间: {}\n",
            kind.label(),
            topic.tool(),
            topic.tool(),
            topic.class().label(),
            now,
            abstract_text.trim_end(),
            record_line(now, source_session, detail_text),
            detail_text.trim_end(),
            self.model_version,
            self.pipeline_version,
            now
        );
        self.vfs.write_content(&uri, &full_detail).await?;

        Ok(())
    }

    /// 把一次新失败并入已有主题条目：计数 +1、最近失败前插、保留最近 N 条。
    async fn append_occurrence(
        &self,
        uri: &TianyanUri,
        topic: &RuleTopic,
        detail_text: &str,
        source_session: &str,
        now: &str,
    ) -> Result<()> {
        let old = self
            .vfs
            .read_content(uri, ContentLevel::Detail)
            .await
            .unwrap_or_default();

        let occurrences = parse_occurrences(&old) + 1;
        let mut recent = parse_recent(&old);
        recent.insert(0, record_line(now, source_session, detail_text));
        recent.truncate(MAX_RECENT_OCCURRENCES);

        // 首次失败详情：新格式取「## 首次失败详情」之后的段；老格式（本改动前写入的
        // `rule-{ts}` 条目）把整份 detail 当作首次详情，保证信息不丢。
        let first = match old.split("## 首次失败详情").nth(1) {
            Some(rest) => rest.trim_start_matches('\n').trim_end().to_string(),
            None => old.trim_end().to_string(),
        };

        let merged = format!(
            "工具: {}\n主题: {} / {}\n累计次数: {}\n最近发生: {}\n\n\
             ## 最近失败\n{}\n\n\
             ## 首次失败详情\n{}\n\n\
             ---\n## 规则元数据\n- 合并模型: {}\n- Pipeline 版本: {}\n- 最近合并: {}\n",
            topic.tool(),
            topic.tool(),
            topic.class().label(),
            occurrences,
            now,
            recent.join("\n"),
            first,
            self.model_version,
            self.pipeline_version,
            now
        );
        self.vfs.write_content(uri, &merged).await?;

        // 合并路径与首次写入同一口径：abstract 是「场景 → 动作」，计数在尾部
        self.vfs
            .write_abstract(
                uri,
                &rule_abstract(topic.tool(), topic.class(), occurrences),
            )
            .await?;

        Ok(())
    }
}

/// 失败类别 → 应对动作（简短、可执行）。
///
/// 这是「规则可检索性」的另一半：abstract 只有场景没有动作，检索命中之后模型仍然
/// 不知道该怎么办。个别工具给特化动作，其余按类别给通用动作。
pub fn action_hint(tool: &str, class: FailureClass) -> &'static str {
    match (tool, class) {
        ("apply_patch" | "apply_edit", FailureClass::Conflict) => {
            "连续两次冲突即停，改用唯一短锚点（apply_edit）或按真实文本重写"
        }
        ("read_file" | "list_dir" | "glob", FailureClass::NotFound) => {
            "先用 glob / list_dir 确认真实路径，不要换路径猜"
        }
        ("execute_command", FailureClass::Timeout) => {
            "缩短命令或改 background:true 后重试，不要原样重发"
        }
        (_, FailureClass::Conflict) => "定位锚点与真实内容不符：先只读核对再改，连续两次即停",
        (_, FailureClass::NotFound) => "先确认目标是否存在，不要换路径重试",
        (_, FailureClass::Timeout) => "缩短操作或转后台再试，不要原样重发",
        (_, FailureClass::InvalidInput) => "对照工具 schema 补齐必填参数，不要凭记忆拼参数",
        (_, FailureClass::Permission) => "换可写位置或走审批，不要重试同一路径",
        (_, FailureClass::Config) => "先查明缺哪项配置，不要在缺配置时硬调",
        (_, FailureClass::Cancelled) => "被取消不重试，先确认这次操作是否还需要",
        (_, FailureClass::Other) => "先读错误原文再调整参数，不要原样重试",
    }
}

/// 规则摘要：**触发场景 → 动作**（+ 累计次数）。
///
/// 为什么不再是 `[规则] xxx 执行失败（累计 N 次）`：abstract 是语义检索的唯一依据，
/// 事件记录式的写法与用户提问的相似度天然低——2026-11-10 只读验证：活跃 65 条规则
/// 里 54 条（83%）**从未被检索命中**。同一批数据换成「场景 → 动作」的写法，才有被
/// 命中的可能；累计次数留在尾部供人工判断严重程度。
pub fn rule_abstract(tool: &str, class: FailureClass, occurrences: u32) -> String {
    format!(
        "{tool} {} → {}（累计 {occurrences} 次）",
        class.label(),
        action_hint(tool, class)
    )
}

/// 单条规则最多保留的「最近失败」记录行数。
///
/// 更早的发生只留在 `累计次数` 里——detail 不能随失败次数线性增长，否则这条规则
/// 迟早撑爆上下文（一次 `apply_patch` 的 detail 里带着完整补丁）。
const MAX_RECENT_OCCURRENCES: usize = 20;

/// 从既有 detail 里取累计次数（老格式或缺这一行按 1 次算）。
fn parse_occurrences(detail: &str) -> u32 {
    detail
        .lines()
        .find_map(|line| line.strip_prefix("累计次数: "))
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(1)
}

/// 从既有 detail 里取「## 最近失败」段的记录行（新的在前）。
fn parse_recent(detail: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in detail.lines() {
        if line.starts_with("## 最近失败") {
            inside = true;
            continue;
        }
        if inside {
            if line.starts_with("## ") {
                break;
            }
            if line.starts_with("- ") {
                out.push(line.to_string());
            }
        }
    }
    out
}

/// 生成一条压缩后的发生记录（参数与错误各截断，防止 detail 无限膨胀）。
fn record_line(ts: &str, session: &str, detail_text: &str) -> String {
    let param = detail_text
        .lines()
        .find_map(|line| line.strip_prefix("参数: "))
        .unwrap_or("");
    let err = detail_text
        .lines()
        .find_map(|line| line.strip_prefix("错误: "))
        .unwrap_or("");
    format!(
        "- {ts} [{session}] 参数={} 错误={}",
        crate::common::truncate::truncate_utf8_boundary(param, 240),
        crate::common::truncate::truncate_utf8_boundary(err, 360)
    )
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

        /// 第一个被创建的条目的 detail（测试断言用）。
        fn detail_of_first(&self) -> String {
            let Some(uri) = self.created_files.lock().unwrap().first().cloned() else {
                return String::new();
            };
            self.content
                .lock()
                .unwrap()
                .get(&(uri, ContentLevel::Detail))
                .cloned()
                .unwrap_or_default()
        }

        /// 第一个被创建的条目的 abstract。
        fn abstract_of_first(&self) -> String {
            let Some(uri) = self.created_files.lock().unwrap().first().cloned() else {
                return String::new();
            };
            self.content
                .lock()
                .unwrap()
                .get(&(uri, ContentLevel::Abstract))
                .cloned()
                .unwrap_or_default()
        }

        /// 模拟 `SummaryTask` 用 LLM 重写 L0 摘要（改写后原文措辞不复存在）。
        fn overwrite_abstract(&self, text: &str) {
            let Some(uri) = self.created_files.lock().unwrap().first().cloned() else {
                return;
            };
            self.content
                .lock()
                .unwrap()
                .insert((uri, ContentLevel::Abstract), text.to_string());
        }

        /// 测试用便捷主题（`execute_command` / `Other`）。
        fn topic() -> RuleTopic {
            RuleTopic::new("execute_command", FailureClass::Other)
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
                .ok_or_else(|| TianyanError::not_found(uri))
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
            .record_with_kind(
                &SpyVfs::topic(),
                "summary",
                "detail",
                "s1",
                FailureKind::Transient,
            )
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
            .record_with_kind(
                &SpyVfs::topic(),
                "summary",
                "detail",
                "s1",
                FailureKind::Input,
            )
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
                &SpyVfs::topic(),
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

    // ── 分类：FailureClass 只映射语义谓词（不在调用方做字符串匹配）────

    #[test]
    fn test_failure_class_from_semantic_predicates() {
        assert_eq!(
            FailureClass::of(&TianyanError::permission("x")),
            FailureClass::Permission
        );
        assert_eq!(
            FailureClass::of(&TianyanError::not_found("x")),
            FailureClass::NotFound
        );
        assert_eq!(
            FailureClass::of(&TianyanError::conflict("x")),
            FailureClass::Conflict
        );
        assert_eq!(
            FailureClass::of(&TianyanError::invalid_input("x")),
            FailureClass::InvalidInput
        );
        assert_eq!(
            FailureClass::of(&TianyanError::timeout("x")),
            FailureClass::Timeout
        );
        assert_eq!(
            FailureClass::of(&TianyanError::cancelled("x")),
            FailureClass::Cancelled
        );
        assert_eq!(
            FailureClass::of(&TianyanError::config("x")),
            FailureClass::Config
        );
        // 外层包装未携带语义前缀（executor 的通用包装）→ Other
        assert_eq!(
            FailureClass::of(&TianyanError::Custom(
                "tool: 执行失败：executor: 命令执行失败：目录名称无效。 (os error 267)".into()
            )),
            FailureClass::Other
        );
    }

    #[test]
    fn test_topic_prefix_is_uri_safe() {
        let topic = RuleTopic::new("Apply_Patch-2", FailureClass::Conflict);
        assert_eq!(topic.tool(), "apply_patch_2");
        assert_eq!(topic.file_prefix(), "rule-apply_patch_2-conflict-");
    }

    // ── 聚合：同主题只保留一条，计数累加 ─────────────────────────────

    #[tokio::test]
    async fn test_same_topic_merges_into_one_entry() {
        let vfs = Arc::new(SpyVfs::new());
        let recorder = RuleRecorder::new(vfs.clone());

        recorder
            .record_with_kind(
                &SpyVfs::topic(),
                "write_file 工具执行失败",
                "工具: write_file\n参数: first-args\n错误: 拒绝访问（os error 5）\n",
                "s1",
                FailureKind::Logic,
            )
            .await
            .unwrap();
        assert_eq!(vfs.created_count(), 1);

        recorder
            .record_with_kind(
                &SpyVfs::topic(),
                "write_file 工具执行失败",
                "工具: write_file\n参数: second-args\n错误: 拒绝访问（os error 5）\n",
                "s2",
                FailureKind::Logic,
            )
            .await
            .unwrap();

        assert_eq!(vfs.created_count(), 1, "同主题不应新建条目");
        let detail = vfs.detail_of_first();
        assert!(detail.contains("累计次数: 2"), "计数应累加：{detail}");
        assert!(detail.contains("[s2]"), "应追加最近一次来源会话：{detail}");
        assert!(
            detail.contains("first-args"),
            "首次失败详情必须保留（否则合并会丢信息）：{detail}"
        );
        assert!(
            vfs.abstract_of_first().contains("累计 2 次"),
            "abstract 应反映累计次数（供人阅读；计数权威在 detail）"
        );
    }

    /// **判别力测试**：`abstract` 被 `SummaryTask` 改写后，同主题仍必须聚合。
    ///
    /// 旧实现以 abstract 文本相似度判重——摘要任务改写后（实测库里 100% 被改写）
    /// 判据失配，同一失败会再次新建条目。本用例在旧实现下必红，是本修复的判别依据。
    #[tokio::test]
    async fn test_merge_survives_abstract_rewrite() {
        let vfs = Arc::new(SpyVfs::new());
        let recorder = RuleRecorder::new(vfs.clone());

        recorder
            .record_with_kind(
                &SpyVfs::topic(),
                "write_file 工具执行失败",
                "工具: write_file\n参数: a\n错误: 拒绝访问（os error 5）\n",
                "s1",
                FailureKind::Logic,
            )
            .await
            .unwrap();

        // 模拟 SummaryTask：L0 被 LLM 重写成叙述长文，原文措辞不复存在
        vfs.overwrite_abstract(
            "write_file 向临时目录写入脚本时连续报「拒绝访问 os error 5」，属逻辑类错误。",
        );

        recorder
            .record_with_kind(
                &SpyVfs::topic(),
                "write_file 工具执行失败",
                "工具: write_file\n参数: b\n错误: 拒绝访问（os error 5）\n",
                "s2",
                FailureKind::Logic,
            )
            .await
            .unwrap();

        assert_eq!(
            vfs.created_count(),
            1,
            "abstract 被改写后仍应按主题聚合（旧实现此处会新建第 2 条）"
        );
        assert!(
            vfs.detail_of_first().contains("累计次数: 2"),
            "改写不影响计数（计数权威在 detail）"
        );
    }

    // ── 不同主题各自独立 ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_same_tool_different_class_creates_separate_entry() {
        let vfs = Arc::new(SpyVfs::new());
        let recorder = RuleRecorder::new(vfs.clone());

        recorder
            .record_with_kind(
                &RuleTopic::new("execute_command", FailureClass::Timeout),
                "executor 命令执行失败",
                "工具: execute_command\n参数: a\n错误: 操作超时：executor: 执行超时\n",
                "s1",
                FailureKind::Logic,
            )
            .await
            .unwrap();

        recorder
            .record_with_kind(
                &RuleTopic::new("execute_command", FailureClass::Conflict),
                "executor 命令执行失败",
                "工具: execute_command\n参数: b\n错误: 冲突：executor: …\n",
                "s2",
                FailureKind::Logic,
            )
            .await
            .unwrap();

        assert_eq!(
            vfs.created_count(),
            2,
            "同工具不同失败类别是不同主题，应各成一条"
        );
    }

    #[tokio::test]
    async fn test_different_tool_creates_separate_entry() {
        let vfs = Arc::new(SpyVfs::new());
        let recorder = RuleRecorder::new(vfs.clone());

        recorder
            .record_with_kind(
                &RuleTopic::new("write_file", FailureClass::Other),
                "write_file 工具执行失败",
                "工具: write_file\n参数: a\n错误: 拒绝访问\n",
                "s1",
                FailureKind::Logic,
            )
            .await
            .unwrap();

        recorder
            .record_with_kind(
                &RuleTopic::new("read_file", FailureClass::Other),
                "read_file 工具执行失败",
                "工具: read_file\n参数: b\n错误: 拒绝访问\n",
                "s2",
                FailureKind::Logic,
            )
            .await
            .unwrap();

        assert_eq!(vfs.created_count(), 2, "不同工具是不同的主题，应各成一条");
    }

    // ── abstract 形态：场景 → 动作（检索可命中性） ──────────────────────
    //
    // 背景（2026-11-10 只读验证）：活跃 65 条规则里 54 条（83%）从未被检索命中——
    // abstract 写成「[规则] xxx 执行失败（累计 N 次）」这类事件记录，与用户提问的
    // 语义相似度天然低。以下用例把「场景 → 动作」的形态锁进写入侧。
    /// 摘要必须是「场景 → 动作」，不能退回事件记录式写法。
    ///
    /// 判别力：把 `rule_abstract` 改回 `[规则] {tool} {class}（累计 N 次）` → 本用例红。
    #[test]
    fn test_rule_abstract_is_scenario_to_action() {
        let text = rule_abstract("apply_patch", FailureClass::Conflict, 7);
        assert!(text.contains("apply_patch"), "要带工具名：{text}");
        assert!(text.contains('→'), "要写成「场景 → 动作」：{text}");
        assert!(text.contains("累计 7 次"), "计数保留在尾部：{text}");
        assert!(
            !text.starts_with("[规则]"),
            "不得用事件记录式前缀（检索命中率极低）：{text}"
        );
    }
    /// 不同工具 / 类别的动作不同；未知组合落通用动作（不能空）。
    #[test]
    fn test_action_hint_specializes_known_pairs() {
        let patch = action_hint("apply_patch", FailureClass::Conflict);
        let timeout = action_hint("execute_command", FailureClass::Timeout);
        let other = action_hint("some_new_tool", FailureClass::Other);
        assert_ne!(patch, timeout, "不同失败类别应给不同动作");
        assert!(
            patch.contains("锚点") || patch.contains("重写"),
            "补丁冲突要有具体动作：{patch}"
        );
        assert!(
            timeout.contains("background") || timeout.contains("缩短"),
            "超时要给具体处置：{timeout}"
        );
        assert!(!other.is_empty(), "未知组合必须有兜底动作");
    }
    /// 写入侧真的用了这个口径（首次写入与合并两条路径都要）。
    ///
    /// 判别力：只改纯函数、忘了替换写入模板时，本用例红。
    #[tokio::test]
    async fn test_recorded_abstract_uses_scenario_form() {
        let vfs = Arc::new(SpyVfs::new());
        let recorder = RuleRecorder::new(vfs.clone());

        recorder
            .record_with_kind(
                &RuleTopic::new("write_file", FailureClass::Permission),
                "write_file 工具执行失败",
                "工具: write_file\n参数: a\n错误: 拒绝访问（os error 5）\n",
                "s1",
                FailureKind::Logic,
            )
            .await
            .unwrap();

        let first = vfs.abstract_of_first();
        assert!(
            first.contains('→'),
            "首次写入的 abstract 应是「场景 → 动作」：{first}"
        );
        assert!(
            !first.starts_with("[规则]"),
            "不得带事件记录式前缀：{first}"
        );

        // 合并路径（同一主题第二次失败）也要保持同一口径
        recorder
            .record_with_kind(
                &RuleTopic::new("write_file", FailureClass::Permission),
                "write_file 工具执行失败",
                "工具: write_file\n参数: b\n错误: 拒绝访问（os error 5）\n",
                "s2",
                FailureKind::Logic,
            )
            .await
            .unwrap();
        let merged = vfs.abstract_of_first();
        assert!(
            merged.contains('→'),
            "合并后的 abstract 也应保持形态：{merged}"
        );
        assert!(merged.contains("累计 2 次"), "合并后计数在尾部：{merged}");
    }
}

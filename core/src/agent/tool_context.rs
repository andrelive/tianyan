//! 工具执行上下文（ADR-049 期 0.5.3）：工具依赖的**显式**载体。
//!
//! ## 为什么需要它
//!
//! 内置工具的执行方法此前都是 `impl ToolRegistry` 的方法，直接读注册表的
//! `pub(crate)` 字段。期 1 要把工具域（fs / code / symbol / repo_map /
//! knowledge / evolution）连同实现搬进独立 crate —— 跨 crate 后这些字段访问
//! 全部不可见，所以依赖必须先落在一个**公开结构**上。
//!
//! 实测边界很清楚：六个工具域只用到 10 个符号（`security_policy` 16 处、
//! `resolve_tool_path` 7 处、`ensure_approved` 4 处、`resolve_base_dir` 3 处、
//! `vfs` / `session_store` / `knowledge_ingestor` / `execution_log` /
//! `session_recall` / `repo_map_cache`），而 `agent_ops`（委托 / 任务面板）
//! 用到 32 个，其中含 `execute_parallel` / `child_registry` / `definitions` /
//! `spawn_background_delegate` —— 那是**内核能力**，不属于工具上下文。
//! 这个不对称正好就是本结构存在的位置：工具上下文只装「工具干活需要的外部世界」。
//!
//! ## 与 `ToolRegistry` 的关系
//!
//! `ToolRegistry` 持有一份 [`ToolContext`] 并 `Deref` 到它：注册表内部逻辑与
//! 全部既有调用点（`registry.resolve_tool_path(..)` 等）无需改动即可继续工作；
//! 而工具域共用的解析 / 审批方法写在 `impl ToolContext` 上，期 1 搬 crate 时
//! 整体迁走，新家只依赖本结构。
//!
//! **不是第二权威、不是新存储**：本结构只搬运 `Arc` 句柄与纯策略值，
//! 没有任何所有权或状态副本（`pending_approval_fingerprints` 与注册表共享同一 Arc）。

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::common::error::TianyanError;
use crate::executor::approval::{ApprovalDecision, ApprovalGate};
use crate::executor::{Action, SecurityPolicy};
use crate::knowledge::KnowledgeIngestor;
use crate::observability::execution_log::ExecutionLog;
use crate::session::search::SessionRecall;
use crate::session::store::SessionStore;
use crate::session::SessionManager;
use crate::vfs::VirtualFileSystem;

/// 工具执行上下文：工具域（fs / code / symbol / repo_map / knowledge /
/// evolution）需要的全部依赖与共用操作。
#[derive(Clone)]
pub struct ToolContext {
    /// 安全策略（路径沙箱规则 / 命令拦截 / 审批模式；ADR-033）。
    pub security_policy: SecurityPolicy,
    /// 会话权威存储（`vfs_read` 读 `tianyan://session/{id}` 的兼容层依赖）。
    pub session_store: Option<Arc<SessionStore>>,
    /// 会话管理器（相对路径按**会话绑定的工作目录**解析）。
    pub session_manager: Option<Arc<dyn SessionManager>>,
    /// VFS（search_vfs / vfs_read / vfs_list / knowledge_ingest 依赖）。
    pub vfs: Option<Arc<dyn VirtualFileSystem>>,
    /// 知识导入器（knowledge_ingest 依赖）。
    pub knowledge_ingestor: Option<Arc<KnowledgeIngestor>>,
    /// 执行记录日志（ADR-017 GEPA 数据层）。
    pub execution_log: Option<Arc<ExecutionLog>>,
    /// 会话回忆服务（ADR-017 决策 6：session_recall 依赖）。
    pub session_recall: Option<Arc<SessionRecall>>,
    /// 审批门控（write_file / apply_edit / apply_patch / execute_command
    /// 危险操作门控，统一经 [`Self::ensure_approved`]）。
    ///
    /// 具体实现已出 core（`plugins/policy-approval`，ADR-049 期 3-T1）：
    /// 这里只持契约对象，内核不认识审批策略。
    pub approval_gate: Option<Arc<dyn ApprovalGate>>,
    /// 待用户确认的操作指纹（审批拒绝 → 询问用户 → 用户回答后确认/放弃）。
    pub pending_approval_fingerprints: Arc<Mutex<Vec<String>>>,
    /// 数据目录（ADR-050：`view_image` 的冻结副本写到 `{data_dir}/session_images/`；
    /// 组装层按同一目录回读）。未注入 = 该能力不可用（工具报错，不 panic）。
    pub data_dir: Option<std::path::PathBuf>,
    /// 全局默认工作目录（`[agent] working_directory`）。
    ///
    /// 会话 header 未绑定或绑定目录已失效时，相对路径解析的兜底——与
    /// `Agent::default_working_directory` **同源**。不设此字段会让工具侧
    /// 静默回退进程 cwd（服务安装目录），`apply_patch` / `execute_command`
    /// 会落到意外位置（安全风险）。
    pub default_working_directory: Option<std::path::PathBuf>,
}

impl ToolContext {
    /// 以安全策略创建上下文（其余依赖缺省为 None，由装配层逐个注入）。
    pub fn new(security_policy: SecurityPolicy) -> Self {
        Self {
            security_policy,
            session_store: None,
            session_manager: None,
            vfs: None,
            knowledge_ingestor: None,
            execution_log: None,
            session_recall: None,
            approval_gate: None,
            pending_approval_fingerprints: Arc::new(Mutex::new(Vec::new())),
            data_dir: None,
            default_working_directory: None,
        }
    }

    /// 注入全局默认工作目录（`[agent] working_directory`）。
    ///
    /// **与 `Agent::default_working_directory` 同源**：Agent 侧回退到的目录与工具侧
    /// 相对路径解析的兜底必须是同一个值。两侧不一致时，模型会在 A 里看到文件、
    /// 写到 B 去（旧实现在「会话目录失效」时正是这样：Agent 退到配置的
    /// `F:\workspace`，工具退到**服务进程 cwd**）。
    pub fn with_default_working_directory(mut self, dir: Option<std::path::PathBuf>) -> Self {
        self.default_working_directory = dir;
        self
    }

    /// 会话生效的工作目录（**工具侧唯一取值口**）：会话绑定优先，其次全局默认。
    ///
    /// 两个候选都要过 `is_dir()`：绑定目录失效（被删 / 移走 / 未挂载）→ 用全局
    /// 默认；默认目录同样不存在 → `None`。**没有第三个选项**——进程 cwd 不是
    /// 兜底，它是服务安装目录，写操作落在那儿是安全事故而不是降级。
    ///
    /// 与 `Agent::resolve_working_directory` **同源**（同一份
    /// `[agent] working_directory` + 同一套「绑定目录必须真实存在」规则）：
    /// Agent 侧告诉模型「在 X 干活」时，工具侧解析相对路径也必须落到 X。
    async fn session_working_directory(&self, session_id: &str) -> Option<std::path::PathBuf> {
        // 全局默认目录也要存在（否则等于给了个假的兜底）
        let fallback = self
            .default_working_directory
            .as_deref()
            .filter(|p| p.is_dir());
        if let Some(sm) = &self.session_manager {
            if let Ok(Some(session)) = sm.get_session(session_id).await {
                if let Some(wd) = session.working_directory(fallback) {
                    return Some(wd);
                }
            }
        }
        fallback.map(std::path::Path::to_path_buf)
    }

    /// 解析工具文件路径：绝对路径原样（归一化）；相对路径基于**会话生效的工作目录**
    /// 解析。
    ///
    /// 输出统一做**词法归一化**（折叠 `.`/`..`）——判定（沙箱前缀匹配）
    /// 与执行（OS 解析）使用同一形态的路径，`sub/../..` 与符号链接都无法
    /// 制造两者口径差（T0-1）；对模型也更直观（`a/../b` 直接落 `b`）。
    ///
    /// 与 execute_command 默认 cwd 同一套归属规则：模型在会话工作区内工作时，
    /// `read_file ".git/HEAD"`、`glob path="."` 等相对路径落在工作区而非
    /// 进程 cwd（如天演仓库根）。
    ///
    /// # Errors
    /// 会话未绑定 / 绑定目录失效 **且** 没有可用的全局默认目录时，相对路径**无从
    /// 解析**：返回 `invalid_input`（消息含原路径，便于模型改正）。旧实现在这里是
    /// 「静默按进程 cwd 解析」——`apply_patch` / `write_file` 因此会写到服务安装
    /// 目录，而模型以为自己在工作区里干活（安全风险，不只是可用性问题）。
    pub async fn resolve_tool_path(
        &self,
        session_id: &str,
        path: &str,
    ) -> Result<String, TianyanError> {
        use crate::executor::security::lexical_normalize;

        let p = std::path::Path::new(path);
        if p.is_absolute() {
            return Ok(lexical_normalize(p).to_string_lossy().into_owned());
        }
        let Some(wd) = self.session_working_directory(session_id).await else {
            return Err(TianyanError::invalid_input(format!(
                "executor: 没有可用工作目录，无法解析相对路径 `{path}`。\
                 请为该会话绑定工作区，或改用绝对路径。"
            )));
        };
        let joined = wd.join(path);
        Ok(lexical_normalize(&joined).to_string_lossy().into_owned())
    }

    /// 解析会话生效的基准目录：会话绑定优先，其次全局默认。
    ///
    /// 与 [`Self::resolve_tool_path`] 同一套归属规则；供 apply_patch 基准目录、
    /// glob / grep 搜索根等需要「目录本身」（而非拼接路径）的场景使用。
    ///
    /// # Errors
    /// 没有任何可用工作目录（会话未绑定/失效 且 无全局默认）→ `invalid_input`。
    /// **不再回退 `std::env::current_dir()`**：进程 cwd 是服务安装目录，把
    /// `apply_patch` 的基准指到那里意味着「补丁写到了天演自己的源码树里」。
    pub async fn resolve_base_dir(
        &self,
        session_id: &str,
    ) -> Result<std::path::PathBuf, TianyanError> {
        self.session_working_directory(session_id)
            .await
            .ok_or_else(|| {
                TianyanError::invalid_input(
                    "executor: 没有可用工作目录，无法确定基准目录（apply_patch 基准、\
                     glob / grep 搜索根都依赖它）。请为该会话绑定工作区，或改用绝对路径。",
                )
            })
    }

    /// 记录被审批拒绝、待用户确认的操作指纹。
    ///
    /// 由"询问用户"降级链路调用（`ensure_approved` 拒绝分支）；消费方为
    /// `Agent` 轮末读取 [`Self::pending_approval_fingerprints`] 生成追问；
    /// 用户回答后经注册表的 `confirm_pending_approval` 决定是否放行。
    pub async fn remember_pending_approval(&self, action: &Action) {
        let fp = crate::executor::approval::action_fingerprint(action);
        self.pending_approval_fingerprints.lock().await.push(fp);
        tracing::info!(action = ?action, "操作已进入待用户确认队列");
    }

    /// 审批门控：请求审批并处理拒绝降级。
    ///
    /// 统一 4 个危险工具（write_file / apply_edit / apply_patch / execute_command）
    /// 的审批序列，消除各执行器内的逐字复制：
    /// 1. `approval_gate` 未装配（None）时直接放行；
    /// 2. 子任务（subagent）非交互审批——未授权操作立即拒绝（ADR-011）；
    /// 3. 拒绝 → 记录待确认指纹（用户经"询问用户"链路批准后放行）并返回
    ///    统一"操作需要用户确认"错误。
    ///
    /// 审批**策略**（风险定级 / 规则引擎 / 模式短路 / 挂起通道）在插件里
    /// （`plugins/policy-approval`）；本方法只负责「问一次 → 拒绝时记指纹 →
    /// 统一错误文案」这段核心侧编排。
    pub async fn ensure_approved(
        &self,
        session_id: &str,
        subagent: bool,
        action: &Action,
    ) -> Result<(), TianyanError> {
        let Some(ref approval) = self.approval_gate else {
            return Ok(());
        };
        // 子任务 = 不允许挂起等待人工（ADR-011）
        let approval_result = approval.authorize(session_id, action, !subagent).await;
        let resp = approval_result
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：审批工作流错误: {}", e)))?;
        if resp.decision != ApprovalDecision::Approve {
            self.remember_pending_approval(action).await;
            return Err(TianyanError::Custom(format!(
                "tool: 安全违规：操作需要用户确认：{}",
                resp.reason.unwrap_or_default()
            )));
        }
        Ok(())
    }

    /// 获取待用户确认的操作指纹快照。
    pub async fn pending_approval_fingerprints(&self) -> Vec<String> {
        self.pending_approval_fingerprints.lock().await.clone()
    }

    /// 当前会话生效的**聊天模型能力标签**（ADR-050 的图片能力门控）。
    ///
    /// 解析顺序与会话路由一致：`SessionHeader.model_selection`（会话级选择）
    /// 优先，缺失时回落 `models.preferences.chat`（全局默认）。
    ///
    /// 返回 `None` 的三种情形都**不门控**（调用方放行、让上游兜底）：未注入会话
    /// 管理器、配置槽位未初始化、该模型不在配置里（未知模型）。门控数据缺失时
    /// 不该凭空拦截用户——与「未装配不阻塞」同一档语义。
    pub async fn chat_model_capabilities(
        &self,
        session_id: &str,
    ) -> Option<(String, Vec<crate::config::ModelCapability>)> {
        let config = crate::config::try_get_config()?;
        let session_selection = match &self.session_manager {
            Some(manager) => manager
                .get_session(session_id)
                .await
                .ok()
                .flatten()
                .and_then(|session| session.header.model_selection),
            None => None,
        };
        let (provider, model) = match session_selection {
            Some(selection) => (Some(selection.provider), selection.model),
            None => {
                let pref = config.models.preferences.chat.as_ref()?;
                (Some(pref.provider.clone()), pref.model.clone())
            }
        };
        lookup_model_capabilities(config, provider.as_deref(), &model)
    }
}

/// 从配置里按 `provider + model` 查能力标签（ADR-050 门控的纯函数核心）。
///
/// `provider` 为 `None` 时跨 provider 找第一个同名模型（配置里同名模型可挂在
/// 多个 provider 上，此时按 provider 列表顺序取第一个——与模型路由的兜底同向）。
/// 查不到返回 `None`：未知模型属"数据缺失"，调用方放行、由上游兜底。
///
/// **为什么抽成纯函数**：门控数据源是配置里的可选标签，判错方向（误拦 vs 误放）
/// 直接影响用户能否看图——这段解析必须可单测，而不是靠手搓脚本读 TOML 推断
/// （2026-10-24 实错：人工解析把字母序靠前的 `capabilities` 归到了上一条模型）。
pub(crate) fn lookup_model_capabilities(
    config: &crate::config::TianyanConfig,
    provider: Option<&str>,
    model: &str,
) -> Option<(String, Vec<crate::config::ModelCapability>)> {
    let entry = config
        .models
        .providers
        .iter()
        .filter(|p| provider.is_none_or(|want| p.name == want))
        .flat_map(|p| p.models.iter())
        .find(|m| m.name == model)?;
    Some((model.to_string(), entry.capabilities.clone()))
}

#[cfg(test)]
mod capability_lookup_tests {
    use super::*;
    use crate::config::{ModelCapability, TianyanConfig};

    /// 用 TOML 构造配置（**逐字节模拟落盘形态**：字段按字母序，`capabilities`
    /// 在 `name` 之前——2026-10-24 正是这一点让手工解析读错）。
    fn config_with_models() -> TianyanConfig {
        toml::from_str(
            r#"
[models.preferences.chat]
model = "pref-model"
provider = "prov-a"

[[models.providers]]
endpoint = "http://localhost:11434/v1"
name = "prov-a"

[[models.providers.models]]
capabilities = ["chat", "vision"]
name = "vision-model"

[[models.providers.models]]
capabilities = ["chat"]
name = "text-model"

[[models.providers.models]]
name = "unlabeled-model"

[[models.providers]]
endpoint = "http://localhost:11435/v1"
name = "prov-b"

[[models.providers.models]]
capabilities = ["chat", "vision"]
name = "pref-model"
"#,
        )
        .expect("解析测试配置")
    }

    #[test]
    fn test_lookup_finds_vision_model_by_provider() {
        let config = config_with_models();
        let (model, caps) =
            lookup_model_capabilities(&config, Some("prov-a"), "vision-model").expect("应命中");
        assert_eq!(model, "vision-model");
        assert!(caps.contains(&ModelCapability::Vision));
    }

    #[test]
    fn test_lookup_text_only_model_has_no_vision() {
        let config = config_with_models();
        let (_, caps) =
            lookup_model_capabilities(&config, Some("prov-a"), "text-model").expect("应命中");
        assert!(
            !caps.contains(&ModelCapability::Vision),
            "只标 chat 的模型无 vision"
        );
    }

    #[test]
    fn test_lookup_unlabeled_model_is_empty_not_missing() {
        let config = config_with_models();
        let (_, caps) = lookup_model_capabilities(&config, Some("prov-a"), "unlabeled-model")
            .expect("未标能力的模型仍应命中（空数组，而非查不到）");
        assert!(caps.is_empty(), "capabilities 缺省 = 空数组");
    }

    #[test]
    fn test_lookup_respects_provider_scope() {
        let config = config_with_models();
        // prov-a 没有 pref-model，prov-b 有：限定 prov-a 时必须查不到
        assert!(lookup_model_capabilities(&config, Some("prov-a"), "pref-model").is_none());
        assert!(
            lookup_model_capabilities(&config, Some("prov-b"), "pref-model").is_some(),
            "prov-b 的 pref-model 存在"
        );
    }

    #[test]
    fn test_lookup_without_provider_uses_first_match() {
        let config = config_with_models();
        let (_, caps) =
            lookup_model_capabilities(&config, None, "pref-model").expect("跨 provider 命中");
        assert!(caps.contains(&ModelCapability::Vision));
    }

    #[test]
    fn test_lookup_unknown_model_returns_none() {
        let config = config_with_models();
        assert!(lookup_model_capabilities(&config, Some("prov-a"), "nope").is_none());
    }
}

/// 相对路径归属解析（L1/L2）：工作目录失效时**明确失败**，绝不静默落到进程 cwd。
///
/// 这组用例锁的是安全断言，不是"行为细节"：旧实现在「无任何工作目录」时按
/// **进程 cwd（服务安装目录）** 解析相对路径，于是 `apply_patch` / `write_file`
/// 会把改动写进天演的安装目录，而模型以为自己仍在工作区里干活。
#[cfg(test)]
mod path_resolution_tests {
    use super::*;
    use crate::common::types::Message;
    use crate::session::Session;
    use std::path::PathBuf;

    /// 固定返回同一个会话的会话管理器（`None` = 会话未找到）。
    struct FixedSessionManager {
        session: Option<Session>,
    }

    #[async_trait::async_trait]
    impl SessionManager for FixedSessionManager {
        async fn create_session(&self, id: &str, _message: Message) -> crate::Result<Session> {
            Ok(Session::new(id))
        }

        async fn get_session(&self, _id: &str) -> crate::Result<Option<Session>> {
            Ok(self.session.clone())
        }

        async fn list_sessions(&self) -> crate::Result<Vec<Session>> {
            Ok(Vec::new())
        }
    }

    /// 构造**未装配会话管理器**的上下文（`default` = 全局默认目录）。
    fn ctx(default: Option<&std::path::Path>) -> ToolContext {
        ToolContext::new(SecurityPolicy::default())
            .with_default_working_directory(default.map(PathBuf::from))
    }

    /// 装配会话管理器：`bound` = 会话 header 绑定的目录，`default` = 全局默认目录。
    fn ctx_with_session(
        bound: Option<&std::path::Path>,
        default: Option<&std::path::Path>,
    ) -> ToolContext {
        let mut session = Session::new("s1");
        session.header.working_directory = bound.map(|p| p.to_string_lossy().into_owned());
        let mut c = ctx(default);
        c.session_manager = Some(Arc::new(FixedSessionManager {
            session: Some(session),
        }));
        c
    }

    /// **最关键的断言**：无任何工作目录 + 相对路径 → `Err`，且消息含原路径。
    ///
    /// 判别力：若实现仍静默按进程 cwd 解析，本用例会拿到
    /// `Ok(<安装目录>/src/main.rs)`，随后 `apply_patch` 就会往安装目录里写——
    /// 正是要修的安全问题。
    #[tokio::test]
    async fn test_no_working_directory_relative_path_fails_with_path_in_message() {
        let c = ctx(None);
        let err = c
            .resolve_tool_path("s1", "src/../src/main.rs")
            .await
            .expect_err("无工作目录时相对路径必须失败，不得回退进程 cwd");
        assert!(
            err.is_invalid_input(),
            "必须是 invalid_input 语义（调用方可判类）：{err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("src/../src/main.rs"),
            "错误消息必须含原路径（模型据此改正）：{msg}"
        );
        assert!(
            msg.contains("没有可用工作目录"),
            "错误消息须点明原因与动作：{msg}"
        );
    }

    /// 无工作目录 + **绝对**路径 → `Ok`（绝对路径不依赖任何基准）。
    #[tokio::test]
    async fn test_no_working_directory_absolute_path_is_ok() {
        let c = ctx(None);
        let abs = if cfg!(windows) {
            "C:\\data\\a.txt"
        } else {
            "/data/a.txt"
        };
        let resolved = c
            .resolve_tool_path("s1", abs)
            .await
            .expect("绝对路径不需要工作目录");
        assert_eq!(resolved, abs);
    }

    /// 会话绑定失效 + 有默认目录 → 解析到**默认目录**（不是进程 cwd）。
    ///
    /// 判别力：这是「两侧一致」的核心断言——Agent 侧（环境段）回退到默认目录，
    /// 工具侧必须落到同一个目录，否则模型写 A、文件进 B。
    #[tokio::test]
    async fn test_bound_invalid_falls_back_to_default_not_process_cwd() {
        let gone = tempfile::tempdir().unwrap();
        let gone_path = gone.path().to_path_buf();
        let default = tempfile::tempdir().unwrap();
        drop(gone); // 绑定目录被删

        let c = ctx_with_session(Some(&gone_path), Some(default.path()));
        let resolved = c
            .resolve_tool_path("s1", "src/main.rs")
            .await
            .expect("有默认目录时不应失败");
        assert_eq!(
            PathBuf::from(&resolved),
            default.path().join("src/main.rs"),
            "必须解析到全局默认目录"
        );
        assert_ne!(
            PathBuf::from(&resolved),
            std::env::current_dir().unwrap().join("src/main.rs"),
            "绝不能落到进程 cwd（服务安装目录）"
        );
    }

    /// 会话绑定有效 → 用绑定目录（默认目录不抢）。
    #[tokio::test]
    async fn test_bound_valid_wins_over_default() {
        let bound = tempfile::tempdir().unwrap();
        let default = tempfile::tempdir().unwrap();
        let c = ctx_with_session(Some(bound.path()), Some(default.path()));
        let resolved = c.resolve_tool_path("s1", "./a.txt").await.unwrap();
        assert_eq!(PathBuf::from(resolved), bound.path().join("a.txt"));
    }

    /// 会话未绑定 + 有默认 → 用默认目录（不报错）。
    #[tokio::test]
    async fn test_unbound_with_default_resolves() {
        let default = tempfile::tempdir().unwrap();
        let c = ctx_with_session(None, Some(default.path()));
        let resolved = c.resolve_tool_path("s1", "a.txt").await.unwrap();
        assert_eq!(PathBuf::from(resolved), default.path().join("a.txt"));
    }

    /// 绑定失效 + 默认目录**也不存在** → `Err`（默认目录不做假兜底）。
    #[tokio::test]
    async fn test_missing_default_is_not_a_fallback() {
        let gone = tempfile::tempdir().unwrap();
        let gone_path = gone.path().to_path_buf();
        let default = tempfile::tempdir().unwrap();
        let default_path = default.path().to_path_buf();
        drop(gone);
        drop(default);

        let c = ctx_with_session(Some(&gone_path), Some(&default_path));
        let err = c
            .resolve_tool_path("s1", "a.txt")
            .await
            .expect_err("两个候选目录都不存在时必须失败");
        assert!(err.is_invalid_input(), "{err}");
    }

    /// `resolve_base_dir`（apply_patch 基准 / glob、grep 搜索根）：无目录 → `Err`，
    /// 不再回退 `std::env::current_dir()`。
    #[tokio::test]
    async fn test_resolve_base_dir_never_falls_back_to_process_cwd() {
        let c = ctx(None);
        let err = c
            .resolve_base_dir("s1")
            .await
            .expect_err("无工作目录时基准目录必须失败，不得回退进程 cwd");
        assert!(err.is_invalid_input(), "{err}");
        let msg = err.to_string();
        assert!(msg.contains("没有可用工作目录"), "{msg}");
        assert!(
            msg.contains("apply_patch"),
            "错误消息须点明受影响的操作，便于模型改绝对路径：{msg}"
        );
    }

    /// `resolve_base_dir` 绑定有效时返回绑定目录；未绑定时返回默认目录。
    #[tokio::test]
    async fn test_resolve_base_dir_prefers_bound_then_default() {
        let bound = tempfile::tempdir().unwrap();
        let default = tempfile::tempdir().unwrap();
        let c = ctx_with_session(Some(bound.path()), Some(default.path()));
        assert_eq!(c.resolve_base_dir("s1").await.unwrap(), bound.path());

        let c = ctx_with_session(None, Some(default.path()));
        assert_eq!(c.resolve_base_dir("s1").await.unwrap(), default.path());
    }
}

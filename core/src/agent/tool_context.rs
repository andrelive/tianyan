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
        }
    }

    /// 解析工具文件路径：相对路径（非绝对）基于**会话绑定的工作目录**解析
    /// （缺省回退进程 cwd 语义，原样返回）；绝对路径原样解析。
    ///
    /// 输出统一做**词法归一化**（折叠 `.`/`..`）——判定（沙箱前缀匹配）
    /// 与执行（OS 解析）使用同一形态的路径，`sub/../..` 与符号链接都无法
    /// 制造两者口径差（T0-1）；对模型也更直观（`a/../b` 直接落 `b`）。
    ///
    /// 与 execute_command 默认 cwd 同一套归属规则：模型在会话工作区内工作时，
    /// `read_file ".git/HEAD"`、`glob path="."` 等相对路径落在工作区而非
    /// 进程 cwd（如天演仓库根）。
    pub async fn resolve_tool_path(&self, session_id: &str, path: &str) -> String {
        use crate::executor::security::lexical_normalize;

        let p = std::path::Path::new(path);
        if p.is_absolute() {
            return lexical_normalize(p).to_string_lossy().into_owned();
        }
        if let Some(sm) = &self.session_manager {
            if let Ok(Some(session)) = sm.get_session(session_id).await {
                if let Some(wd) = session.working_directory(None) {
                    let joined = wd.join(path);
                    return lexical_normalize(&joined).to_string_lossy().into_owned();
                }
            }
        }
        // 无会话绑定：保留相对语义（进程 cwd），同样折叠 `.`/`..`
        lexical_normalize(p).to_string_lossy().into_owned()
    }

    /// 解析会话生效的基准目录：会话绑定的工作目录优先，缺省回退进程 cwd。
    ///
    /// 与 [`Self::resolve_tool_path`] 同一套归属规则；供 apply_patch 基准目录、
    /// glob 搜索根等需要「目录本身」（而非拼接路径）的场景使用。
    pub async fn resolve_base_dir(
        &self,
        session_id: &str,
    ) -> Result<std::path::PathBuf, TianyanError> {
        if let Some(sm) = &self.session_manager {
            if let Ok(Some(session)) = sm.get_session(session_id).await {
                if let Some(wd) = session.working_directory(None) {
                    return Ok(wd);
                }
            }
        }
        std::env::current_dir()
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：无法获取工作目录: {e}")))
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
}

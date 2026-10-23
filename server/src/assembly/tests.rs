//! 插槽测试（原 `assembly.rs` 的 `mod tests`，ADR-049 期 4b 随目录化整体搬入）。
//!
//! 构造点用**组语法**：`AssemblyServices { storage: StorageAssemblies::default()
//! .with_vector_factory(f), ..Default::default() }`——断言语义与分组前逐字一致
//! （判别力不弱化：每个用例仍是「装配了就一定**只用**工厂 / 未装配就一定是降级」）。

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use tianyan::config::StorageConfig;
use tianyan::executor::approval::ApprovalGate;
use tianyan::vfs::{ContextEntry, StorageBackend, VectorPoint, VectorStorage};

use super::storage::NoopVectorStorage;
use super::*;

/// 未装配工厂时降级为空向量存储：VFS 不会因此构建失败，但检索恒空。
#[tokio::test]
async fn test_default_assembly_yields_noop_vector_storage() {
    let config = StorageConfig {
        vector: tianyan::config::VectorStorageConfig {
            collection_name: "noop_test".to_string(),
            vector_dimension: 8,
        },
        ..Default::default()
    };
    let storage = AssemblyServices::new()
        .storage()
        .vector(&config)
        .await
        .unwrap();
    assert_eq!(storage.embedding_dim(), 8);
    assert_eq!(storage.count_rows().await.unwrap(), 0);
    assert!(storage.list_point_ids().await.unwrap().is_empty());
}

/// 装配了工厂时必须**只用工厂**（降级路径不得抢占）。
#[tokio::test]
async fn test_assembly_uses_injected_vector_factory() {
    let factory: VectorStorageFactory = Arc::new(|config| {
        let dim = config.vector.vector_dimension;
        Box::pin(async move {
            let storage: Arc<dyn VectorStorage> = Arc::new(NoopVectorStorage {
                embedding_dim: dim + 1,
                upsert_warned: AtomicBool::new(false),
            });
            Ok(storage)
        })
    });
    let config = StorageConfig {
        vector: tianyan::config::VectorStorageConfig {
            collection_name: "injected".to_string(),
            vector_dimension: 8,
        },
        ..Default::default()
    };
    let assembly = AssemblyServices {
        storage: StorageAssemblies::default().with_vector_factory(factory),
        ..Default::default()
    };
    let storage = assembly.storage().vector(&config).await.unwrap();
    assert_eq!(
        storage.embedding_dim(),
        9,
        "应使用注入的工厂（维度 +1 是工厂内的判别标记）"
    );
}

/// 工厂必须可重复调用（启动入口会被数据目录搬迁后的重启复用）。
#[tokio::test]
async fn test_vector_factory_is_repeatable() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    let factory: VectorStorageFactory = Arc::new(move |_config| {
        let counter = counter.clone();
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            let storage: Arc<dyn VectorStorage> = Arc::new(NoopVectorStorage {
                embedding_dim: 4,
                upsert_warned: AtomicBool::new(false),
            });
            Ok(storage)
        })
    });
    let assembly = AssemblyServices {
        storage: StorageAssemblies::default().with_vector_factory(factory),
        ..Default::default()
    };
    let config = StorageConfig::default();
    let _ = assembly.storage().vector(&config).await.unwrap();
    let _ = assembly.storage().vector(&config).await.unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "工厂应可重复调用（不是 FnOnce）"
    );
}

/// 空向量存储必须在**首次写入**时留下丢弃证据。
///
/// 判别力：若删掉 `upsert_point` 里的 `swap`，生产误配的唯一表现就是「检索恒空」，
/// 日志无任何线索（ADR-038 可见失败）；本测试锁住「写入路径有痕迹」。
#[tokio::test]
async fn test_noop_vector_storage_warns_on_first_upsert_only() {
    let config = StorageConfig::default();
    let storage = NoopVectorStorage::new(&config);
    let point = VectorPoint {
        schema_version: 1,
        id: "p1".to_string(),
        abstract_vector: Some(vec![0.0; 4]),
        overview_vector: None,
        visual_vector: None,
        payload: tianyan::common::types::EntryMetadata::default(),
    };
    // 丢弃不等于失败：写入必须仍然 Ok，否则会把「降级」变成「报错」。
    for _ in 0..3 {
        storage.upsert_point(&point).await.expect("丢弃写入不报错");
    }
    assert!(
        storage.upsert_warned.load(Ordering::Relaxed),
        "首次写入后须置起告警标志（证据链）；随后不再重复打日志（热路径不刷屏）"
    );
    assert_eq!(storage.count_rows().await.expect("计数"), 0);
}

// ── 结构化存储插槽（ADR-049 期 2 序 4）──────────────────────────────

/// 未装配工厂时给的是**可用但不落盘**的内存后端：VFS 因此仍能读写（不 panic、
/// 不阻断启动），但必须留下「未落盘」的证据。
///
/// 判别力：若把兜底改成 `Err`，本测试的 `backend` 直接失败；若把
/// `warn_write_once` 删了，`write_warned` 断言必红（生产误配将完全无痕）。
#[tokio::test]
async fn test_default_assembly_storage_is_ephemeral_but_functional() {
    let config = StorageConfig::default();
    let database = tianyan::db::Database::open_in_memory().expect("内存库");
    let storage = AssemblyServices::new()
        .storage()
        .backend(&config, database)
        .expect("未装配时应给内存兜底而不是报错");

    let uri = tianyan::TianyanUri::new(
        tianyan::ContextNamespace::Knowledge,
        vec!["probe.md".to_string()],
    );
    storage
        .write_content(&uri, tianyan::ContentLevel::Detail, "临时内容")
        .await
        .expect("兜底后端应可写入");
    assert_eq!(
        storage
            .read_content(&uri, tianyan::ContentLevel::Detail)
            .await
            .expect("回读"),
        "临时内容"
    );
    assert!(storage.exists(&uri).await.expect("存在性"));

    // 首次写入必须留下「未落盘」证据（直接构造以便检查私有标志）
    let direct = EphemeralStorageBackend::new();
    assert!(!direct.write_warned.load(Ordering::Relaxed));
    direct
        .write_content(&uri, tianyan::ContentLevel::Detail, "x")
        .await
        .expect("写入");
    assert!(
        direct.write_warned.load(Ordering::Relaxed),
        "首次写入后须置起告警标志（不静默丢数据）"
    );
}

/// 装配了工厂时**只用工厂**（兜底不得抢占）。
///
/// 判别力：工厂返回的后端里预置了一个条目，返回值若能读到它就证明走的是工厂。
#[tokio::test]
async fn test_assembly_uses_injected_storage_factory() {
    let marker = tianyan::TianyanUri::new(
        tianyan::ContextNamespace::Knowledge,
        vec!["injected.md".to_string()],
    );
    let injected = marker.clone();
    let factory: StorageBackendFactory = Arc::new(move |_config, _database| {
        let backend = EphemeralStorageBackend::new();
        // 同步工厂：先写进一个预置条目（无需 async 上下文）
        let entry = ContextEntry::new_file(injected.clone());
        backend
            .entries
            .try_write()
            .expect("未竞争")
            .insert(entry.uri().to_string(), entry);
        Ok(Arc::new(backend) as Arc<dyn StorageBackend>)
    });
    let config = StorageConfig::default();
    let database = tianyan::db::Database::open_in_memory().expect("内存库");
    let assembly = AssemblyServices {
        storage: StorageAssemblies::default().with_storage_factory(factory),
        ..Default::default()
    };
    let storage = assembly
        .storage()
        .backend(&config, database)
        .expect("装配后应成功");
    assert!(
        storage.exists(&marker).await.expect("存在性"),
        "应使用注入的工厂（预置条目是工厂内的判别标记）"
    );
}

/// 工厂必须可重复调用（数据目录搬迁后的重启会再次装配）。
#[tokio::test]
async fn test_storage_factory_is_repeatable() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    let factory: StorageBackendFactory = Arc::new(move |_config, _database| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(EphemeralStorageBackend::new()) as Arc<dyn StorageBackend>)
    });
    let assembly = AssemblyServices {
        storage: StorageAssemblies::default().with_storage_factory(factory),
        ..Default::default()
    };
    let config = StorageConfig::default();
    let database = tianyan::db::Database::open_in_memory().expect("内存库");
    let _ = assembly
        .storage()
        .backend(&config, database.clone())
        .unwrap();
    let _ = assembly.storage().backend(&config, database).unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "工厂应可重复调用（不是 FnOnce）"
    );
}

/// 内存兜底的条目语义必须与 `SqliteBackend` 同口径（否则兜底会引入只属于测试的
/// 行为差异，重演「Mock 形态掩盖缺陷」）：删除按「URI 后接斜杠」前缀级联、
/// 不误删同前缀兄弟；列举只给一级子条目。
#[tokio::test]
async fn test_ephemeral_storage_prefix_semantics_match_sqlite() {
    let backend = EphemeralStorageBackend::new();
    let ns = tianyan::ContextNamespace::Knowledge;
    let parent = tianyan::TianyanUri::new(ns, vec!["ab".to_string()]);
    let sibling = tianyan::TianyanUri::new(ns, vec!["abc".to_string()]);
    let child = tianyan::TianyanUri::new(ns, vec!["ab".to_string(), "x.md".to_string()]);
    let grandchild = tianyan::TianyanUri::new(
        ns,
        vec!["ab".to_string(), "deep".to_string(), "y.md".to_string()],
    );
    for uri in [&parent, &sibling, &child, &grandchild] {
        backend
            .write_entry(&ContextEntry::new_file(uri.clone()))
            .await
            .expect("写入");
    }

    // 列举只给一级子条目（孙条目被过滤）
    let listed = backend.list_directory(&parent).await.expect("列举");
    assert_eq!(listed.len(), 1, "只应列出一级子条目");

    backend.delete_entry(&parent).await.expect("删除");
    assert!(!backend.exists(&parent).await.expect("存在性"));
    assert!(
        !backend.exists(&child).await.expect("存在性"),
        "子条目应被级联删除"
    );
    assert!(
        !backend.exists(&grandchild).await.expect("存在性"),
        "孙条目应被级联删除"
    );
    assert!(
        backend.exists(&sibling).await.expect("存在性"),
        "同前缀兄弟条目（ab vs abc）不应被误删"
    );
}

// ── 内置任务插槽（ADR-049 期 2 序 5a）────────────────────────────────

/// 只用于装配测试的聊天服务桩（本库的测试不发起任何真实请求）。
struct StubChatService;

#[async_trait]
impl tianyan::model::ChatService for StubChatService {
    async fn chat_completion(
        &self,
        _request: tianyan::model::types::ChatCompletionRequest,
    ) -> tianyan::Result<tianyan::model::types::ChatCompletionResponse> {
        Err(tianyan::TianyanError::Custom(
            "测试桩：不发起聊天请求".to_string(),
        ))
    }

    async fn chat_completion_stream(
        &self,
        _request: tianyan::model::types::ChatCompletionRequest,
    ) -> tianyan::Result<
        tokio::sync::mpsc::Receiver<tianyan::Result<tianyan::model::types::ChatCompletionChunk>>,
    > {
        Err(tianyan::TianyanError::Custom(
            "测试桩：不发起流式聊天请求".to_string(),
        ))
    }
}

/// 只用于装配测试的会话管理器桩（任务清单判定不读会话）。
struct StubSessionManager;

#[async_trait]
impl tianyan::session::SessionManager for StubSessionManager {
    async fn create_session(
        &self,
        id: &str,
        _message: tianyan::Message,
    ) -> tianyan::Result<tianyan::session::Session> {
        Ok(tianyan::session::Session::new(id))
    }

    async fn get_session(&self, _id: &str) -> tianyan::Result<Option<tianyan::session::Session>> {
        Ok(None)
    }

    async fn list_sessions(&self) -> tianyan::Result<Vec<tianyan::session::Session>> {
        Ok(Vec::new())
    }
}

/// 只用于装配测试的任务处理器（自报名字由工厂给定）。
struct StubTask(&'static str);

#[async_trait]
impl tianyan::scheduler::TaskHandler for StubTask {
    async fn execute(
        &self,
        _ctx: &tianyan::scheduler::TaskContext,
    ) -> tianyan::scheduler::TaskResult {
        tianyan::scheduler::TaskResult::success(0)
    }

    fn name(&self) -> &str {
        self.0
    }
}

/// 构造任务装配能力集合（`snapshot_manager` 固定 `None`：本库的插槽测试不关心它；
/// `evolution_executor` 同理，清单判定只用到它是否为 `Some`）。
async fn make_task_services() -> TaskServices {
    let database = tianyan::db::Database::open_in_memory().expect("内存库");
    database.init_schemas().await.expect("建表");
    let usage_stats = tianyan::observability::usage_stats::UsageStats::new(database).expect("统计");
    TaskServices {
        config: tianyan::config::TianyanConfig::default(),
        usage_stats,
        snapshot_manager: None,
        session_manager: Arc::new(StubSessionManager),
        notification_sink: Arc::new(tianyan::notification::NoopNotificationSink),
        chat: Arc::new(StubChatService),
        chat_model: "stub-model".to_string(),
        working_sets: None,
        evolution_executor: None,
    }
}

/// 未装配工厂时**不注册任何任务**（空清单），且不 panic、不报错。
///
/// 判别力：若把降级改成 `Err`，`start_server*` 这组不带装配的公开入口在所有构建里
/// 都起不来（序 3/4 的同类取舍）；若改成「兜底注册一份内置清单」，本库就得知道
/// 具体任务（D2-3 破）。
#[tokio::test]
async fn test_default_assembly_yields_no_tasks() {
    let services = make_task_services().await;
    let registrations = AssemblyServices::new().manifests().tasks(&services);
    assert!(
        registrations.is_empty(),
        "未装配任务工厂时应返回空清单（不注册内置任务），实际 {} 条",
        registrations.len()
    );
}

/// 装配了工厂时**只用工厂**（空清单不得抢占）。
#[tokio::test]
async fn test_assembly_uses_injected_task_factory() {
    let factory: TaskFactory = Arc::new(|_services| {
        vec![TaskRegistration::new(
            "injected_task",
            "注入的测试任务",
            42,
            Arc::new(StubTask("injected_task")),
        )]
    });
    let services = make_task_services().await;
    let assembly = AssemblyServices {
        manifests: ManifestAssemblies::default().with_task_factory(factory),
        ..Default::default()
    };
    let registrations = assembly.manifests().tasks(&services);

    assert_eq!(registrations.len(), 1, "应返回工厂给的清单");
    assert_eq!(registrations[0].name, "injected_task");
    assert_eq!(registrations[0].display_name, "注入的测试任务");
    assert_eq!(registrations[0].interval_secs, 42);
    assert_eq!(registrations[0].handler.name(), "injected_task");
}

/// 工厂必须可重复调用（数据目录搬迁后的重启会再次装配）。
#[tokio::test]
async fn test_task_factory_is_repeatable() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    let factory: TaskFactory = Arc::new(move |_services| {
        counter.fetch_add(1, Ordering::SeqCst);
        Vec::new()
    });
    let assembly = AssemblyServices {
        manifests: ManifestAssemblies::default().with_task_factory(factory),
        ..Default::default()
    };
    let services = make_task_services().await;
    let _ = assembly.manifests().tasks(&services);
    let _ = assembly.manifests().tasks(&services);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "工厂应可重复调用（不是 FnOnce）"
    );
}

// ── 审批门控插槽（ADR-049 期 3-T1）──────────────────────────────────

/// 只用于装配测试的审批门控桩（本库的插槽测试只关心「工厂是否被使用」）。
struct StubApprovalGate {
    /// 判别标记：装配层注入的是哪个实例（与「未装配」区分）。
    marker: &'static str,
}

#[async_trait]
impl ApprovalGate for StubApprovalGate {
    async fn authorize(
        &self,
        _session_id: &str,
        _action: &tianyan::executor::Action,
        _allow_human_wait: bool,
    ) -> tianyan::Result<tianyan::executor::approval::ApprovalResponse> {
        Ok(tianyan::executor::approval::ApprovalResponse {
            request_id: self.marker.to_string(),
            decision: tianyan::executor::approval::ApprovalDecision::Deny,
            reason: None,
            responded_at: chrono::Utc::now(),
            approved_by: "stub".to_string(),
            edited_command: None,
        })
    }

    async fn status(&self) -> tianyan::executor::approval::ApprovalStatusSnapshot {
        tianyan::executor::approval::ApprovalStatusSnapshot {
            config: tianyan::executor::approval::ApprovalWorkflowConfig::default(),
            pending_approvals: Vec::new(),
            pending_confirmations: Vec::new(),
            recent_records: Vec::new(),
            confirmed_action_count: 0,
        }
    }

    async fn respond_to_approval(
        &self,
        _request_id: &str,
        _decision: tianyan::executor::approval::ApprovalDecision,
        _reason: Option<String>,
        _approved_by: &str,
        _edited_command: Option<String>,
    ) -> tianyan::Result<()> {
        Ok(())
    }

    async fn record_user_confirmation_by_fingerprint(&self, _fingerprint: &str) {}
}

/// 未装配工厂时插槽为空：`AgentBuilder` 因此拿到 `None`，工具走「未装配直接放行」
/// 分支 + 启动期告警。
///
/// 判别力：若把 `AssemblyServices::default()` 改成「兜底造一个放行门控」，本断言
/// 必红——而兜底门控会让「未装配」与「装配了全自主模式」在日志与状态查询上
/// **无法区分**（ADR-038 可见失败的反面）。
#[test]
fn test_default_assembly_has_no_approval_factory() {
    assert!(
        AssemblyServices::new()
            .policies()
            .approval_factory()
            .is_none(),
        "未装配时插槽必须为空（降级由 AgentBuilder 的 None 分支承担）"
    );
}

/// 装配了工厂时**只用工厂**，且工厂可重复调用（配置热重载会重建 agent，
/// 审批规则必须跟着新配置走）。
///
/// 判别力：桩返回 `approved_by = "stub"`（工厂内的判别标记）——若插槽没被使用
/// （例如 `approval_factory()` 恒返回 `None`），`expect` 直接 panic。
#[tokio::test]
async fn test_approval_factory_is_injected_and_repeatable() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    let factory: ApprovalGateFactory = Arc::new(
        move |_security: &tianyan::config::SecurityConfig,
              _working_sets: Arc<tianyan::agent::working_set::WorkingSetRegistry>,
              _sink: tianyan::notification::SharedNotificationSink| {
            counter.fetch_add(1, Ordering::SeqCst);
            Arc::new(StubApprovalGate { marker: "injected" }) as Arc<dyn ApprovalGate>
        },
    );
    let assembly = AssemblyServices {
        policies: PolicyAssemblies::default().with_approval_factory(factory),
        ..Default::default()
    };
    let security = tianyan::config::SecurityConfig::default();
    let working_sets = tianyan::agent::working_set::WorkingSetRegistry::new(None);
    let sink: tianyan::notification::SharedNotificationSink =
        Arc::new(tianyan::notification::NoopNotificationSink);

    let gate = assembly
        .policies()
        .approval_factory()
        .expect("装配后必须有工厂")(&security, working_sets.clone(), sink.clone());
    let _ = assembly
        .policies()
        .approval_factory()
        .expect("装配后必须有工厂")(&security, working_sets, sink);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "工厂应可重复调用（不是 FnOnce）"
    );

    let action = tianyan::executor::Action::ReadFile {
        path: "probe".to_string(),
    };
    let resp = gate
        .authorize("s1", &action, false)
        .await
        .expect("桩不该失败");
    assert_eq!(resp.approved_by, "stub", "应使用注入的实例");
}

// ── 上下文策略插槽（ADR-049 期 3-T2）────────────────────────────────

/// 只用于装配测试的上下文端口桩（本库的插槽测试只关心「工厂是否被使用」）。
struct StubContextPort {
    /// 判别标记：装配层注入的是哪个实例（与「未装配」区分）。
    marker: &'static str,
}

#[async_trait]
impl ContextPort for StubContextPort {
    async fn load_injectable(
        &self,
        _query: &str,
        _working_directory: Option<&std::path::Path>,
    ) -> tianyan::Result<tianyan::common::types::InjectableContext> {
        Ok(tianyan::common::types::InjectableContext::new())
    }

    async fn compress_for_session(
        &self,
        _messages: &[tianyan::Message],
        _session_id: &str,
        _recent_input_tokens: usize,
        _force: bool,
        _existing_summary: Option<&str>,
    ) -> Option<tianyan::common::types::StructuredMessage> {
        None
    }

    fn extract_marker_summary(
        &self,
        _marker: &tianyan::common::types::StructuredMessage,
    ) -> Option<String> {
        Some(self.marker.to_string())
    }
}

/// 构造上下文装配能力集合（VFS / 模型服务 / 统计全为最小替身）。
fn make_context_services() -> ContextServices {
    let database = tianyan::db::Database::open_in_memory().expect("内存库");
    ContextServices {
        config: tianyan::config::TianyanConfig::default(),
        vfs: Arc::new(tianyan::test_utils::MockVfs::new()),
        usage_stats: tianyan::observability::usage_stats::UsageStats::new(database).expect("统计"),
        chat: Arc::new(StubChatService),
        chat_model: "stub-model".to_string(),
        chat_spec: None,
    }
}

/// 未装配工厂时插槽为空：`AgentBuilder` 因此拿到 `None`，落到 `NullContextPort`
/// （空可注入上下文 + 永不压缩）+ 启动期告警；`start_server*` 另有硬守卫。
///
/// 判别力：若把 `AssemblyServices::default()` 改成「兜底造一个空直通端口」，本断言
/// 必红——那会让「未装配」与「装配了某个真实现」在日志上**无法区分**
/// （ADR-038 可见失败的反面）。
#[test]
fn test_default_assembly_has_no_context_factory() {
    assert!(
        AssemblyServices::new()
            .policies()
            .context_factory()
            .is_none(),
        "未装配时插槽必须为空（降级由 AgentBuilder 的 None 分支承担）"
    );
    assert!(
        !AssemblyServices::new().policies().has_context_factory(),
        "未装配时启动守卫必须能看见「缺件」"
    );
}

/// 装配了工厂时**只用工厂**，且工厂可重复调用（配置热重载 / 模型切换会重建 agent，
/// 压缩窗口必须跟着新模型规格走）。
///
/// 判别力：桩在 `extract_marker_summary` 里回 marker（工厂内的判别标记）——若插槽
/// 没被使用（例如 `context_factory()` 恒返回 `None`），`expect` 直接 panic。
#[tokio::test]
async fn test_context_factory_is_injected_and_repeatable() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    let factory: ContextFactory = Arc::new(move |_services: &ContextServices| {
        counter.fetch_add(1, Ordering::SeqCst);
        Arc::new(StubContextPort { marker: "injected" }) as Arc<dyn ContextPort>
    });
    let assembly = AssemblyServices {
        policies: PolicyAssemblies::default().with_context_factory(factory),
        ..Default::default()
    };
    assert!(
        assembly.policies().has_context_factory(),
        "装配后守卫必须放行"
    );
    let services = make_context_services();

    let port = assembly
        .policies()
        .context_factory()
        .expect("装配后必须有工厂")(&services);
    let _ = assembly
        .policies()
        .context_factory()
        .expect("装配后必须有工厂")(&services);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "工厂应可重复调用（不是 FnOnce）"
    );

    // 端口身份探针：桩不看内容，任意消息即可。
    let probe = tianyan::context::ContextAssembler::message_to_structured(
        &tianyan::Message::user("probe"),
        "s1",
        None,
        None,
    );
    assert_eq!(
        port.extract_marker_summary(&probe).as_deref(),
        Some("injected"),
        "应使用注入的实例"
    );
}

// ── 评审策略插槽（ADR-049 期 3-T3）──────────────────────────────────

/// 只用于装配测试的自审器桩（本库的插槽测试只关心「工厂是否被使用」）。
struct StubTaskReviewer {
    /// 判别标记：装配层注入的是哪个实例（与「未装配」区分）。
    marker: &'static str,
}

#[async_trait]
impl tianyan::executor::review::TaskReviewer for StubTaskReviewer {
    async fn review(
        &self,
        _description: &str,
        _result: &str,
    ) -> tianyan::executor::review::Judgment {
        tianyan::executor::review::Judgment {
            verdict: tianyan::executor::review::Verdict::NeedsChanges {
                suggestion: self.marker.to_string(),
            },
            reason: self.marker.to_string(),
        }
    }
}

/// 只用于装配测试的复审端口桩。
struct StubSkillReviewPort {
    /// 判别标记：写进记录的 `reason`，用来证明用的是注入的实例。
    marker: &'static str,
}

#[async_trait]
impl tianyan::skills::SkillReviewPort for StubSkillReviewPort {
    async fn latest_review(
        &self,
        skill_id: &str,
    ) -> tianyan::Result<Option<tianyan::skills::SkillReview>> {
        Ok(Some(tianyan::skills::SkillReview {
            ts: 0,
            skill_id: skill_id.to_string(),
            session_id: String::new(),
            score: 0,
            verdict: "neutral".to_string(),
            user_feedback: "none".to_string(),
            reason: self.marker.to_string(),
            evidence: String::new(),
        }))
    }
}

/// 未装配工厂时两个评审插槽都为空：`AgentBuilder` 因此拿到 `None`（不装配自审器
/// + 装配期告警），`create_skill_reviewer` 返回 `None`（`reviews` 为空数组）。
///
/// 判别力：若把 `AssemblyServices::default()` 改成「兜底造一个评审器」，本断言必红
/// ——而兜底评审器会让「未装配」与「装配了真实现」在日志与响应上**无法区分**
/// （ADR-038 可见失败的反面）。
#[test]
fn test_default_assembly_has_no_review_factories() {
    assert!(
        AssemblyServices::new()
            .policies()
            .task_reviewer_factory()
            .is_none(),
        "未装配时自审插槽必须为空（降级由 AgentBuilder 的 None 分支承担）"
    );
    assert!(
        AssemblyServices::new()
            .policies()
            .skill_review_factory()
            .is_none(),
        "未装配时复审插槽必须为空（降级由 create_skill_reviewer 的 None 承担）"
    );
}

/// 装配了工厂时**只用工厂**，且工厂可重复调用（配置热重载重建 agent、以及每次
/// 技能统计请求都现造复审器——两者都要求「不是 FnOnce」）。
///
/// 判别力：两个桩都把 `marker` 写进返回值（自审的 `reason` / 复审记录的 `reason`）
/// —— 插槽没被使用（`*_factory()` 恒 `None`）时 `expect` 直接 panic；若实现被换成
/// 「总是通过」的桩，`NeedsChanges` 的断言也会红。
#[tokio::test]
async fn test_review_factories_are_injected_and_repeatable() {
    let task_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let skill_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let task_counter = task_calls.clone();
    let task_factory: TaskReviewerFactory = Arc::new(
        move |_chat: Arc<dyn tianyan::model::ChatService>, _model: &str| {
            task_counter.fetch_add(1, Ordering::SeqCst);
            Arc::new(StubTaskReviewer { marker: "injected" })
                as Arc<dyn tianyan::executor::review::TaskReviewer>
        },
    );
    let skill_counter = skill_calls.clone();
    let skill_factory: SkillReviewFactory = Arc::new(
        move |_chat: Arc<dyn tianyan::model::ChatService>,
              _model: &str,
              _vfs: Arc<dyn tianyan::vfs::VirtualFileSystem>| {
            skill_counter.fetch_add(1, Ordering::SeqCst);
            Arc::new(StubSkillReviewPort { marker: "injected" })
                as Arc<dyn tianyan::skills::SkillReviewPort>
        },
    );

    let assembly = AssemblyServices {
        policies: PolicyAssemblies::default()
            .with_task_reviewer_factory(task_factory)
            .with_skill_review_factory(skill_factory),
        ..Default::default()
    };
    let chat: Arc<dyn tianyan::model::ChatService> = Arc::new(StubChatService);
    let vfs: Arc<dyn tianyan::vfs::VirtualFileSystem> =
        Arc::new(tianyan::test_utils::MockVfs::new());

    let reviewer = assembly
        .policies()
        .task_reviewer_factory()
        .expect("装配后必须有工厂")(chat.clone(), "stub-model");
    let _ = assembly
        .policies()
        .task_reviewer_factory()
        .expect("装配后必须有工厂")(chat.clone(), "stub-model");
    assert_eq!(
        task_calls.load(Ordering::SeqCst),
        2,
        "工厂应可重复调用（不是 FnOnce）"
    );
    let judgment = reviewer.review("任务", "结果").await;
    assert_eq!(judgment.reason, "injected", "应使用注入的自审器实例");
    assert!(
        !judgment.verdict.is_pass(),
        "桩给的是 NeedsChanges——若判为通过说明走的不是注入的实例"
    );

    let port = assembly
        .policies()
        .skill_review_factory()
        .expect("装配后必须有工厂")(chat, "stub-model", vfs);
    let _ = assembly
        .policies()
        .skill_review_factory()
        .expect("装配后必须有工厂")(
        Arc::new(StubChatService),
        "stub-model",
        Arc::new(tianyan::test_utils::MockVfs::new()),
    );
    assert_eq!(
        skill_calls.load(Ordering::SeqCst),
        2,
        "工厂应可重复调用（不是 FnOnce）"
    );
    let latest = port
        .latest_review("probe")
        .await
        .expect("桩不该失败")
        .expect("桩恒回 Some");
    assert_eq!(latest.reason, "injected", "应使用注入的复审实例");
}

// ── provider 插槽（ADR-049 期 2 序 6）────────────────────────────────

/// 只用于装配测试的连通性探针桩（`marker` 是判别标记）。
struct StubConnectionProbe {
    marker: &'static str,
}

#[async_trait]
impl ConnectionProbe for StubConnectionProbe {
    async fn probe(
        &self,
        _endpoint: &str,
        _api_key: &str,
        model: &str,
    ) -> tianyan::Result<Vec<String>> {
        Ok(vec![format!("{}::{model}", self.marker)])
    }
}

/// 只用于装配测试的嵌入服务桩。
struct StubEmbeddingService;

#[async_trait]
impl tianyan::model::EmbeddingService for StubEmbeddingService {
    async fn embed(
        &self,
        _request: tianyan::model::EmbeddingRequest,
    ) -> tianyan::Result<tianyan::model::EmbeddingResponse> {
        Err(tianyan::TianyanError::Custom(
            "测试桩：不发起嵌入请求".to_string(),
        ))
    }

    fn embedding_dimension(&self, _model: &str) -> usize {
        8
    }
}

/// 只用于装配测试的视觉服务桩。
struct StubVlmService;

#[async_trait]
impl tianyan::model::VlmService for StubVlmService {
    async fn analyze_image(
        &self,
        _request: tianyan::model::VisionRequest,
    ) -> tianyan::Result<tianyan::model::VisionResponse> {
        Err(tianyan::TianyanError::Custom(
            "测试桩：不发起视觉请求".to_string(),
        ))
    }
}

/// 装配了工厂时的能力集合（`marker` 是判别标记，用来证明用的是注入的实例）。
fn stub_llm_services(marker: &'static str) -> ProviderLlmServices {
    let mut chat_clients: std::collections::HashMap<String, tianyan::model::SharedChatService> =
        std::collections::HashMap::new();
    chat_clients.insert(
        "p1".to_string(),
        Arc::new(StubChatService) as tianyan::model::SharedChatService,
    );
    ProviderLlmServices {
        chat: Arc::new(StubChatService),
        chat_model: format!("{marker}-chat"),
        vision: Arc::new(StubVlmService),
        vision_model: Some(format!("{marker}-vision")),
        chat_clients,
        probe: Some(Arc::new(StubConnectionProbe { marker })),
    }
}

fn stub_embed_services(marker: &'static str) -> ProviderEmbedServices {
    ProviderEmbedServices {
        embedding: Arc::new(StubEmbeddingService),
        embedding_model: Some(format!("{marker}-embed")),
        usage_handle: Arc::new(std::sync::Mutex::new(None)),
    }
}

/// 未装配工厂时：两个插槽都为空，容器落到 core 的「未配置」降级服务，探针为 `None`。
///
/// 判别力：① 若 `*_factory()` 兜底造一份实现，第一条断言必红（「未装配」与「装配了
/// 真实现」就再也无法区分，ADR-038 可见失败的反面）；② 若降级服务改成「静默成功」，
/// 调用断言必红——那会让缺配置从「响亮失败」退化成「模型莫名其妙不说话」。
#[tokio::test]
async fn test_default_assembly_provider_services_are_degraded() {
    assert!(
        AssemblyServices::new().providers().llm_factory().is_none(),
        "未装配时聊天/视觉插槽必须为空"
    );
    assert!(
        AssemblyServices::new()
            .providers()
            .embed_factory()
            .is_none(),
        "未装配时嵌入插槽必须为空"
    );

    let config = tianyan::config::ModelsConfig::default();
    let services = crate::model_services::ModelServices::from_factories(&config, None, None)
        .expect("未装配不应报错（降级服务 + 告警）");
    assert_eq!(services.chat_model, "", "未配置时聊天模型名为空串");
    assert!(services.embedding_model.is_none());
    assert!(services.vision_model.is_none());
    assert!(services.chat_clients().is_empty(), "路由池为空");
    assert!(
        services.probe().is_none(),
        "未装配时探针必须缺件（配置页据此报「连接测试不可用」而不是假装成功）"
    );

    let err = services
        .chat
        .chat_completion(tianyan::model::ChatCompletionRequest::new(
            "m",
            vec![tianyan::Message::user("hi")],
        ))
        .await
        .expect_err("降级服务的调用必须报错");
    assert!(
        err.to_string().contains("应用未配置"),
        "降级文案必须是前端向导的判定依据：{err}"
    );
}

/// 装配了工厂时**只用工厂**，且工厂可重复调用（配置热重载会重建模型服务）。
///
/// 判别力：桩把 `marker` 写进模型名与探针返回值——插槽没被使用（`*_factory()` 恒
/// `None`）时容器会是降级形态，四条断言全红。
#[test]
fn test_provider_factories_are_injected_and_repeatable() {
    let llm_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let embed_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let llm_counter = llm_calls.clone();
    let llm_factory: ProviderLlmFactory =
        Arc::new(move |_config: &tianyan::config::ModelsConfig| {
            llm_counter.fetch_add(1, Ordering::SeqCst);
            Ok(stub_llm_services("injected"))
        });
    let embed_counter = embed_calls.clone();
    let embed_factory: ProviderEmbedFactory =
        Arc::new(move |_config: &tianyan::config::ModelsConfig| {
            embed_counter.fetch_add(1, Ordering::SeqCst);
            Ok(stub_embed_services("injected"))
        });

    let assembly = AssemblyServices {
        providers: ProviderAssemblies::default()
            .with_provider_llm_factory(llm_factory)
            .with_provider_embed_factory(embed_factory),
        ..Default::default()
    };
    let config = tianyan::config::ModelsConfig::default();

    let llm_factory = assembly
        .providers()
        .llm_factory()
        .expect("装配后必须有工厂");
    let embed_factory = assembly
        .providers()
        .embed_factory()
        .expect("装配后必须有工厂");
    let first = crate::model_services::ModelServices::from_factories(
        &config,
        Some(&llm_factory),
        Some(&embed_factory),
    )
    .expect("装配后应成功");
    let _ = crate::model_services::ModelServices::from_factories(
        &config,
        Some(&llm_factory),
        Some(&embed_factory),
    )
    .expect("装配后应成功");

    assert_eq!(
        llm_calls.load(Ordering::SeqCst),
        2,
        "工厂应可重复调用（不是 FnOnce）"
    );
    assert_eq!(embed_calls.load(Ordering::SeqCst), 2);
    assert_eq!(first.chat_model, "injected-chat", "应使用注入的实例");
    assert_eq!(first.embedding_model.as_deref(), Some("injected-embed"));
    assert_eq!(first.vision_model.as_deref(), Some("injected-vision"));
    assert!(
        first.chat_clients().contains_key("p1"),
        "路由池来自注入的能力集合"
    );
    assert!(first.probe().is_some(), "探针随能力集合一起注入");
}

/// 连通性探针契约把**临时**凭据原样递给实现（不是已配置的 provider 实例），
/// 且错误分类仍由消费端按语义谓词完成（ADR-014）。
///
/// 判别力：若插槽层的 `probe()` 被改成「用已配置的 provider 造客户端」，本测试
/// 传进去的 endpoint / key 就会丢失，返回的模型名也不再带桩的判别标记。
#[tokio::test]
async fn test_connection_probe_contract_carries_ad_hoc_credentials() {
    let probe: Arc<dyn ConnectionProbe> = Arc::new(StubConnectionProbe { marker: "probe" });
    let models = probe
        .probe("https://gw.example.com/v1", "sk-temp", "gpt-test")
        .await
        .expect("桩不该失败");
    assert_eq!(models, vec!["probe::gpt-test".to_string()]);
}

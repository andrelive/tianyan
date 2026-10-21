//! 装配级校验（ADR-049 期 1）：插件工具注册 + 角色白名单 × 真实工具集。
//!
//! ## 为什么这个测试住在 app 包
//!
//! 期 1 把 fs 域六个工具迁进 `plugins/tools-fs` 后，工具集**不再等于**
//! `builtin_tool_names()`（插件工具在运行时注册）。原先 core 侧的静态断言
//! （白名单 ⊆ 内置表）会误报，已移除；校验改到**装配层**——这里的注册表
//! 是真值（内置 + 插件 + 未来更多插件），保护等价且更正确。
//!
//! 这也是「装配清单住在 bin/装配包」这条边界的副产品：只有依赖插件的包
//! 才能构造出完整工具集。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;
use std::sync::Arc;

use tianyan::agent::{DynamicToolExecutor, RoleRegistry, ToolContext, ToolRegistry};
use tianyan::executor::SecurityPolicy;
use tianyan::goals::GoalStore;
use tianyan::model::types::ToolPresentation;
use tianyan::todos::TodoStore;
use tianyan::vfs::{ContextEntry, StorageBackend, VectorStorage};
use tianyan_server::{StorageBackendFactory, VectorStorageFactory};
use tianyan_storage_sqlite::{LocalFileBackend, SqliteBackend};
use tianyan_storage_vector::LanceDbVectorStore;

/// 插件工具清单——与 `app/src/main.rs::plugin_factory` **同源同序**：
/// fs → code → todo → goal → web（顺序即注册顺序，抖动会白掉提示词前缀缓存）。
///
/// `todo` / `goal` 的实现迁到 `plugins/tools-todo` 后仍需 store：这里用临时目录
/// 建（注册与取 definition 不碰盘，`TempDir` 释放后工具也不会再去读）。
/// `web_search` / `web_fetch` 用默认 `WebConfig`（只取 schema/definition，不发请求）。
fn plugin_tools(ctx: &ToolContext) -> Vec<Arc<dyn DynamicToolExecutor>> {
    let dir = tempfile::tempdir().unwrap();
    let mut tools = tianyan_tools_fs::tools(ctx);
    tools.extend(tianyan_tools_code::tools(ctx));
    tools.extend(tianyan_tools_todo::tools(
        Arc::new(TodoStore::new(dir.path())),
        Arc::new(GoalStore::new(dir.path())),
    ));
    tools.extend(tianyan_tools_web::tools(
        &tianyan::config::WebConfig::default(),
    ));
    tools
}

/// 构造「内置 + 插件」的完整注册表（与运行时装配同一工厂）。
async fn full_registry() -> ToolRegistry {
    let registry = ToolRegistry::new(SecurityPolicy::default());
    let ctx = registry.context().clone();
    for tool in plugin_tools(&ctx) {
        registry.register_dynamic_tool(tool).await;
    }
    registry
}

/// 插件工具必须全部注册，且展示意图来自插件（A2）。
///
/// 覆盖三批迁移：fs 域 6 个（tools-fs）→ code 域 3 个（tools-code）→
/// `todo` / `goal`（tools-todo）。后两个是 `Generic`（结构化状态卡片，
/// 与迁移前的组件工具渲染一致）——它们**不需要**进
/// `MIGRATED_TOOL_PRESENTATIONS`（该表只兜底非默认意图）。
#[tokio::test]
async fn test_plugin_tools_registered_with_presentation() {
    let registry = full_registry().await;
    let defs = registry.definitions().await;
    let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();

    let expected = [
        ("read_file", ToolPresentation::Read),
        ("write_file", ToolPresentation::Write),
        ("apply_edit", ToolPresentation::Diff),
        ("apply_patch", ToolPresentation::Diff),
        ("glob", ToolPresentation::Search),
        ("list_dir", ToolPresentation::Search),
        ("symbol_outline", ToolPresentation::Code),
        ("repo_map", ToolPresentation::Code),
        ("grep", ToolPresentation::Search),
        // tools-todo（ADR-049 期 1）：会话绑定的推理辅助工具
        ("todo", ToolPresentation::Generic),
        ("goal", ToolPresentation::Generic),
        // tools-web（ADR-049 期 2）：网络检索/抓取工具
        ("web_search", ToolPresentation::Web),
        ("web_fetch", ToolPresentation::Web),
    ];
    for (name, presentation) in expected {
        assert!(names.contains(&name), "插件工具未注册：{name}");
        assert_eq!(
            registry.presentation(name),
            presentation,
            "展示意图丢失（插件需覆盖 DynamicToolExecutor::presentation）：{name}"
        );
    }
}

/// 动态工具注册顺序固定：fs（6）→ code（3）→ todo → goal。
///
/// 顺序是提示词前缀的一部分——`todo` / `goal` 从 server 组件工具迁到插件后仍须
/// 保持 `todo` 在 `goal` 之前、且都排在 code 域之后（与迁移前相对次序一致）。
#[tokio::test]
async fn test_plugin_tool_registration_order_is_stable() {
    let registry = full_registry().await;
    let defs = registry.definitions().await;
    let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
    let pos = |name: &str| {
        names
            .iter()
            .position(|n| *n == name)
            .unwrap_or_else(|| panic!("工具未注册：{name}（顺序断言的前提）"))
    };
    assert!(pos("grep") < pos("todo"), "todo 应排在 code 域之后");
    assert!(
        pos("todo") < pos("goal"),
        "todo 必须在 goal 之前（前缀缓存稳定性）"
    );
}

/// 内置角色的工具白名单必须全部已注册（白名单是过滤清单，含未注册名会被
/// **静默忽略**——表现为"该角色某项能力凭空消失"而无任何报错）。
#[tokio::test]
async fn test_builtin_role_whitelists_are_registered() {
    let registry = full_registry().await;
    let defs = registry.definitions().await;
    let names: HashSet<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();

    let roles = RoleRegistry::builtin();
    let mut checked = 0usize;
    for role_name in roles.names() {
        let Some(role) = roles.get(&role_name) else {
            continue;
        };
        let Some(tools) = &role.tools else {
            continue;
        };
        for tool in tools {
            checked += 1;
            assert!(
                names.contains(tool.as_str()),
                "角色 {role_name} 的白名单含未注册工具：{tool}（删除工具时需同步清理白名单）"
            );
        }
    }
    assert!(checked > 0, "应至少检查到一个角色白名单条目");
}

/// 插件工具与内置工具不重名——重名会让 `register_dynamic_tool` 静默跳过，
/// 表现为"插件写了但没生效"（迁移期最容易出现的假成功）。
#[tokio::test]
async fn test_plugin_tools_do_not_shadow_builtin() {
    let builtin = ToolRegistry::new(SecurityPolicy::default());
    let builtin_defs = builtin.definitions().await;
    let builtin_names: HashSet<String> = builtin_defs
        .iter()
        .map(|d| d.function.name.clone())
        .collect();

    let ctx = builtin.context().clone();
    for tool in plugin_tools(&ctx) {
        let name = tool.tool_name();
        assert!(
            !builtin_names.contains(&name),
            "插件工具与内置工具重名（会被静默跳过）：{name}——迁移未完成或表项未删"
        );
    }
}

/// 回归：插件清单工厂必须**可重复调用**（`Arc<dyn Fn>` 而非 `Box<dyn FnOnce>`）。
///
/// 场景：配置热重载 / 模型切换 / MCP 变更都会重建 agent，`reload_agent` 用同一份
/// 工厂重新注入插件工具。若工厂是一次性的，重载后**全部插件工具**
/// （`read_file` / `glob` / `grep` / `repo_map` / `todo` / `goal` …）会静默消失，
/// 直到重启进程——这正是 `tool: 未知工具：todo` 那类故障的放大版。
///
/// 判别力：工厂被调用两次且工具集一致；同时锁住 `ToolPluginFactory` 的类型
/// （改成 `Box<dyn FnOnce>` 会让本测试编译失败）。
#[tokio::test]
async fn test_plugin_factory_is_repeatable_across_agent_rebuilds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let make_services = || {
        tianyan_server::PluginServices::new(
            Arc::new(TodoStore::new(dir.path())),
            Arc::new(GoalStore::new(dir.path())),
            tianyan::config::WebConfig::default(),
        )
    };
    let factory: tianyan_server::ToolPluginFactory = Arc::new(|ctx, services| {
        let mut tools = tianyan_tools_fs::tools(ctx);
        tools.extend(tianyan_tools_code::tools(ctx));
        tools.extend(tianyan_tools_todo::tools(
            services.todo_store(),
            services.goal_store(),
        ));
        tools.extend(tianyan_tools_web::tools(&services.web_config()));
        tools
    });

    // 第一次装配（首建）与第二次装配（模拟 reload 后的新注册表）必须得到同一工具集
    let mut snapshots: Vec<Vec<String>> = Vec::new();
    for _ in 0..2 {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        let ctx = registry.context().clone();
        for tool in factory(&ctx, make_services()) {
            registry.register_dynamic_tool(tool).await;
        }
        let mut names: Vec<String> = registry
            .definitions()
            .await
            .iter()
            .map(|d| d.function.name.clone())
            .collect();
        names.sort();
        snapshots.push(names);
    }

    assert_eq!(
        snapshots[0], snapshots[1],
        "插件工厂必须可重复调用：热重载后工具集必须与首建一致"
    );
    for expected in [
        "todo",
        "goal",
        "read_file",
        "glob",
        "grep",
        "repo_map",
        "web_search",
        "web_fetch",
    ] {
        assert!(
            snapshots[0].iter().any(|n| n == expected),
            "插件工具缺失：{expected}"
        );
    }
}

/// 装配回归（ADR-049 期 2 序 3）：向量存储工厂必须真的产出 **LanceDB**
/// （而不是 server 侧「未装配」时的空降级实现）。
///
/// 判别力：
/// 1. 证明 `VectorStorageFactory` 的写法在**装配包外部**可用（闭包 → `Arc<dyn Fn>`
///    → boxed future 的类型擦除链真的编译得过，与 `app/src/main.rs` 同形）；
/// 2. 证明 `{data_dir}/lancedb` 被真实创建——空实现不会在磁盘留痕；
/// 3. 维度来自 `StorageConfig`（8），说明配置被正确传入而非默认值。
#[tokio::test]
async fn test_vector_factory_yields_real_lancedb_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = tianyan::config::StorageConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };
    config.vector.collection_name = "assembly_test".to_string();
    config.vector.vector_dimension = 8;
    // 与 `app/src/main.rs::vector_factory` 同形（借用 → clone → move 进 async）
    let factory: VectorStorageFactory = Arc::new(|config: &tianyan::config::StorageConfig| {
        let config = config.clone();
        Box::pin(async move {
            let store = LanceDbVectorStore::new(&config).await?;
            Ok(Arc::new(store) as Arc<dyn VectorStorage>)
        })
    });

    let store = factory(&config).await.expect("工厂应构造成功");
    assert_eq!(store.embedding_dim(), 8, "维度应来自注入的配置");
    assert_eq!(store.count_rows().await.expect("行数可读"), 0);
    assert!(
        dir.path().join("lancedb").is_dir(),
        "LanceDB 数据目录应被创建（空实现不留痕）"
    );
}

/// 装配回归（ADR-049 期 2 序 4）：结构化存储工厂必须按 `config.storage.backend`
/// 真的产出**插件里的实现**，而不是 server 侧「未装配」时的内存兜底。
///
/// 判别力：
/// 1. 与 `app/src/main.rs::storage_factory` 同形的工厂在**装配包外部**构造得出来
///    （同步工厂的类型擦除链真的编译得过）；
/// 2. `Sqlite` 分支写入的条目能被**另开一条连接到同一数据库文件**的后端读回——
///    内存兜底不落盘，`exists` 必为 false；即「装配层真的注入了 SqliteBackend」；
/// 3. `Local` 分支会真的在磁盘上建出数据目录——内存兜底不建目录；
/// 4. 两个分支共用同一份 `StorageConfig`，说明 adapter 选择发生在装配层（server
///    库只递配置与共享连接，不认识这两个类型）。
#[tokio::test]
async fn test_storage_factory_yields_real_backend_per_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("tianyan.db");
    // 与 `app/src/main.rs::storage_factory` 同形
    let factory: StorageBackendFactory = Arc::new(
        |config: &tianyan::config::StorageConfig, database: Arc<tianyan::db::Database>| {
            use tianyan::config::StorageBackendType;
            let backend: Arc<dyn StorageBackend> = match config.backend {
                StorageBackendType::Local => Arc::new(LocalFileBackend::new(config.clone())),
                StorageBackendType::Sqlite => Arc::new(SqliteBackend::new(database)),
            };
            Ok(backend)
        },
    );

    // ── Sqlite 分支：写入必须落到数据库文件 ──────────────────────────────
    let sqlite_config = tianyan::config::StorageConfig {
        data_dir: dir.path().to_path_buf(),
        backend: tianyan::config::StorageBackendType::Sqlite,
        sqlite_path: Some(db_path.clone()),
        ..Default::default()
    };
    let database = tianyan::db::Database::open(db_path.clone()).expect("打开数据库");
    database.init_schemas().await.expect("初始化 schema");
    let backend = factory(&sqlite_config, database).expect("Sqlite 分支应构造成功");
    backend.initialize().await.expect("建 vfs_entries 表");
    let uri = tianyan::TianyanUri::new(
        tianyan::ContextNamespace::Knowledge,
        vec!["assembly_probe.md".to_string()],
    );
    backend
        .write_entry(&ContextEntry::new_file(uri.clone()))
        .await
        .expect("写条目");
    drop(backend);

    // 另开一条到同一文件的连接读回（内存兜底不会有任何落盘痕迹）
    let reopened = tianyan::db::Database::open(db_path).expect("重开数据库");
    reopened.init_schemas().await.expect("重开初始化 schema");
    let reopened_backend =
        factory(&sqlite_config, reopened).expect("Sqlite 分支应可重复构造（重启/搬迁）");
    assert!(
        reopened_backend.exists(&uri).await.expect("存在性查询"),
        "Sqlite 分支必须落盘（未装配时会退化成内存后端，此处读回为空）"
    );

    // ── Local 分支：必须真的在磁盘上建目录 ───────────────────────────────
    let local_dir = dir.path().join("local-data");
    let local_config = tianyan::config::StorageConfig {
        data_dir: local_dir.clone(),
        backend: tianyan::config::StorageBackendType::Local,
        sqlite_path: None,
        ..Default::default()
    };
    let database = tianyan::db::Database::open(dir.path().join("tianyan2.db")).expect("打开数据库");
    let local_backend = factory(&local_config, database).expect("Local 分支应构造成功");
    assert!(
        !local_dir.exists(),
        "前置：构造阶段不做 IO（建目录在 initialize 里）"
    );
    local_backend.initialize().await.expect("建数据目录");
    assert!(
        local_dir.is_dir(),
        "Local 分支应建出数据目录（内存兜底不会留痕）"
    );
}

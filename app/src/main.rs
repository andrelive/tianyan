//! 天演装配器（服务器二进制入口）。
//!
//! ## 期 4：清单与工厂搬到 `tianyan-assembly`（ADR-049 期 4）
//!
//! 改造前本文件与 `tauri/src/server.rs` **各写了一遍**那 11 个工厂（逐字同形，靠
//! 注释「与 `app/src/main.rs::xxx_factory` 同形同源」互相提醒）。两份副本的漂移是
//! **静默的**：插槽签名加一个参数时，先改的那份能编、后改的那份报错；而**跨插件
//! 顺序纪律**（工具 fs → code → todo/goal → web、任务 evolution 排第 2 位）在两份里
//! 分别成立，一旦某份被单独调整，症状是「桌面端聊天前缀缓存命中率突然下降」而不是
//! 编译失败——没有编译器能替你发现它。
//!
//! 因此期 4 把 11 个工厂与两份装配清单（`full()` 生产 11 插槽 / `policies()` 集成
//! 测试用的 9 插槽前缀，外加供工具目录生成器与装配级测试共用的 `plugin_factory()`）
//! 搬进 `tianyan-assembly`（全仓唯一 `use` 全部插件的地方，见该 crate 文档的依赖
//! 方向图），本包只保留**装配器职责**：加载配置与日志 + 启动 server + 搬迁重启循环
//! ——即「一个 bin 该有的样子」。下面各段历史记录的是每一步为什么这样分层，仍是
//! 理解 `tianyan-assembly` 为何长成那样的依据（不要因为「代码已不在这里」而删掉）。
//!
//! ## 为什么单独成包（ADR-049 期 1）
//!
//! **装配清单住在 bin**。Cargo 的依赖粒度是 package，若清单写在
//! `tianyan-server` 的 bin 里，整个 server 包就依赖插件 crate —— 改一行工具会
//! 重编 server 库（13.4K 源码），闸门 A（≤3.5K）直接失效。
//!
//! 因此本包只做三件事：加载配置与日志、给出插件清单、启动 server。
//! server 库通过 `ToolPluginFactory`（类型擦除）接收清单，**不认识任何插件类型**。
//! （期 4 起「给出插件清单」这一步改由 `tianyan-assembly` 承担，分层理由不变：
//! 依赖插件的包在 server **之上**。）
//!
//! 期 2 序 3 起同一模式也用于**向量存储**：`lancedb`/`arrow`/`datafusion` 链搬进
//! `plugins/storage-vector` 后，core 不再承担它；本包把 `LanceDbVectorStore` 包成
//! `VectorStorageFactory` 注入启动入口（server 库同样不认识该实现）。
//!
//! 期 2 序 4 再扩一个插槽：VFS 的**结构化存储**（`SqliteBackend` /
//! `LocalFileBackend`）搬进 `plugins/storage-sqlite`，本包把两者包成
//! `StorageBackendFactory` 注入——`config.storage.backend` 选哪个 adapter 也在这里
//! 决定（server 库只递配置与共享连接）。
//!
//! 期 2 序 5a 扩第四插槽：**内置定时任务**（摘要 / GC / 统计刷盘 / 快照 GC / 提醒）
//! 搬进 `plugins/task-builtin`，本包把插件的 `tasks()` 包成 `TaskFactory` 注入
//! （server 库只认清单项类型，不认识任何具体任务）。
//!
//! 期 2 序 5b 该插槽收尾：**演化任务**搬进 `plugins/task-evolution`；两个插件的清单
//! 在 `task_factory()` 里**拼接**（`splice(1..1, …)`：演化任务紧跟 `summary_generation`）
//! ——跨插件顺序只有装配清单知道，插件之间不互相 `import`（D2-2）。
//!
//! 期 3-T1 扩第五插槽：**审批策略**（风险定级 / 自动规则 / 模式短路 / 人工审批挂起 /
//! 审计）搬进 `plugins/policy-approval`，本包把插件的 `gate()` 包成
//! `ApprovalGateFactory` 注入——server 库只认 `Arc<dyn ApprovalGate>`，不认识审批策略。
//!
//! 期 3-T2 扩第六插槽：**上下文策略**（注入管线 / 双层检索 RRF / 意图分类 / 分层压缩）
//! 搬进 `plugins/policy-context`，本包把插件的 `pipeline()` 包成 `ContextFactory` 注入
//! ——server 库只认 `Arc<dyn ContextPort>`，不认识上下文策略。这是期 3 的主体收益：
//! 改 prompt 模板 / 压缩阈值 / 检索参数不再重编内核与通道层。

use tianyan_server::start_server_with_assembly;

#[tokio::main]
async fn main() -> tianyan::common::error::Result<()> {
    // 日志初始化只做一次（tracing 全局 subscriber 不可重复设置；
    // 搬迁重启循环复用同一日志）
    //
    // 装配顺序（ADR-049 期 3-T4）：**先加载并注入内核槽位，再初始化日志**
    // ——`init()` / 内核读 `get_config()` 时要拿到真实配置（[logging] 节）。
    // `load_and_install` 是这条语义的唯一落点（加载失败 → stderr + 诊断槽位 +
    // 回退默认配置），三处装配因此各写一行。
    let config = tianyan_service_config::load_and_install();
    tianyan::common::logging::init_logging(&config.logging)?;
    // panic hook（最后防线）：panic 现场写入 <日志目录>/panic.log
    // （与桌面端同一目录：core::common::panic_hook::default_log_dir）。
    tianyan::common::panic_hook::install(tianyan::common::panic_hook::default_log_dir());

    // 主循环：数据目录搬迁后以新配置重启（搬迁由 start_server 关停后的
    // handle_pending_migration 执行；无搬迁时单次运行即退出）
    loop {
        // 重启前重读配置（搬迁/设置页改过 data_dir）；内核槽位是**启动快照**，
        // 重复注入不生效——返回值才是本次要用的配置，与迁移前同义。
        let config = tianyan_service_config::load_and_install();

        // 端口来源单点：from_env 读 TIANYAN_PORT（未设 → 默认 127.0.0.1:3000）。
        // e2e 借此用备用端口启动，避免与运行中的桌面应用抢占 3000。
        let server_config = tianyan_server::ServerConfig::from_env();
        // 完整装配（11 插槽）：单一实现在 `tianyan-assembly`（ADR-049 期 4），
        // 与 `tauri/src/server.rs` 共用同一份清单与同一条顺序纪律。
        match start_server_with_assembly(server_config, config.clone(), tianyan_assembly::full())
            .await
        {
            Ok(()) => {
                // 服务器正常关停：检查是否有待处理的数据目录搬迁
                if tianyan_server::migration::handle_pending_migration(&config)
                    .await
                    .is_some()
                {
                    eprintln!("数据目录搬迁完成，以新配置重启");
                    continue;
                }
                break;
            }
            Err(e) => {
                eprintln!("服务器错误：{}", e);
                break;
            }
        }
    }
    Ok(())
}

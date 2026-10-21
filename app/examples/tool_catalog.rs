//! 工具目录生成器（**权威生成器**：覆盖 core 内置 + 插件工具 + server 组件工具）。
//!
//! 用法：**一律经 `.cargo/config.toml` 的 alias 调用**（命令单一来源——CI 步骤、
//! `scripts/gen-tool-catalog.ps1`、`scripts/wsl-ci-parity.sh` 共用）：
//!
//! ```text
//! cargo catalog-gen     # 生成目录内容到 stdout
//! cargo catalog-check   # freshness 门禁（漂移 → 退出码 1）
//! ```
//!
//! **为什么在 app 侧**（ADR-049 期 1）：生成器必须能装配**插件工具**
//! （`tianyan-tools-fs` 的 glob / list_dir、`tianyan-tools-code`、`tianyan-tools-todo`
//! 的 todo / goal），而插件依赖只允许出现在装配层。此前生成器在 server 侧；若在
//! 那里注册插件，server 库就依赖插件 crate，改一行工具会重编 server（13.4K）——
//! 闸门 A 直接失效。
//!
//! `schedule_task` 仍来自 server 包：它由 server 侧定义（依赖 server 内部的定时
//! 任务管理器通道），app 只做装配。

use std::sync::Arc;

use tianyan::agent::{tool_catalog, ToolRegistry};
use tianyan::executor::SecurityPolicy;
use tianyan::goals::GoalStore;
use tianyan::todos::TodoStore;
use tianyan_server::scheduled_tasks::tool::ScheduleTaskTool;

#[tokio::main]
async fn main() {
    let check = std::env::args().any(|a| a == "--check");
    let registry = ToolRegistry::new(SecurityPolicy::default());

    // 插件工具（ADR-049 期 1）：与运行时装配同一工厂——目录与真实工具集一致，
    // 否则插件工具既无文档覆盖、也不受 freshness 门禁保护。
    let ctx = registry.context().clone();
    for tool in tianyan_tools_fs::tools(&ctx) {
        registry.register_dynamic_tool(tool).await;
    }
    for tool in tianyan_tools_code::tools(&ctx) {
        registry.register_dynamic_tool(tool).await;
    }

    // 组件工具：临时存储 + 一次性通道装配（只为取 definition，不执行任何操作）。
    // `todo` / `goal` 已迁至 `plugins/tools-todo`（ADR-049 期 1）：与运行时装配
    // 同一函数，store 用临时目录（只取 schema，不落盘）。
    let dir = std::env::temp_dir().join(format!("tianyan-tool-catalog-{}", std::process::id()));
    for tool in tianyan_tools_todo::tools(
        Arc::new(TodoStore::new(&dir)),
        Arc::new(GoalStore::new(&dir)),
    ) {
        registry.register_dynamic_tool(tool).await;
    }
    // `web_search` / `web_fetch` 已迁至 `plugins/tools-web`（ADR-049 期 2）：用默认
    // WebConfig（catalog 只取 schema，不发起任何请求）。
    for tool in tianyan_tools_web::tools(&tianyan::config::WebConfig::default()) {
        registry.register_dynamic_tool(tool).await;
    }
    let (req_tx, _req_rx) = tokio::sync::mpsc::channel(1);
    registry
        .register_dynamic_tool(Arc::new(ScheduleTaskTool::new(req_tx)))
        .await;

    let generated = tool_catalog::render(&registry).await;
    let path = std::path::Path::new("docs/architecture/tool-catalog.md");
    if check {
        if tool_catalog::check(path, &generated) {
            println!("OK: 工具目录与代码一致");
        } else {
            eprintln!("FAIL: 工具目录已漂移！请运行 scripts/gen-tool-catalog.ps1 重新生成");
            std::process::exit(1);
        }
    } else {
        print!("{generated}");
    }
}

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
//! 插件的**清单出处**是 `tianyan_assembly::plugin_factory()`（ADR-049 期 4）——与
//! 运行时装配**同一份**实现：目录与真实工具集因此逐字一致，插件工具既有文档覆盖、
//! 也受 freshness 门禁保护。改造前这里自写了一遍清单（连同两个装配器与一个集成测试
//! 工厂，同一份清单共 5 处副本），任一处的顺序漂移都会让目录与运行时工具集悄悄错位。
//!
//! `schedule_task` 仍来自 server 包：它由 server 侧定义（依赖 server 内部的定时
//! 任务管理器通道），app 只做装配。

use std::sync::Arc;

use tianyan::agent::ToolRegistry;
use tianyan::executor::SecurityPolicy;
use tianyan::goals::GoalStore;
use tianyan::todos::TodoStore;
use tianyan_server::scheduled_tasks::tool::ScheduleTaskTool;
// 目录渲染与 freshness 比较口径（ADR-049 期 3-T5：实现已出 core，随工具管线插件走）。
use tianyan_tool_pipeline::tool_catalog;

#[tokio::main]
async fn main() {
    let check = std::env::args().any(|a| a == "--check");
    let registry = ToolRegistry::new(SecurityPolicy::default());

    // 插件工具（ADR-049 期 1 + 期 4）：与运行时装配同一工厂——目录与真实工具集一致，
    // 否则插件工具既无文档覆盖、也不受 freshness 门禁保护。
    //
    // 顺序由工厂内部固定（fs → code → todo/goal → web，前缀缓存稳定性）；这里的
    // store 用临时目录（只取 schema，不落盘），`WebConfig::default()`（不发请求）。
    let ctx = registry.context().clone();
    let dir = std::env::temp_dir().join(format!("tianyan-tool-catalog-{}", std::process::id()));
    let services = tianyan_server::PluginServices::new(
        Arc::new(TodoStore::new(&dir)),
        Arc::new(GoalStore::new(&dir)),
        tianyan::config::WebConfig::default(),
    );
    for tool in tianyan_assembly::plugin_factory()(&ctx, services) {
        registry.register_dynamic_tool(tool).await;
    }

    // 组件工具：一次性通道装配（只为取 definition，不执行任何操作）。
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

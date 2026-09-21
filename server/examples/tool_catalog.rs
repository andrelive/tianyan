//! 工具目录生成器（**权威生成器**：覆盖 core 内置 + server 组件工具）。
//!
//! 用法（经 `scripts/gen-tool-catalog.ps1` 调用）：
//!
//! ```text
//! cargo run -q -p tianyan-server --example tool_catalog             # 生成
//! cargo run -q -p tianyan-server --example tool_catalog -- --check  # freshness 门禁
//! ```
//!
//! **为什么在 server 侧**：只有这里能同时装配 core 内置工具与组件工具
//! （`todo`/`goal`/`schedule_task`，ADR-003 组件工具化）。此前生成器在 core，
//! 故这三个工具**不在目录内**——既没有文档覆盖、也不受 freshness 门禁保护，
//! 而它们的 schema 又是手写的：两个保险都缺（`todo` 的「单条快捷形态」与
//! `goal` 的 `status` 一词两用正是在这种土壤里长出来的）。

use std::sync::Arc;

use tianyan::agent::{tool_catalog, ToolRegistry};
use tianyan::executor::SecurityPolicy;
use tianyan::goals::GoalStore;
use tianyan::todos::TodoStore;
use tianyan_server::api::goals::tool::GoalTool;
use tianyan_server::api::todos::tool::TodoTool;
use tianyan_server::scheduled_tasks::tool::ScheduleTaskTool;

#[tokio::main]
async fn main() {
    let check = std::env::args().any(|a| a == "--check");
    let registry = ToolRegistry::new(SecurityPolicy::default());

    // 组件工具：临时存储 + 一次性通道装配（只为取 definition，不执行任何操作）。
    let dir = std::env::temp_dir().join(format!("tianyan-tool-catalog-{}", std::process::id()));
    let (req_tx, _req_rx) = tokio::sync::mpsc::channel(1);
    registry
        .register_dynamic_tool(Arc::new(TodoTool::new(Arc::new(TodoStore::new(&dir)))))
        .await;
    registry
        .register_dynamic_tool(Arc::new(GoalTool::new(Arc::new(GoalStore::new(&dir)))))
        .await;
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

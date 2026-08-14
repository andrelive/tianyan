//! 工具目录生成器（A3 生成式文档：DSH gen-tool-catalog 吸收）。
//!
//! 从代码真启动 ToolRegistry（运行时注册是 schema 唯一真相源），输出
//! 内置工具 + 动态工具的 markdown 目录。脚本化调用：
//!
//! ```text
//! cargo run -p tianyan-core --example tool_catalog > docs/architecture/tool-catalog.md
//! ```
//!
//! `--check` 模式：输出到临时文件并与既有文档对比，不一致则退出码 1
//! （CI freshness 门禁，见 scripts/gen-tool-catalog.ps1）。

use tianyan::agent::ToolRegistry;
use tianyan::executor::SecurityPolicy;
use tianyan::model::types::ToolPresentation;

fn main() {
    let check = std::env::args().any(|a| a == "--check");
    let registry = ToolRegistry::new(SecurityPolicy::default());
    let defs = tokio::runtime::Runtime::new()
        .expect("runtime 创建失败")
        .block_on(async { registry.definitions().await });

    let mut md = String::new();
    md.push_str("# 天演工具目录（自动生成）\n\n");
    md.push_str("> 本文件由 `cargo run -p tianyan-core --example tool_catalog` 自动生成。\n");
    md.push_str("> 手工修改将被覆盖；freshness 由 `scripts/gen-tool-catalog.ps1 -Check` 门禁。\n");
    md.push_str("> 生成原则（DSH gen-tool-catalog 吸收）：运行时注册是 schema 唯一真相源。\n\n");
    md.push_str(&format!("共 {} 个工具。\n\n", defs.len()));
    md.push_str("| 工具 | 展示意图 | 描述 |\n");
    md.push_str("|------|---------|------|\n");

    let mut names: Vec<_> = defs.iter().map(|d| d.function.name.clone()).collect();
    names.sort();
    for name in names {
        let def = defs
            .iter()
            .find(|d| d.function.name == name)
            .expect("定义存在");
        let presentation = match registry.presentation(&name) {
            ToolPresentation::Generic => "generic",
            ToolPresentation::Read => "read",
            ToolPresentation::Write => "write",
            ToolPresentation::Terminal => "terminal",
            ToolPresentation::Diff => "diff",
            ToolPresentation::Search => "search",
            ToolPresentation::Web => "web",
            ToolPresentation::Skill => "skill",
            ToolPresentation::Knowledge => "knowledge",
            ToolPresentation::Delegate => "delegate",
            ToolPresentation::Code => "code",
        };
        let description = def.function.description.replace('|', "\\|");
        md.push_str(&format!(
            "| `{}` | {} | {} |\n",
            name, presentation, description
        ));
    }

    if check {
        let path = std::path::Path::new("docs/architecture/tool-catalog.md");
        let existing = std::fs::read_to_string(path).unwrap_or_default();
        // Windows 编辑器/脚本重定向可能写入 CRLF——比较前归一化为 LF，
        // 与程序输出的 '\n' 对齐（换行差异不算漂移）。
        let existing = existing.replace("\r\n", "\n");
        if existing == md {
            println!("OK: 工具目录与代码一致");
        } else {
            eprintln!("FAIL: 工具目录已漂移！请运行 scripts/gen-tool-catalog.ps1 重新生成");
            std::process::exit(1);
        }
    } else {
        print!("{}", md);
    }
}

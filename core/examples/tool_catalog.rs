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

/// 平台描述归一化（freshness 跨平台比较用）：`execute_command` 的描述含构建
/// 平台信息（Windows/Unix shell 提示，见 builtin_tools::shell_platform_hint）——
/// 生成产物按平台不同。门禁在 CI（Linux）对 Windows 生成的仓库文件比较时，
/// 两边都把平台提示替换为占位（平台差异不算漂移；换行差异由调用方另行归一化）。
fn normalize_platform_hints(s: &str) -> String {
    const PLACEHOLDER: &str = "<平台提示>";
    s.replace(tianyan::agent::PLATFORM_HINT_WINDOWS, PLACEHOLDER)
        .replace(tianyan::agent::PLATFORM_HINT_UNIX, PLACEHOLDER)
}

fn main() {
    let check = std::env::args().any(|a| a == "--check");
    let registry = ToolRegistry::new(SecurityPolicy::default());
    let defs = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt.block_on(async { registry.definitions().await }),
        Err(e) => {
            eprintln!("tool_catalog: 运行时创建失败: {e}");
            std::process::exit(2);
        }
    };

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
        let Some(def) = defs.iter().find(|d| d.function.name == name) else {
            eprintln!("tool_catalog: 定义缺失: {name}");
            std::process::exit(2);
        };
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
        // 平台差异归一化后比较（见 normalize_platform_hints 文档）。
        if normalize_platform_hints(&existing) == normalize_platform_hints(&md) {
            println!("OK: 工具目录与代码一致");
        } else {
            eprintln!("FAIL: 工具目录已漂移！请运行 scripts/gen-tool-catalog.ps1 重新生成");
            std::process::exit(1);
        }
    } else {
        print!("{}", md);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 判别力：归一化必须抹平"Windows 提示 vs Unix 提示"的差异——
    /// 否则跨平台 freshness 门禁恒红（CI 上的"目录偏移"实为平台差异）。
    #[test]
    fn platform_hint_normalization_erases_platform_difference() {
        let windows_catalog =
            format!("head\n{}\ntail", tianyan::agent::PLATFORM_HINT_WINDOWS);
        let unix_catalog =
            format!("head\n{}\ntail", tianyan::agent::PLATFORM_HINT_UNIX);
        assert_ne!(windows_catalog, unix_catalog, "原始文本应含平台差异");
        assert_eq!(
            normalize_platform_hints(&windows_catalog),
            normalize_platform_hints(&unix_catalog),
            "归一化后应一致（跨平台不误报漂移）"
        );
    }
}

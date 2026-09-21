//! 工具目录渲染与 freshness 校验（生成式文档：DSH gen-tool-catalog 纪律吸收）。
//!
//! **运行时注册是 schema 唯一真相源**：目录由 [`ToolRegistry`] 实际注册的工具
//! 渲染（内置 + 动态注册的工具）。
//!
//! 权威生成器在 **server 侧**（`server/examples/tool_catalog.rs`）——只有它能
//! 同时装配 core 内置工具与 server 组件工具（`todo`/`goal`/`schedule_task`，
//! ADR-003 组件工具化）。此前生成器在 core 侧，故那三个工具**不在目录内**，
//! 既没有文档覆盖、也不受 freshness 门禁保护（其 schema 又是手写的，两个保险
//! 都缺——`todo` 的「单条快捷形态」正是在这种土壤里长出来的）。

use std::path::Path;

use super::ToolRegistry;
use crate::model::types::ToolPresentation;

/// 平台描述归一化（freshness 跨平台比较用）：`execute_command` 的描述含
/// **动态 shell 事实**（`executor::shell::HINT_TEMPLATES`——随执行底层而变，
/// ADR-037）——生成产物按环境不同。门禁在 CI（Linux）对 Windows 生成的仓库
/// 文件比较时，两边都把**所有内置提示词模板**替换为占位（平台差异不算漂移；
/// 换行差异由调用方另行归一化）。custom hint 不进仓库产物（产物按默认配置
/// auto 生成），无需归一化。
pub fn normalize_platform_hints(s: &str) -> String {
    const PLACEHOLDER: &str = "<平台提示>";
    let mut out = s.to_string();
    for hint in crate::executor::shell::HINT_TEMPLATES {
        out = out.replace(*hint, PLACEHOLDER);
        // 产物表格把描述里的 `|` 转义为 `\|`——PS 系提示词含 `&& / ||`，
        // 故仓库产物中的 hint 是**转义形态**。不用转义形态再替换一次，
        // 跨平台门禁会误报漂移（同平台两侧同文本，Windows 上侥幸通过）。
        let escaped = hint.replace('|', "\\|");
        if escaped != *hint {
            out = out.replace(&escaped, PLACEHOLDER);
        }
    }
    out
}

/// 展示意图 → 目录里的短名。
fn presentation_name(p: ToolPresentation) -> &'static str {
    match p {
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
    }
}

/// 渲染工具目录（markdown）：工具按名排序；描述里的 `|` 转义。
pub async fn render(registry: &ToolRegistry) -> String {
    let defs = registry.definitions().await;
    let mut md = String::new();
    md.push_str("# 天演工具目录（自动生成）\n\n");
    md.push_str("> 本文件由 `scripts/gen-tool-catalog.ps1` 自动生成（权威生成器：`server/examples/tool_catalog.rs`——覆盖 core 内置 + server 组件工具）。\n");
    md.push_str("> 手工修改将被覆盖；freshness 由 `scripts/gen-tool-catalog.ps1 -Check` 门禁。\n");
    md.push_str("> 生成原则（DSH gen-tool-catalog 吸收）：运行时注册是 schema 唯一真相源。\n\n");
    md.push_str(&format!("共 {} 个工具。\n\n", defs.len()));
    md.push_str("| 工具 | 展示意图 | 描述 |\n");
    md.push_str("|------|---------|------|\n");

    let mut names: Vec<_> = defs.iter().map(|d| d.function.name.clone()).collect();
    names.sort();
    for name in names {
        let Some(def) = defs.iter().find(|d| d.function.name == name) else {
            continue;
        };
        let presentation = presentation_name(registry.presentation(&name));
        let description = def.function.description.replace('|', "\\|");
        md.push_str(&format!(
            "| `{}` | {} | {} |\n",
            name, presentation, description
        ));
    }
    md
}

/// 目录是否与既有文件一致（换行/平台差异归一化后比较）。
///
/// 既有文件缺失时返回 `false`（视为漂移——需生成）。
pub fn check(existing_path: &Path, generated: &str) -> bool {
    let Ok(existing) = std::fs::read_to_string(existing_path) else {
        return false;
    };
    // Windows 编辑器/脚本重定向可能写入 CRLF——比较前归一化为 LF，
    // 与程序输出的 '\n' 对齐（换行差异不算漂移）。
    let existing = existing.replace("\r\n", "\n");
    normalize_platform_hints(&existing) == normalize_platform_hints(generated)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 判别力：归一化必须抹平"不同 shell 事实"的差异——否则跨平台/跨配置的
    /// freshness 门禁恒红（CI 上的"目录偏移"实为平台差异）。
    #[test]
    fn platform_hint_normalization_erases_platform_difference() {
        // 两种形态都要抹平：产物表格会把 `|` 转义为 `\|`（PS 系 hint 含
        // `&& / ||`）——只处理未转义形态会让跨平台门禁恒红。
        for (label, hint) in [
            ("HINT_PWSH", crate::executor::shell::HINT_PWSH),
            (
                "HINT_WINDOWS_POWERSHELL",
                crate::executor::shell::HINT_WINDOWS_POWERSHELL,
            ),
            ("HINT_CMD", crate::executor::shell::HINT_CMD),
        ] {
            let unix_catalog = format!("head\n{}\ntail", crate::executor::shell::HINT_BASH);
            for raw in [hint, &hint.replace('|', "\\|")] {
                let windows_catalog = format!("head\n{raw}\ntail");
                if raw == hint {
                    assert_ne!(
                        windows_catalog, unix_catalog,
                        "{label}: 原始文本应含平台差异"
                    );
                }
                assert_eq!(
                    normalize_platform_hints(&windows_catalog),
                    normalize_platform_hints(&unix_catalog),
                    "{label}: 归一化后应一致（形态：{raw}）"
                );
            }
        }
    }

    #[tokio::test]
    async fn render_lists_registered_tools_sorted_with_presentation() {
        let registry = ToolRegistry::new(crate::executor::SecurityPolicy::default());
        let md = render(&registry).await;
        assert!(md.contains("个工具"), "{md}");
        assert!(md.contains("| `read_file` | read |"), "应含 read_file 行");
        assert!(md.contains("| `execute_command` | terminal |"));
    }
}

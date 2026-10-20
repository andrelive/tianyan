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

use tianyan::agent::{RoleRegistry, ToolRegistry};
use tianyan::executor::SecurityPolicy;
use tianyan::model::types::ToolPresentation;

/// 构造「内置 + 插件」的完整注册表（与运行时装配同一工厂）。
async fn full_registry() -> ToolRegistry {
    let registry = ToolRegistry::new(SecurityPolicy::default());
    let ctx = registry.context().clone();
    for tool in tianyan_tools_fs::tools(&ctx) {
        registry.register_dynamic_tool(tool).await;
    }
    registry
}

/// 插件提供的六个 fs 工具必须全部注册，且展示意图来自插件（A2）。
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
    for tool in tianyan_tools_fs::tools(&ctx) {
        let name = tool.tool_name();
        assert!(
            !builtin_names.contains(&name),
            "插件工具与内置工具重名（会被静默跳过）：{name}——迁移未完成或表项未删"
        );
    }
}

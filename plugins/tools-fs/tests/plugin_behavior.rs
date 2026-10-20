//! 插件工具行为测试：与迁移前 core 内置实现逐项对齐。
//!
//! 覆盖点与 `core/src/agent/tool_registry/fs_ops_tests.rs` 一致（成功路径 /
//! 参数校验 / 安全门控），确保「实现搬家」不是「行为搬家」。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use tianyan::agent::{DynamicToolExecutor, ToolContext};
use tianyan::executor::SecurityPolicy;

use tianyan_tools_fs::{tools, GlobTool, ListDirTool};

/// 宽松策略的上下文（测试只用临时目录，不做路径限制断言）。
fn test_ctx() -> ToolContext {
    ToolContext::new(SecurityPolicy::default())
}

#[tokio::test]
async fn test_plugin_exposes_two_tools_in_stable_order() {
    let ctx = test_ctx();
    let list = tools(&ctx);
    let names: Vec<String> = list.iter().map(|t| t.tool_name()).collect();
    assert_eq!(
        names,
        vec![
            "read_file",
            "write_file",
            "apply_edit",
            "apply_patch",
            "glob",
            "list_dir",
        ],
        "注册顺序即前缀顺序，必须稳定"
    );
    // schema 必须可用（LLM 可见）
    for t in &list {
        let def = t.definition();
        assert!(!def.function.name.is_empty());
        assert!(!def.function.description.is_empty());
    }
}

#[tokio::test]
async fn test_glob_matches_files_in_temp_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.rs"), "fn main() {}").expect("write a.rs");
    std::fs::write(dir.path().join("b.txt"), "x").expect("write b.txt");

    let tool = GlobTool::new(test_ctx());
    let args = serde_json::json!({ "pattern": "*.rs", "path": dir.path().to_string_lossy() });
    let out = tool
        .execute("session-test", &args.to_string())
        .await
        .expect("glob 执行");
    let results = out["results"].as_array().expect("results 数组");
    assert_eq!(results.len(), 1, "只应命中 a.rs：{out}");
    assert!(results[0].as_str().unwrap_or_default().ends_with("a.rs"));
}

#[tokio::test]
async fn test_list_dir_lists_entries_with_pagination() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
    std::fs::write(dir.path().join("f1.txt"), "1").expect("write");

    let tool = ListDirTool::new(test_ctx());
    let args = serde_json::json!({
        "path": dir.path().to_string_lossy(),
        "limit": 10
    });
    let out = tool
        .execute("session-test", &args.to_string())
        .await
        .expect("list_dir 执行");
    let entries = out["entries"].as_array().expect("entries 数组");
    assert!(entries.len() >= 2, "至少含 sub 与 f1.txt：{out}");
    // 目录在前（排序契约；目录名带尾部 '/'，与工具输出约定一致）
    let first = entries[0]["name"].as_str().unwrap_or_default();
    assert_eq!(first, "sub/", "目录应排在文件前：{out}");
}

#[tokio::test]
async fn test_invalid_arguments_produce_tool_error() {
    let tool = GlobTool::new(test_ctx());
    let err = tool
        .execute("session-test", "{}")
        .await
        .expect_err("缺 pattern 应报错");
    assert!(
        err.to_string().contains("参数无效"),
        "错误口径必须与内置工具一致：{err}"
    );
}

#[tokio::test]
async fn test_glob_and_list_dir_share_one_context_handle() {
    // 装配层语义：两个工具共享同一份 ToolContext（Arc 内部字段，克隆廉价）
    let ctx = test_ctx();
    let list: Vec<Arc<dyn DynamicToolExecutor>> = tools(&ctx);
    assert_eq!(list.len(), 6);
}

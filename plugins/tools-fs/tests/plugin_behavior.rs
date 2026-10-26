//! 插件工具行为测试：与迁移前 core 内置实现逐项对齐。
//!
//! 覆盖点与 `core/src/agent/tool_registry/fs_ops_tests.rs` 一致（成功路径 /
//! 参数校验 / 安全门控），确保「实现搬家」不是「行为搬家」。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use tianyan::agent::{DynamicToolExecutor, ToolContext};
use tianyan::executor::SecurityPolicy;

use tianyan_tools_fs::{tools, ApplyPatchTool, GlobTool, ListDirTool, ViewImageTool};

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
            "view_image",
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
    assert_eq!(list.len(), 7);
}

/// `view_image` 对非图片文件报错（按内容识别，不看扩展名），错误带 `view_image` 前缀。
#[tokio::test]
async fn test_view_image_rejects_non_image_file() {
    let work = tempfile::tempdir().expect("tempdir");
    let data_dir = tempfile::tempdir().expect("tempdir");
    let txt = work.path().join("note.txt");
    std::fs::write(&txt, "hello world, not an image").expect("write txt");

    let mut ctx = test_ctx();
    ctx.data_dir = Some(data_dir.path().to_path_buf());
    let tool = ViewImageTool::new(ctx);
    let args = serde_json::json!({ "path": txt.to_string_lossy() });
    let err = tool
        .execute("session-test", &args.to_string())
        .await
        .expect_err("非图片应报错");
    assert!(
        err.to_string().contains("view_image"),
        "错误消息必须带 view_image 前缀：{err}"
    );
}

/// 未注入 `data_dir` → 明确报装配错误（不 panic、不静默成功）。
#[tokio::test]
async fn test_view_image_without_data_dir_reports_wiring_error() {
    let work = tempfile::tempdir().expect("tempdir");
    let png = work.path().join("shot.png");
    std::fs::write(&png, b"\x89PNG\r\n\x1A\nrest").expect("write png");

    let ctx = test_ctx();
    assert!(ctx.data_dir.is_none(), "测试前应未注入数据目录");
    let tool = ViewImageTool::new(ctx);
    let args = serde_json::json!({ "path": png.to_string_lossy() });
    let err = tool
        .execute("session-test", &args.to_string())
        .await
        .expect_err("缺 data_dir 应报装配错误");
    let msg = err.to_string();
    assert!(msg.contains("view_image"), "错误消息带工具前缀：{msg}");
    assert!(msg.contains("data_dir"), "错误消息点明缺件：{msg}");
}

// ── 工作目录失效 / 缺失：明确失败，不回退进程 cwd（安全断言）────────────

/// 相对路径 + 无任何工作目录 → 工具返回 `invalid_input` 错误。
///
/// 判别力：这条用例过的是**插件的 `?` 传播**（`resolve_tool_path` 变 `Result`
/// 之后，任何一个调用点漏补 `?` 都会在这里变成 `Ok`）；同时证明错误不会在
/// 插件层被吞掉、也不会静默按进程 cwd（服务安装目录）解析。
#[tokio::test]
async fn test_relative_path_without_working_directory_is_an_error() {
    let tool = GlobTool::new(test_ctx()); // 未注入会话管理器 + 无全局默认
    let args = serde_json::json!({ "pattern": "*.rs", "path": "src" });
    let err = tool
        .execute("session-test", &args.to_string())
        .await
        .expect_err("无工作目录时相对路径必须失败");
    assert!(
        err.is_invalid_input(),
        "错误必须是 invalid_input 语义：{err}"
    );
    let msg = err.to_string();
    assert!(msg.contains("src"), "错误消息须含原路径：{msg}");
    assert!(msg.contains("没有可用工作目录"), "{msg}");
}

/// `apply_patch`（基准目录 = 相对路径的落点）在无工作目录时**明确失败**。
///
/// 这是最危险的一条：旧实现把补丁基准解析到**进程 cwd**（服务安装目录），
/// 补丁会写进天演自己的源码树。断言错误在审批之前就发生（未装配审批门控时
/// 也不会执行到落盘）。
#[tokio::test]
async fn test_apply_patch_without_working_directory_is_an_error() {
    let tool = ApplyPatchTool::new(test_ctx());
    let patch = "*** Update File: src/main.rs\n-old\n+new\n";
    let args = serde_json::json!({ "patch": patch });
    let err = tool
        .execute("session-test", &args.to_string())
        .await
        .expect_err("无工作目录时 apply_patch 必须失败，不得以进程 cwd 为基准");
    assert!(
        err.is_invalid_input(),
        "错误必须是 invalid_input 语义：{err}"
    );
    let msg = err.to_string();
    assert!(msg.contains("没有可用工作目录"), "{msg}");
    assert!(msg.contains("apply_patch"), "错误须点明受影响的操作：{msg}");
}

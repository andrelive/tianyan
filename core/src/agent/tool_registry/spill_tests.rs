//! 工具输出 spill 监听器测试（ADR-047）。
//!
//! 判别力要点：落盘文件必须是**完整**原文（含被预览截掉的尾部），
//! 而非预览副本——否则"可回溯"承诺落空。

use std::path::PathBuf;

use serde_json::json;

use super::*;
use crate::agent::ToolRegistry;
use crate::common::types::{FunctionCall, ToolCallType};
use crate::executor::SecurityPolicy;

/// 构造工具调用（仅工具名参与判定）。
fn make_call(name: &str) -> ToolCall {
    ToolCall {
        id: "call_spill".into(),
        call_type: ToolCallType::Function,
        function: FunctionCall {
            name: name.into(),
            arguments: "{}".into(),
        },
    }
}

/// 构造行数可控的结果对象（序列化后必然超过小阈值）。
fn make_result(lines: usize) -> Value {
    let body: Vec<String> = (0..lines)
        .map(|i| format!("line-{i}-xxxxxxxxxxxxxxxxxxxx"))
        .collect();
    json!({ "content": body.join("\n") })
}

/// 配置后的监听器（小阈值便于构造超限）。
fn listener(dir: &std::path::Path, max_lines: usize, max_bytes: usize) -> ToolOutputSpillListener {
    let l = ToolOutputSpillListener::default();
    l.configure(
        dir.to_path_buf(),
        &ToolOutputConfig {
            enabled: true,
            max_lines,
            max_bytes,
            ..ToolOutputConfig::default()
        },
    );
    l
}

/// 超限结果 → 替换为 spilled 对象 + 完整原文落盘（判别力核心）。
#[tokio::test]
async fn test_oversized_result_spills_and_replaces() {
    let tmp = tempfile::tempdir().unwrap();
    let l = listener(tmp.path(), 5, 100);
    let original = make_result(50);
    let mut result: Result<Value, TianyanError> = Ok(original);
    l.on_post_execute(
        &make_call("grep"),
        "s",
        &mut result,
        std::time::Duration::ZERO,
    )
    .await;

    let value = result.expect("结果应为 Ok");
    assert_eq!(
        value["spilled"],
        json!(true),
        "超限结果应被替换为 spilled 对象"
    );
    assert!(value["preview"].is_string(), "应保留预览");
    let path = value["spill_path"].as_str().expect("应携带落盘路径");
    let spilled = std::fs::read_to_string(path).expect("落盘文件应存在");
    // 判别核心：落盘的是完整原文（含预览之外的尾部行）
    assert!(
        spilled.contains("line-49-"),
        "落盘文件应含被预览截掉的尾部内容（完整原文）"
    );
    assert_eq!(
        value["spill_bytes"].as_u64().unwrap() as usize,
        spilled.len(),
        "spill_bytes 应为完整内容字节数"
    );
    assert_eq!(
        value["spill_lines"].as_u64().unwrap() as usize,
        spilled.lines().count(),
        "spill_lines 应为完整内容行数"
    );
    assert!(value["note"].as_str().unwrap().contains("read_file"));
}

/// 预算内结果 → 原样返回，且不产生任何文件（常态零成本）。
#[tokio::test]
async fn test_within_budget_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let l = listener(tmp.path(), 2000, 50 * 1024);
    let original = make_result(10);
    let mut result: Result<Value, TianyanError> = Ok(original.clone());
    l.on_post_execute(
        &make_call("grep"),
        "s",
        &mut result,
        std::time::Duration::ZERO,
    )
    .await;
    assert_eq!(result.unwrap(), original, "预算内结果必须原样返回");
    let files = std::fs::read_dir(tmp.path()).unwrap().count();
    assert_eq!(files, 0, "未超预算不应落盘");
}

/// 白名单工具（read_file）超限也不改写（模型主动选的精确内容）。
#[tokio::test]
async fn test_skip_tool_not_spilled() {
    let tmp = tempfile::tempdir().unwrap();
    let l = listener(tmp.path(), 5, 100);
    let original = make_result(50);
    let mut result: Result<Value, TianyanError> = Ok(original.clone());
    l.on_post_execute(
        &make_call("read_file"),
        "s",
        &mut result,
        std::time::Duration::ZERO,
    )
    .await;
    assert_eq!(result.unwrap(), original, "白名单工具不得被改写");
}

/// execute_command 已有 log_file 机制 → 不由本监听器二次落盘。
#[tokio::test]
async fn test_execute_command_not_spilled() {
    let tmp = tempfile::tempdir().unwrap();
    let l = listener(tmp.path(), 5, 100);
    let original = make_result(50);
    let mut result: Result<Value, TianyanError> = Ok(original.clone());
    l.on_post_execute(
        &make_call("execute_command"),
        "s",
        &mut result,
        std::time::Duration::ZERO,
    )
    .await;
    assert_eq!(result.unwrap(), original);
}

/// 失败结果不改写（错误信息通常很小；改写会掩盖真实错误）。
#[tokio::test]
async fn test_error_result_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let l = listener(tmp.path(), 5, 100);
    let mut result: Result<Value, TianyanError> = Err(TianyanError::Custom("boom".into()));
    l.on_post_execute(
        &make_call("grep"),
        "s",
        &mut result,
        std::time::Duration::ZERO,
    )
    .await;
    assert!(result.is_err(), "错误结果必须原样保留");
}

/// enabled = false → 完全旁路（配置总开关）。
#[tokio::test]
async fn test_disabled_bypasses() {
    let tmp = tempfile::tempdir().unwrap();
    let l = ToolOutputSpillListener::default();
    l.configure(
        tmp.path().to_path_buf(),
        &ToolOutputConfig {
            enabled: false,
            max_lines: 5,
            max_bytes: 100,
            ..ToolOutputConfig::default()
        },
    );
    let original = make_result(50);
    let mut result: Result<Value, TianyanError> = Ok(original.clone());
    l.on_post_execute(
        &make_call("grep"),
        "s",
        &mut result,
        std::time::Duration::ZERO,
    )
    .await;
    assert_eq!(result.unwrap(), original, "enabled=false 应完全旁路");
}

/// 未注入目录（默认状态）→ 旁路：不落盘、不改写。
#[tokio::test]
async fn test_not_configured_bypasses() {
    let l = ToolOutputSpillListener::default();
    let original = make_result(500);
    let mut result: Result<Value, TianyanError> = Ok(original.clone());
    l.on_post_execute(
        &make_call("grep"),
        "s",
        &mut result,
        std::time::Duration::ZERO,
    )
    .await;
    assert_eq!(result.unwrap(), original, "未配置目录 = 不落盘");
}

/// 落盘失败（目录被文件占位）→ 保持原结果，不阻塞、不报错（best effort）。
#[tokio::test]
async fn test_spill_failure_keeps_original() {
    let tmp = tempfile::tempdir().unwrap();
    let blocker = tmp.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let l = listener(&blocker, 5, 100);
    let original = make_result(50);
    let mut result: Result<Value, TianyanError> = Ok(original.clone());
    l.on_post_execute(
        &make_call("grep"),
        "s",
        &mut result,
        std::time::Duration::ZERO,
    )
    .await;
    assert_eq!(result.unwrap(), original, "落盘失败必须保持原结果");
}

/// 注册顺序：spill 是第二个后置监听器（观测在前——数据层须见原始结果）。
#[test]
fn test_registry_registers_spill_listener_second() {
    let registry = ToolRegistry::new(SecurityPolicy::default());
    assert_eq!(
        registry.post_execute_listeners.len(),
        2,
        "应注册 [观测, spill] 两个后置监听器"
    );
    assert_eq!(
        registry.tool_output_spill.snapshot().1,
        None,
        "默认未注入目录 = 不落盘"
    );

    let registry = registry.with_tool_output(
        PathBuf::from("/tmp/tool_output"),
        &ToolOutputConfig {
            enabled: true,
            max_lines: 7,
            max_bytes: 42,
            ..ToolOutputConfig::default()
        },
    );
    let (enabled, dir, max_lines, max_bytes) = registry.tool_output_spill.snapshot();
    assert!(enabled);
    assert_eq!(dir, Some(PathBuf::from("/tmp/tool_output")));
    assert_eq!(max_lines, 7);
    assert_eq!(max_bytes, 42);
}

/// 阈值下限钳制：0 预算不得让一切结果都落盘（钳到 1）。
#[test]
fn test_zero_budget_clamped_to_one() {
    let l = ToolOutputSpillListener::default();
    l.configure(
        PathBuf::from("/tmp/x"),
        &ToolOutputConfig {
            enabled: true,
            max_lines: 0,
            max_bytes: 0,
            ..ToolOutputConfig::default()
        },
    );
    let (_, _, max_lines, max_bytes) = l.snapshot();
    assert_eq!(max_lines, 1);
    assert_eq!(max_bytes, 1);
}

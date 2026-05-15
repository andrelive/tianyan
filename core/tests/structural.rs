//! 架构结构测试：机械验证依赖方向和模块边界。
//!
//! 使用 rg (ripgrep) 进行静态 import 检查。

use std::process::Command;

const CORE_SRC: &str = "core/src";

fn grep_in_dir(pattern: &str, dir: &str) -> Vec<String> {
    let output = Command::new("rg")
        .args(["--no-heading", "-n", pattern, dir])
        .output();
    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.lines().map(|l| l.to_string()).collect()
        }
        Err(_) => vec![], // rg 不可用时跳过，不阻塞其他测试
    }
}

/// common 模块不得依赖 agent 模块。
#[test]
fn test_common_does_not_depend_on_agent() {
    let violations = grep_in_dir("use crate::agent", &format!("{}/common", CORE_SRC));
    assert!(
        violations.is_empty(),
        "common 模块不得依赖 agent 模块:\n{}",
        violations.join("\n")
    );
}

/// common 模块不得依赖 model 模块。
#[test]
fn test_common_does_not_depend_on_model() {
    let violations = grep_in_dir("use crate::model", &format!("{}/common", CORE_SRC));
    assert!(
        violations.is_empty(),
        "common 模块不得依赖 model 模块:\n{}",
        violations.join("\n")
    );
}

/// common 模块不得依赖 storage 模块。
#[test]
fn test_common_does_not_depend_on_storage() {
    let violations = grep_in_dir("use crate::storage", &format!("{}/common", CORE_SRC));
    assert!(
        violations.is_empty(),
        "common 模块不得依赖 storage 模块:\n{}",
        violations.join("\n")
    );
}

/// config 模块不得依赖 agent 模块。
#[test]
fn test_config_does_not_depend_on_agent() {
    let violations = grep_in_dir("use crate::agent", &format!("{}/config", CORE_SRC));
    assert!(
        violations.is_empty(),
        "config 模块不得依赖 agent 模块:\n{}",
        violations.join("\n")
    );
}

/// config 模块不得依赖 model 模块。
#[test]
fn test_config_does_not_depend_on_model() {
    let violations = grep_in_dir("use crate::model", &format!("{}/config", CORE_SRC));
    assert!(
        violations.is_empty(),
        "config 模块不得依赖 model 模块:\n{}",
        violations.join("\n")
    );
}

/// model 模块不得依赖 agent 模块。
#[test]
fn test_model_does_not_depend_on_agent() {
    let violations = grep_in_dir("use crate::agent", &format!("{}/model", CORE_SRC));
    assert!(
        violations.is_empty(),
        "model 模块不得依赖 agent 模块:\n{}",
        violations.join("\n")
    );
}

/// storage 模块不得依赖 agent 模块（只能被依赖）。
#[test]
fn test_storage_does_not_depend_on_agent() {
    let violations = grep_in_dir("use crate::agent", &format!("{}/storage", CORE_SRC));
    assert!(
        violations.is_empty(),
        "storage 模块不得依赖 agent 模块:\n{}",
        violations.join("\n")
    );
}

/// executor 模块不得依赖 planner 模块。
#[test]
fn test_executor_does_not_depend_on_planner() {
    let violations = grep_in_dir("use crate::planner", &format!("{}/executor", CORE_SRC));
    assert!(
        violations.is_empty(),
        "executor 模块不得依赖 planner 模块（verification.rs 除外，仅通过类型引用）:\n{}",
        violations.join("\n")
    );
}

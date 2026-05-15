//! Agent 产出验证门控。
//!
//! 在 Executor 执行完成后自动运行编译器/lint 检查，将失败信息
//! 转化为 Planner 可理解的 Step 反馈，实现自我调正。

use crate::common::error::Result;
use crate::planner::types::StepResult;
use serde::Serialize;
use serde_json::json;
use std::process::Command;

const MAX_LOOP_ITERATIONS: usize = 3;

/// 快速检查类型（毫秒~秒级）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastCheck {
    /// cargo check --message-format=json
    CompileCheck,
    /// cargo clippy --message-format=json
    ClippyCheck,
}

/// 单条编译/lint 错误。
#[derive(Debug, Clone, Serialize)]
pub struct VerificationIssue {
    pub file: String,
    pub message: String,
    pub severity: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerificationReport {
    pub passed: bool,
    pub issues: Vec<VerificationIssue>,
    pub summary: String,
}

/// 验证门控。
#[derive(Clone)]
pub struct VerificationGate {
    enabled: bool,
    fast_checks: Vec<FastCheck>,
    project_root: std::path::PathBuf,
    loop_count: usize,
}

impl VerificationGate {
    pub fn new(project_root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            enabled: true,
            fast_checks: vec![FastCheck::CompileCheck],
            project_root: project_root.into(),
            loop_count: 0,
        }
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_checks(mut self, checks: Vec<FastCheck>) -> Self {
        self.fast_checks = checks;
        self
    }

    /// 检查是否启用。
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 执行快速验证。
    pub async fn run_fast(&mut self) -> Result<VerificationReport> {
        if !self.enabled {
            return Ok(VerificationReport {
                passed: true,
                issues: vec![],
                summary: "验证门控未启用".to_string(),
            });
        }

        let mut all_issues = Vec::new();
        let mut all_passed = true;

        for check in &self.fast_checks {
            let (success, issues) = match check {
                FastCheck::CompileCheck => self.run_cargo_check().await,
                FastCheck::ClippyCheck => self.run_cargo_clippy().await,
            };
            if !success {
                all_passed = false;
            }
            all_issues.extend(issues);
        }

        let summary = if all_passed {
            "所有验证通过".to_string()
        } else {
            format!("发现 {} 个问题，请修复后重新提交", all_issues.len())
        };

        Ok(VerificationReport {
            passed: all_passed,
            issues: all_issues,
            summary,
        })
    }

    /// 将 VerificationReport 转换为 Planner 可理解的 StepResult。
    ///
    /// 失败时返回的 StepResult.success = false，
    /// output 中包含结构化的错误信息，便于 Planner 制定修复计划。
    pub fn to_step_result(report: &VerificationReport, step_id: usize) -> StepResult {
        if report.passed {
            StepResult {
                step_id,
                success: true,
                output: json!({"message": "验证通过", "issues": []}),
                error: None,
                actual_importance: None,
            }
        } else {
            let issues_json: Vec<serde_json::Value> = report
                .issues
                .iter()
                .map(|i| {
                    json!({
                        "file": i.file,
                        "severity": i.severity,
                        "message": i.message,
                    })
                })
                .collect();

            StepResult {
                step_id,
                success: false,
                output: json!({
                    "message": report.summary,
                    "issues": issues_json,
                    "suggestion": "请分析以上编译/lint 错误，制定修复计划。每个错误包含文件路径和具体信息。"
                }),
                error: Some(report.summary.clone()),
                actual_importance: None,
            }
        }
    }

    /// 检查是否达到最大循环次数。
    pub fn should_continue(&self) -> bool {
        self.loop_count < MAX_LOOP_ITERATIONS
    }

    /// 增加循环计数。
    pub fn increment_loop(&mut self) {
        self.loop_count += 1;
    }

    async fn run_cargo_check(&self) -> (bool, Vec<VerificationIssue>) {
        let output = Command::new("cargo")
            .args(["check", "--message-format=json"])
            .current_dir(&self.project_root)
            .output();

        let output = match output {
            Ok(o) => o,
            Err(e) => {
                return (
                    false,
                    vec![VerificationIssue {
                        file: String::new(),
                        message: format!("cargo check 启动失败: {}", e),
                        severity: "error".to_string(),
                    }],
                );
            }
        };

        let mut issues = Vec::new();
        let stdout = String::from_utf8_lossy(&output.stdout);

        for line in stdout.lines() {
            if let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(reason) = msg.get("reason").and_then(|r| r.as_str()) {
                    if reason == "compiler-message" {
                        if let Some(compiler_msg) = msg.get("message") {
                            let severity = compiler_msg
                                .get("level")
                                .and_then(|l| l.as_str())
                                .unwrap_or("unknown")
                                .to_string();

                            if severity == "error" || severity == "warning" {
                                let message = compiler_msg
                                    .get("rendered")
                                    .and_then(|r| r.as_str())
                                    .unwrap_or("")
                                    .to_string();

                                let file = compiler_msg
                                    .get("spans")
                                    .and_then(|spans| spans.get(0))
                                    .and_then(|span| span.get("file_name"))
                                    .and_then(|f| f.as_str())
                                    .unwrap_or("")
                                    .to_string();

                                issues.push(VerificationIssue {
                                    file,
                                    message,
                                    severity,
                                });
                            }
                        }
                    }
                }
            }
        }

        let has_errors = issues.iter().any(|i| i.severity == "error");
        (!has_errors, issues)
    }

    async fn run_cargo_clippy(&self) -> (bool, Vec<VerificationIssue>) {
        let output = Command::new("cargo")
            .args(["clippy", "--message-format=json"])
            .current_dir(&self.project_root)
            .output();

        let output = match output {
            Ok(o) => o,
            Err(e) => {
                return (
                    false,
                    vec![VerificationIssue {
                        file: String::new(),
                        message: format!("cargo clippy 启动失败: {}", e),
                        severity: "error".to_string(),
                    }],
                );
            }
        };

        let mut issues = Vec::new();
        let stdout = String::from_utf8_lossy(&output.stdout);

        for line in stdout.lines() {
            if let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(reason) = msg.get("reason").and_then(|r| r.as_str()) {
                    if reason == "compiler-message" {
                        if let Some(compiler_msg) = msg.get("message") {
                            let severity = compiler_msg
                                .get("level")
                                .and_then(|l| l.as_str())
                                .unwrap_or("unknown")
                                .to_string();

                            let message = compiler_msg
                                .get("rendered")
                                .and_then(|r| r.as_str())
                                .unwrap_or("")
                                .to_string();

                            let file = compiler_msg
                                .get("spans")
                                .and_then(|spans| spans.get(0))
                                .and_then(|span| span.get("file_name"))
                                .and_then(|f| f.as_str())
                                .unwrap_or("")
                                .to_string();

                            issues.push(VerificationIssue {
                                file,
                                message,
                                severity,
                            });
                        }
                    }
                }
            }
        }

        let has_errors = issues
            .iter()
            .any(|i| i.severity == "error" || i.severity == "failure-note");
        (!has_errors, issues)
    }
}

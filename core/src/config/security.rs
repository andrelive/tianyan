//! 安全配置模块。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 安全模式 — 决定命令/路径检查层如何处理危险操作。
///
/// 与审批层（[`ApprovalMode`]）正交：本枚举决定"检查层"的阻断/重写行为，
/// [`ApprovalMode`] 决定"审批层"的放行/询问行为（ADR-033）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyMode {
    /// 严格模式：命令元字符/解释器检查 + 黑名单全部生效。
    #[serde(alias = "deny")]
    Strict,
    /// 放宽模式（默认）：跳过元字符/解释器检查（允许链式命令与解释器执行，
    /// 便于智能体自主行动），**黑名单仍然强制生效**。
    #[default]
    Relaxed,
    /// 转换模式：将危险命令自动重写为安全等价操作
    /// （例如 rm → mv 到回收站目录，del → move 到回收站）。
    Transform,
    /// 完全放开模式：跳过一切检查（含黑名单）；用户自行承担风险。
    Permissive,
}

/// 审批行为模式（ADR-033：审批层的单一事实源）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// 全自主（默认）：黑名单外全放行（主/子代理一致）；审计记录。
    #[default]
    Autonomous,
    /// 确认式：危险操作拒绝并降级为"询问用户"链路。
    Confirm,
    /// 交互式：危险操作挂起等待 GUI 审批面板（子代理路径不允许等待，立即拒绝）。
    Interactive,
}

/// 安全配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// 启用安全功能。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 安全模式（命令/路径检查层）。
    #[serde(default)]
    pub safety_mode: SafetyMode,
    /// 审批行为模式（ADR-033；默认全自主）。
    #[serde(default)]
    pub approval_mode: ApprovalMode,
    /// 回收站目录（SafetyMode::Transform 时使用）。
    /// 默认为 `~/.tianyan/trash`。
    #[serde(default = "default_trash_dir")]
    pub trash_directory: PathBuf,
    /// 文件操作允许的目录。
    #[serde(default)]
    pub allowed_directories: Vec<PathBuf>,
    /// 文件操作禁止的目录。
    #[serde(default)]
    pub blocked_directories: Vec<PathBuf>,
    /// 允许执行的命令（自动放行）。
    #[serde(default)]
    pub allowed_commands: Vec<String>,
    /// 禁止执行的命令（硬拒绝；任何审批模式下 Deny 规则最优先）。
    #[serde(default)]
    pub blocked_commands: Vec<String>,
    /// "总是询问"命令列表：命中的命令强制走人工审批/询问，
    /// 不被自动审批规则与任何模式的自动放行覆盖。默认空（不强制）。
    #[serde(default)]
    pub prompt_commands: Vec<String>,
    /// 文件操作最大文件大小（字节）。
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,
    /// 启用审计日志。
    #[serde(default = "default_true")]
    pub audit_logging: bool,
    /// 已弃用（ADR-033）：改为 `approval_mode`。
    /// 仅保留读取兼容（`true` → 归一为 `Interactive`）；不再序列化写出。
    #[serde(default, skip_serializing, rename = "wait_for_approval")]
    pub(crate) deprecated_wait_for_approval: bool,
    /// 已弃用（ADR-033）：由 `approval_mode` 取代（`true` 等价 `Autonomous`）。
    /// 仅保留读取兼容；不再序列化写出。
    #[serde(default, skip_serializing, rename = "allow_all_operations")]
    pub(crate) deprecated_allow_all_operations: bool,
}

fn default_true() -> bool {
    true
}

fn default_max_file_size() -> u64 {
    100 * 1024 * 1024 // 100 MB
}

fn default_trash_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".tianyan")
        .join("trash")
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            safety_mode: SafetyMode::default(),
            approval_mode: ApprovalMode::default(),
            trash_directory: default_trash_dir(),
            allowed_directories: Vec::new(),
            blocked_directories: Vec::new(),
            allowed_commands: Vec::new(),
            blocked_commands: Vec::new(),
            prompt_commands: Vec::new(),
            max_file_size: default_max_file_size(),
            audit_logging: true,
            deprecated_wait_for_approval: false,
            deprecated_allow_all_operations: false,
        }
    }
}

impl SecurityConfig {
    /// 归一旧字段（ADR-033）。配置加载路径调用（与 `expand_user_dir` 同层）。
    ///
    /// 规则：
    /// - `wait_for_approval = true` → `approval_mode = interactive`（保留旧意图）；
    /// - `allow_all_operations = true` → 无动作（等价于默认 `autonomous`），仅记录日志；
    /// - `approval_mode` 非默认值（= 显式设置）时不覆盖，冲突旧字段忽略并告警。
    pub fn normalize(&mut self) {
        if self.deprecated_wait_for_approval {
            if self.approval_mode == ApprovalMode::Autonomous {
                self.approval_mode = ApprovalMode::Interactive;
                tracing::info!(
                    "配置归一（ADR-033）：wait_for_approval=true → approval_mode=interactive"
                );
            } else {
                tracing::warn!(
                    mode = ?self.approval_mode,
                    "旧字段 wait_for_approval 已弃用（ADR-033）且与显式 approval_mode 冲突，忽略旧字段"
                );
            }
        }
        if self.deprecated_allow_all_operations {
            // 命令检查层旧语义（跳过元字符/解释器检查）→ Relaxed。
            // 已是 Transform/Permissive（更放开）时不覆盖。
            if self.safety_mode == SafetyMode::Strict {
                self.safety_mode = SafetyMode::Relaxed;
                tracing::info!(
                    "配置归一（ADR-033）：allow_all_operations=true → safety_mode=relaxed（命令检查层）"
                );
            }
            tracing::info!(
                mode = ?self.approval_mode,
                "旧字段 allow_all_operations 已弃用（ADR-033；等价于 approval_mode=autonomous）"
            );
        }
    }

    /// 验证安全配置。
    pub fn validate(&self) -> Result<(), String> {
        if self.max_file_size == 0 {
            return Err("max_file_size 必须大于 0".to_string());
        }

        // 检查允许目录和禁止目录是否有冲突。
        for allowed in &self.allowed_directories {
            for blocked in &self.blocked_directories {
                if allowed.starts_with(blocked) || blocked.starts_with(allowed) {
                    return Err(format!(
                        "允许目录 {:?} 和禁止目录 {:?} 存在冲突",
                        allowed, blocked
                    ));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_security_config() {
        let config = SecurityConfig::default();
        assert!(config.enabled);
        assert!(config.audit_logging);
        assert_eq!(config.safety_mode, SafetyMode::Relaxed); // ADR-033 默认放宽（黑名单兜底）
        assert_eq!(config.approval_mode, ApprovalMode::Autonomous); // ADR-033 默认全自主
        assert_eq!(config.max_file_size, 100 * 1024 * 1024);
    }

    #[test]
    fn test_normalize_legacy_wait_for_approval_to_interactive() {
        let mut config: SecurityConfig =
            toml::from_str("wait_for_approval = true").expect("旧配置应可解析");
        config.normalize();
        assert_eq!(config.approval_mode, ApprovalMode::Interactive);
    }

    #[test]
    fn test_normalize_legacy_allow_all_relaxes_command_checks() {
        let mut config: SecurityConfig =
            toml::from_str("allow_all_operations = true").expect("旧配置应可解析");
        config.normalize();
        // 命令检查层：strict → relaxed（保留旧行为：跳过元字符/解释器，黑名单仍生效）
        assert_eq!(config.safety_mode, SafetyMode::Relaxed);
        // 审批层：默认全自主（与旧字段 true 的意图一致）
        assert_eq!(config.approval_mode, ApprovalMode::Autonomous);
    }

    #[test]
    fn test_normalize_explicit_mode_not_overridden() {
        let mut config: SecurityConfig =
            toml::from_str("approval_mode = \"confirm\"\nwait_for_approval = true")
                .expect("配置应可解析");
        config.normalize();
        // 显式 approval_mode 非默认：旧字段被忽略，不覆盖显式值
        assert_eq!(config.approval_mode, ApprovalMode::Confirm);
    }

    #[test]
    fn test_serialize_drops_deprecated_fields() {
        let mut parsed: SecurityConfig =
            toml::from_str("wait_for_approval = true\nallow_all_operations = true")
                .expect("旧配置应可解析");
        parsed.normalize();
        let out = toml::to_string_pretty(&parsed).expect("应可序列化");
        assert!(
            !out.contains("wait_for_approval"),
            "旧字段不应再写出：{out}"
        );
        assert!(
            !out.contains("allow_all_operations"),
            "旧字段不应再写出：{out}"
        );
        assert!(out.contains("approval_mode"), "新模式字段应写出：{out}");
        assert!(
            out.contains("interactive"),
            "归一结果应写入 approval_mode：{out}"
        );
    }

    #[test]
    fn test_security_validation() {
        let config = SecurityConfig::default();
        assert!(config.validate().is_ok());

        let mut invalid_config = config.clone();
        invalid_config.max_file_size = 0;
        assert!(invalid_config.validate().is_err());
    }
}

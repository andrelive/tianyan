//! 工具输出 spill 配置（ADR-047）。
//!
//! 工具结果一旦返回就进入会话历史，之后每轮请求都会重复携带（直到被压缩）。
//! 超预算的结果不再原样进入上下文，而是完整内容落盘、会话只保留预览 + 路径，
//! 需要细节时由模型用 `read_file` 分页读取。

use serde::{Deserialize, Serialize};

/// 工具输出落盘配置（`[tool_output]` 节）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutputConfig {
    /// 总开关（默认启用）。false = 完全旁路：工具结果原样返回，不做预算检查。
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// 单次工具结果行数上限：超过则完整内容落盘（默认 2000，对齐 opencode）。
    #[serde(default = "default_max_lines")]
    pub max_lines: usize,
    /// 单次工具结果字节上限：超过则完整内容落盘（默认 50 KiB，对齐 opencode）。
    #[serde(default = "default_max_bytes")]
    pub max_bytes: usize,
}

impl Default for ToolOutputConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            max_lines: default_max_lines(),
            max_bytes: default_max_bytes(),
        }
    }
}

fn default_enabled() -> bool {
    true
}

/// 默认行数上限。
///
/// 与 `executor::truncate::MAX_LINES` 一致（此处不复用常量：`config` 不得
/// 反向依赖 `executor`——依赖方向为 executor → config）。
/// 两者一致性由本模块单测 `test_defaults_match_truncate_limits` 守卫。
fn default_max_lines() -> usize {
    2000
}

/// 默认字节上限（50 KiB），与 `executor::truncate::MAX_BYTES` 一致。
fn default_max_bytes() -> usize {
    50 * 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 配置默认阈值必须与截断层常量一致（防两处漂移）。
    #[test]
    fn test_defaults_match_truncate_limits() {
        let cfg = ToolOutputConfig::default();
        assert_eq!(cfg.max_lines, crate::executor::truncate::MAX_LINES);
        assert_eq!(cfg.max_bytes, crate::executor::truncate::MAX_BYTES);
        assert!(cfg.enabled, "默认应启用");
    }

    /// 缺省 `[tool_output]` 节（旧配置）→ serde 默认值，行为不变。
    #[test]
    fn test_serde_defaults_when_section_absent() {
        let cfg: ToolOutputConfig = toml::from_str("").expect("空节应可反序列化");
        assert!(cfg.enabled);
        assert_eq!(cfg.max_lines, 2000);
        assert_eq!(cfg.max_bytes, 50 * 1024);
    }

    /// 部分字段：未写字段走默认，已写字段生效。
    #[test]
    fn test_serde_partial_section() {
        let cfg: ToolOutputConfig =
            toml::from_str("max_bytes = 1024\n").expect("部分节应可反序列化");
        assert!(cfg.enabled, "未写的 enabled 走默认");
        assert_eq!(cfg.max_lines, 2000, "未写的 max_lines 走默认");
        assert_eq!(cfg.max_bytes, 1024, "已写的 max_bytes 生效");
    }
}

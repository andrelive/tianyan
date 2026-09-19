//! 命令执行底层（shell provider）配置（ADR-037）。

use serde::{Deserialize, Serialize};

/// 执行器配置（`[executor]` 节）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorConfig {
    /// 命令执行底层：auto（默认，探测 pwsh→powershell / bash→sh）| pwsh |
    /// powershell | cmd | bash | sh | custom。
    ///
    /// **切换的提示词后果（明示）**：shell 事实注入 `execute_command` 工具
    /// 描述——切换使工具表指纹变化 → 下一轮重建请求前缀（一次缓存未命中）
    /// + 一次主动压缩（消息足够时）。低频操作，可接受（ADR-037）。
    #[serde(default = "default_shell")]
    pub shell: String,
    /// 可执行程序（名或绝对路径）：
    /// - 内置 provider：可选（覆盖默认程序——如 pwsh 不在 PATH 时指定绝对路径）；
    /// - custom：必填。
    #[serde(default)]
    pub shell_program: Option<String>,
    /// custom 的参数模板（命令串追加在最后）；缺省 `["-c"]`。
    #[serde(default)]
    pub shell_args: Option<Vec<String>>,
    /// custom 的提示词（可选；缺省自动生成）。
    #[serde(default)]
    pub shell_hint: Option<String>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            shell: default_shell(),
            shell_program: None,
            shell_args: None,
            shell_hint: None,
        }
    }
}

fn default_shell() -> String {
    "auto".to_string()
}

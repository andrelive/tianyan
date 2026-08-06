use crate::common::error::TianyanError;
use crate::config::{SafetyMode, SecurityConfig};
use crate::executor::command::{extract_command_base, DEFAULT_COMMAND_TIMEOUT_SECS};

/// Executor 安全策略。
#[derive(Debug, Clone)]
pub struct SecurityPolicy {
    /// 安全模式（strict / transform / permissive）。
    pub safety_mode: SafetyMode,
    /// 回收站目录（transform 模式下 rm/del 命令的目标路径）。
    pub trash_directory: std::path::PathBuf,
    /// 允许的命令白名单。若为 None，则允许所有不在黑名单中的命令。
    pub allowed_commands: Option<Vec<String>>,
    /// 禁止的命令黑名单。黑名单优先于白名单。
    pub blocked_commands: Vec<String>,
    /// 文件操作允许的目录白名单。空 = 不限制。
    pub allowed_directories: Vec<std::path::PathBuf>,
    /// 文件操作禁止的目录黑名单。黑名单优先。
    pub blocked_directories: Vec<std::path::PathBuf>,
    /// 是否允许写入文件。
    pub allow_file_write: bool,
    /// 命令执行超时时间（秒）。
    pub max_command_timeout_secs: u64,
    /// 单次文件读写最大大小（字节）。
    pub max_file_size: u64,
    /// 是否允许通过解释器执行命令。
    pub block_interpreters: bool,
}

impl SecurityPolicy {
    /// 从用户安全配置创建 SecurityPolicy。
    pub fn from_config(config: &SecurityConfig) -> Self {
        let blocked = if config.blocked_commands.is_empty() {
            vec![
                "rm".to_string(),
                "del".to_string(),
                "format".to_string(),
                "rmdir".to_string(),
                "rd".to_string(),
                "shutdown".to_string(),
                "taskkill".to_string(),
            ]
        } else {
            config.blocked_commands.clone()
        };

        Self {
            safety_mode: config.safety_mode,
            trash_directory: config.trash_directory.clone(),
            allowed_commands: if config.allowed_commands.is_empty() {
                None
            } else {
                Some(config.allowed_commands.clone())
            },
            blocked_commands: blocked,
            allowed_directories: config.allowed_directories.clone(),
            blocked_directories: config.blocked_directories.clone(),
            allow_file_write: true,
            max_command_timeout_secs: DEFAULT_COMMAND_TIMEOUT_SECS,
            max_file_size: config.max_file_size,
            block_interpreters: config.safety_mode != SafetyMode::Permissive,
        }
    }

    /// 尝试将危险命令转换为安全等价操作。
    ///
    /// 当前支持的转换：
    /// - `rm -rf <path>` → `mv <path> <trash_dir>/<timestamp>/`
    /// - `rm <path>` → `mv <path> <trash_dir>/<timestamp>/`
    /// - `del /f <path>` (Windows) → `move <path> <trash_dir>/<timestamp>/`
    /// - `rmdir <path>` → `mv <path> <trash_dir>/<timestamp>/`
    ///
    /// 返回 `Some(transformed_command)` 如果可以转换，`None` 表示无法安全转换。
    pub fn transform_command(&self, command: &str) -> Option<String> {
        if self.safety_mode != SafetyMode::Transform {
            return None;
        }

        let cmd_name = command
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_lowercase();

        // Only transform explicitly dangerous commands
        let transformable = matches!(cmd_name.as_str(), "rm" | "rmdir" | "del" | "rd");
        if !transformable {
            return None;
        }

        // Extract the path argument(s). For simplicity, handle the common cases:
        //   rm <path>
        //   rm -rf <path>
        //   rm -r <path>
        //   rm --force <path>
        //   del /f /q <path>
        let parts: Vec<&str> = command.split_whitespace().collect();
        if parts.len() < 2 {
            return None; // Nothing to delete — refuse
        }

        // Collect non-flag arguments as paths
        let paths: Vec<&str> = parts[1..]
            .iter()
            .filter(|p| !p.starts_with('-') && !p.starts_with('/'))
            .copied()
            .collect();

        if paths.is_empty() {
            return None; // All arguments were flags — too dangerous to transform
        }

        // Create a timestamped subdirectory in trash to avoid collisions
        let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
        let trash_target = self.trash_directory.join(format!("{}_{}", ts, cmd_name));

        let mut result = String::new();
        if cfg!(target_os = "windows") {
            result.push_str("mkdir ");
            result.push_str(&trash_target.to_string_lossy());
            for path in &paths {
                result.push_str(" && move ");
                result.push_str(path);
                result.push(' ');
                result.push_str(&trash_target.to_string_lossy());
                result.push('\\');
            }
        } else {
            result.push_str("mkdir -p ");
            result.push_str(&trash_target.to_string_lossy());
            for path in &paths {
                result.push_str(" && mv ");
                result.push_str(path);
                result.push(' ');
                result.push_str(&trash_target.to_string_lossy());
                result.push('/');
            }
        }

        tracing::info!(
            original = %command,
            transformed = %result,
            "命令已转换为安全等价操作"
        );
        Some(result)
    }

    /// 检查文件路径是否在允许/禁止目录范围内。
    ///
    /// - `blocked_directories` 优先：路径在任一禁止目录下则拒绝。
    /// - `allowed_directories` 不为空时：路径必须在某一允许目录下。
    /// - Permissive 模式下跳过所有检查。
    pub fn check_path(&self, path: &std::path::Path) -> Result<(), TianyanError> {
        if self.safety_mode == SafetyMode::Permissive {
            return Ok(());
        }

        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        // Blocked directories first (highest priority)
        for blocked in &self.blocked_directories {
            if canonical.starts_with(blocked) {
                return Err(TianyanError::Custom(format!(
                    "executor: 安全策略违规：路径在黑名单目录中：{} (禁止：{})",
                    path.display(),
                    blocked.display()
                )));
            }
        }

        // If allowed list is set, path must be under an allowed directory
        if !self.allowed_directories.is_empty() {
            let allowed = self
                .allowed_directories
                .iter()
                .any(|d| canonical.starts_with(d));
            if !allowed {
                return Err(TianyanError::Custom(format!(
                    "executor: 安全策略违规：路径不在白名单目录中：{}",
                    path.display()
                )));
            }
        }

        Ok(())
    }

    /// 检查文件大小是否在限制内。
    pub fn check_file_size(&self, size: u64) -> Result<(), TianyanError> {
        if size > self.max_file_size {
            return Err(TianyanError::Custom(format!(
                "executor: 安全策略违规：文件大小 {} 超出限制 {} 字节",
                size, self.max_file_size
            )));
        }
        Ok(())
    }
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self::from_config(&SecurityConfig::default())
    }
}

impl SecurityPolicy {
    /// 检查命令是否被允许执行。
    pub fn check_command(&self, command: &str) -> Result<(), TianyanError> {
        // Permissive mode: skip all checks
        if self.safety_mode == SafetyMode::Permissive {
            return Ok(());
        }

        // 检测命令链和命令替换元字符，防止注入
        if Self::has_shell_metacharacters(command) {
            return Err(TianyanError::Custom(
                "executor: 安全策略违规：命令包含不被允许的 shell 元字符（&&、||、;、`、$() 等）"
                    .to_string(),
            ));
        }

        let cmd_name = command.split_whitespace().next().unwrap_or(command);
        let pure_name = extract_command_base(cmd_name);

        if self.block_interpreters {
            let interpreters = ["cmd", "powershell", "pwsh", "bash", "sh", "wsl", "wsl.exe"];
            if interpreters.contains(&pure_name.as_str()) {
                return Err(TianyanError::Custom(format!(
                    "executor: 安全策略违规：禁止通过解释器执行命令：{}",
                    cmd_name
                )));
            }
        }

        for blocked in &self.blocked_commands {
            let blocked_lower = blocked.to_lowercase();
            if pure_name == blocked_lower {
                return Err(TianyanError::Custom(format!(
                    "executor: 安全策略违规：命令被安全策略禁止：{}",
                    cmd_name
                )));
            }
        }
        if let Some(allowed) = &self.allowed_commands {
            let is_allowed = allowed.iter().any(|a| a.to_lowercase() == pure_name);
            if !is_allowed {
                return Err(TianyanError::Custom(format!(
                    "executor: 安全策略违规：命令不在白名单中：{}",
                    cmd_name
                )));
            }
        }
        Ok(())
    }

    /// 检测命令中是否包含命令链或命令替换元字符。
    ///
    /// 阻止的模式：`&&`（命令链）、`||`（条件链）、`;`（分隔符）、
    /// `` ` ``（反引号替换）、`$(`（命令替换）。
    /// 管道 `|` 和重定向 `>` `<` 不在阻止范围内，因为它们是合法的
    /// 单命令操作。
    fn has_shell_metacharacters(command: &str) -> bool {
        command.contains("&&")
            || command.contains("||")
            || command.contains(';')
            || command.contains('`')
            || command.contains("$(")
    }

    /// 检查文件写入是否被允许。
    pub fn check_file_write(&self) -> Result<(), TianyanError> {
        if self.allow_file_write {
            Ok(())
        } else {
            Err(TianyanError::Custom(
                "executor: 安全策略违规：文件写入被安全策略禁止".to_string(),
            ))
        }
    }
}

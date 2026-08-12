use crate::common::error::TianyanError;
use crate::config::{SafetyMode, SecurityConfig};
use crate::executor::command::{extract_command_base, DEFAULT_COMMAND_TIMEOUT_SECS};

/// 默认禁止命令——工具路径（[`SecurityPolicy`]）与技能路径（`ExecutorConfig`）
/// 共享的单一默认源（空配置时的兜底）。与工具侧既有默认一致，技能侧
/// 此前硬编码的更宽列表（python/curl 等）已收敛到本单一来源。
pub const DEFAULT_BLOCKED_COMMANDS: &[&str] =
    &["rm", "del", "format", "rmdir", "rd", "shutdown", "taskkill"];

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
            DEFAULT_BLOCKED_COMMANDS
                .iter()
                .map(|s| s.to_string())
                .collect()
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
    ///
    /// 判定委托给共享的 [`check_path_rules`]，错误消息逐字保持。
    pub fn check_path(&self, path: &std::path::Path) -> Result<(), TianyanError> {
        if self.safety_mode == SafetyMode::Permissive {
            return Ok(());
        }
        match check_path_rules(path, &self.allowed_directories, &self.blocked_directories) {
            PathCheckOutcome::Allowed => Ok(()),
            PathCheckOutcome::Blocked(blocked) => Err(TianyanError::Custom(format!(
                "executor: 安全策略违规：路径在黑名单目录中：{} (禁止：{})",
                path.display(),
                blocked.display()
            ))),
            PathCheckOutcome::NotAllowed => Err(TianyanError::Custom(format!(
                "executor: 安全策略违规：路径不在白名单目录中：{}",
                path.display()
            ))),
        }
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

/// 路径沙箱判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PathCheckOutcome {
    /// 放行。
    Allowed,
    /// 命中黑名单目录（携带命中的目录，供调用方格式化错误消息）。
    Blocked(std::path::PathBuf),
    /// 白名单非空且路径不在任一白名单目录下。
    NotAllowed,
}

/// 规范化路径用于比较：Windows 上 `canonicalize()` 会产生 `\\?\` verbatim 前缀，
/// 与未规范化的路径（如 `current_dir().join(path)` 的 fallback 结果）直接比较
/// `starts_with` 会失败，导致允许目录内的写入/删除被误拒。
///
/// 从 `skills/executor.rs` 移入，作为技能 `validate_path` 与 `SecurityPolicy::check_path`
/// 两套路径沙箱共享的底层规范化。
pub(crate) fn normalize_path_for_check(p: &std::path::Path) -> std::path::PathBuf {
    let s = p.to_string_lossy();
    let s = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.to_string()
    };
    std::path::PathBuf::from(s)
}

/// 统一路径沙箱判定（黑名单绝对优先），供技能 `validate_path` 与 `SecurityPolicy::check_path`
/// 共用，消除两套语义不一致的路径校验实现：
///
/// 1. 路径与目录均规范化（`canonicalize` + Windows verbatim 前缀剥离）后做组件级前缀匹配；
/// 2. 命中任一黑名单目录 → `Blocked`（黑名单绝对优先）；
/// 3. 白名单非空且路径不在任一白名单目录下 → `NotAllowed`；
/// 4. 其余 → `Allowed`（含白名单为空 = 放行）。
///
/// 规范化失败回退：路径回退到 `current_dir().join(path)`（保留相对路径语义，与
/// `validate_path` 既有行为一致），目录回退原值。
pub(crate) fn check_path_rules(
    path: &std::path::Path,
    allowed: &[std::path::PathBuf],
    blocked: &[std::path::PathBuf],
) -> PathCheckOutcome {
    let canonical = resolve_canonical(path);
    for dir in blocked {
        if canonical.starts_with(normalize_dir_for_check(dir)) {
            return PathCheckOutcome::Blocked(dir.clone());
        }
    }
    if !allowed.is_empty() {
        let hit = allowed
            .iter()
            .any(|d| canonical.starts_with(normalize_dir_for_check(d)));
        if !hit {
            return PathCheckOutcome::NotAllowed;
        }
    }
    PathCheckOutcome::Allowed
}

/// 解析路径为规范化形式：`canonicalize` 失败回退 `current_dir().join(path)`（再回退原值），
/// 并剥离 Windows verbatim 前缀。
fn resolve_canonical(path: &std::path::Path) -> std::path::PathBuf {
    let resolved = path.canonicalize().unwrap_or_else(|_| {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    });
    normalize_path_for_check(&resolved)
}

/// 规范化目录用于前缀比较：`canonicalize` 失败回退原值，并剥离 Windows verbatim 前缀。
fn normalize_dir_for_check(dir: &std::path::Path) -> std::path::PathBuf {
    normalize_path_for_check(&dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()))
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
    /// `&`（Windows cmd 分隔符 / Unix 后台符）、`` ` ``（反引号替换）、
    /// `$(`（命令替换）。
    /// 管道 `|` 和重定向 `>` `<` 不在阻止范围内，因为它们是合法的
    /// 单命令操作。
    fn has_shell_metacharacters(command: &str) -> bool {
        command.contains("&&")
            || command.contains("||")
            || command.contains(';')
            || command.contains('&')
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;

    /// 构造带白/黑名单的严格模式策略（目录不预规范化，模拟配置原始值）。
    fn policy_with(allowed: Vec<PathBuf>, blocked: Vec<PathBuf>) -> SecurityPolicy {
        SecurityPolicy {
            safety_mode: SafetyMode::Strict,
            trash_directory: std::env::temp_dir(),
            allowed_commands: None,
            blocked_commands: Vec::new(),
            allowed_directories: allowed,
            blocked_directories: blocked,
            allow_file_write: true,
            max_command_timeout_secs: 30,
            max_file_size: 1024 * 1024,
            block_interpreters: true,
        }
    }

    // ── normalize_path_for_check ─────────────────────────────────────────

    #[test]
    fn test_normalize_path_for_check_strips_verbatim_prefix() {
        let win = PathBuf::from(r"\\?\C:\Users\me\dir");
        assert_eq!(
            normalize_path_for_check(&win),
            PathBuf::from(r"C:\Users\me\dir")
        );
        let unc = PathBuf::from(r"\\?\UNC\server\share");
        assert_eq!(
            normalize_path_for_check(&unc),
            PathBuf::from(r"\\server\share")
        );
        let plain = PathBuf::from(r"C:\Users\me\dir");
        assert_eq!(normalize_path_for_check(&plain), plain);
    }

    // ── check_path_rules：allowlist 语义 ─────────────────────────────────

    #[test]
    fn test_check_path_rules_allows_inside_allowed() {
        let dir = tempdir().unwrap();
        let inside = dir.path().join("sub").join("file.txt");
        assert_eq!(
            check_path_rules(&inside, &[dir.path().to_path_buf()], &[]),
            PathCheckOutcome::Allowed
        );
    }

    #[test]
    fn test_check_path_rules_rejects_outside_allowed() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let p = outside.path().join("file.txt");
        assert_eq!(
            check_path_rules(&p, &[dir.path().to_path_buf()], &[]),
            PathCheckOutcome::NotAllowed
        );
    }

    #[test]
    fn test_check_path_rules_rejects_prefix_sibling() {
        // 允许目录 /ab 时，/abc 不得被视为其子路径
        let dir = tempdir().unwrap();
        let base = dir.path().join("ab");
        let sibling = dir.path().join("abc");
        std::fs::create_dir(&base).unwrap();
        std::fs::create_dir(&sibling).unwrap();
        assert_eq!(
            check_path_rules(&base.join("x.txt"), std::slice::from_ref(&base), &[]),
            PathCheckOutcome::Allowed
        );
        assert_eq!(
            check_path_rules(&sibling.join("x.txt"), &[base], &[]),
            PathCheckOutcome::NotAllowed
        );
    }

    #[test]
    fn test_check_path_rules_empty_allowlist_is_unrestricted() {
        let p = PathBuf::from("/anywhere");
        assert_eq!(check_path_rules(&p, &[], &[]), PathCheckOutcome::Allowed);
    }

    #[test]
    fn test_check_path_rules_absolute_path_matches_allowed() {
        // 路径（即使不存在）解析为绝对路径后应能匹配绝对白名单目录
        let dir = tempdir().unwrap();
        let abs = dir.path().join("sub").join("new.txt");
        assert_eq!(
            check_path_rules(&abs, &[dir.path().to_path_buf()], &[]),
            PathCheckOutcome::Allowed
        );
    }

    // ── check_path_rules：blacklist 语义 ─────────────────────────────────

    #[test]
    fn test_check_path_rules_rejects_blocked_dir() {
        let dir = tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        let path = blocked.join("x.txt");
        std::fs::write(&path, "x").unwrap();
        assert_eq!(
            check_path_rules(&path, &[], std::slice::from_ref(&blocked)),
            PathCheckOutcome::Blocked(blocked.clone())
        );
    }

    #[test]
    fn test_check_path_rules_rejects_nested_in_blocked_dir() {
        let dir = tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::create_dir_all(blocked.join("deep")).unwrap();
        let path = blocked.join("deep").join("nested.txt");
        std::fs::write(&path, "x").unwrap();
        assert!(matches!(
            check_path_rules(&path, &[], &[blocked]),
            PathCheckOutcome::Blocked(_)
        ));
    }

    #[test]
    fn test_check_path_rules_blacklist_priority_over_allowlist() {
        // 路径同时在白名单与黑名单内：黑名单绝对优先
        let dir = tempdir().unwrap();
        let shared = dir.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let path = shared.join("x.txt");
        std::fs::write(&path, "x").unwrap();
        assert_eq!(
            check_path_rules(
                &path,
                std::slice::from_ref(&shared),
                std::slice::from_ref(&shared)
            ),
            PathCheckOutcome::Blocked(shared.clone())
        );
    }

    #[test]
    fn test_check_path_rules_allows_when_not_blocked_and_in_allowlist() {
        let dir = tempdir().unwrap();
        let allowed = dir.path().join("allowed");
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&allowed).unwrap();
        std::fs::create_dir(&blocked).unwrap();
        let path = allowed.join("x.txt");
        std::fs::write(&path, "x").unwrap();
        assert_eq!(
            check_path_rules(&path, &[allowed], &[blocked]),
            PathCheckOutcome::Allowed
        );
    }

    // ── SecurityPolicy::check_path 行为锁定 ──────────────────────────────

    #[test]
    fn test_check_path_rejects_blocked_with_message() {
        let dir = tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        let path = blocked.join("x.txt");
        std::fs::write(&path, "x").unwrap();
        let err = policy_with(vec![], vec![blocked.clone()])
            .check_path(&path)
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("executor: 安全策略违规：路径在黑名单目录中"));
        assert!(msg.contains(&format!("(禁止：{})", blocked.display())));
    }

    #[test]
    fn test_check_path_rejects_outside_allowlist_with_message() {
        let dir = tempdir().unwrap();
        let allowed = dir.path().join("allowed");
        std::fs::create_dir(&allowed).unwrap();
        let outside = dir.path().join("secret.txt");
        std::fs::write(&outside, "x").unwrap();
        let err = policy_with(vec![allowed], vec![])
            .check_path(&outside)
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("executor: 安全策略违规：路径不在白名单目录中"));
    }

    #[test]
    fn test_check_path_empty_lists_unrestricted() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("x.txt");
        assert!(policy_with(vec![], vec![]).check_path(&p).is_ok());
    }

    #[test]
    fn test_check_path_blocked_priority_over_allowed() {
        let dir = tempdir().unwrap();
        let shared = dir.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let path = shared.join("x.txt");
        std::fs::write(&path, "x").unwrap();
        let err = policy_with(vec![shared.clone()], vec![shared.clone()])
            .check_path(&path)
            .unwrap_err();
        assert!(err.to_string().contains("黑名单"));
    }

    #[test]
    fn test_check_path_permissive_skips_all_checks() {
        let dir = tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        let path = blocked.join("x.txt");
        std::fs::write(&path, "x").unwrap();
        let mut policy = policy_with(vec![], vec![blocked]);
        policy.safety_mode = SafetyMode::Permissive;
        assert!(policy.check_path(&path).is_ok());
    }
}

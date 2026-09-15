use crate::common::error::TianyanError;
use crate::config::{SafetyMode, SecurityConfig};
use crate::executor::command::{extract_command_base, DEFAULT_COMMAND_TIMEOUT_SECS};

/// 默认禁止命令——工具路径（[`SecurityPolicy`]）的单一默认源（空配置时的兜底）。
pub const DEFAULT_BLOCKED_COMMANDS: &[&str] =
    &["rm", "del", "format", "rmdir", "rd", "shutdown", "taskkill"];

/// 危险命令（审批层 Critical 定级）：执行前必须经用户确认。
///
/// 与 [`DEFAULT_BLOCKED_COMMANDS`] 有交集（rm/del/format 双重约束）：
/// blocked 是硬拒绝（check_command 拦截），dangerous 是审批定级——
/// 用户自定义黑名单移除某命令后，危险定级仍然生效。
pub const DANGEROUS_COMMANDS: &[&str] = &["rm", "del", "format", "fdisk", "mkfs", "dd"];

/// 中等风险命令（审批层 Medium 定级）：通常需用户确认。
pub const MODERATE_COMMANDS: &[&str] = &["git", "cargo", "npm", "pip", "docker"];

/// 命令风险定级（审批层与执行层的单一事实源）。
///
/// 输入为 [`crate::executor::command::extract_command_base`] 归一后的命令名
/// （小写、无路径/后缀）；分类顺序：Blocked 优先于 Dangerous/Moderate。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRisk {
    /// 完全禁止（check_command 硬拒绝）。
    Blocked,
    /// 危险命令（审批 Critical）。
    Dangerous,
    /// 中等风险（审批 Medium）。
    Moderate,
    /// 低风险（审批 Low）。
    Low,
}

/// 命令风险定级：内置清单 + 默认黑名单的静态单一事实源。
///
/// 用户自定义黑名单（`SecurityPolicy::blocked_commands`）的判定在
/// [`SecurityPolicy::check_command`]（实例方法，含多词前缀匹配）；
/// 本函数负责内置清单部分，审批层与执行层共用，禁止在别处复制清单。
pub fn classify_command_risk(cmd_name: &str) -> CommandRisk {
    if DEFAULT_BLOCKED_COMMANDS.contains(&cmd_name) {
        return CommandRisk::Blocked;
    }
    if DANGEROUS_COMMANDS.contains(&cmd_name) {
        return CommandRisk::Dangerous;
    }
    if MODERATE_COMMANDS.contains(&cmd_name) {
        return CommandRisk::Moderate;
    }
    CommandRisk::Low
}

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
            // ADR-033：Relaxed/Permissive 跳过解释器检查（Strict/Transform 检查）。
            block_interpreters: matches!(
                config.safety_mode,
                SafetyMode::Strict | SafetyMode::Transform
            ),
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

        // 命令名统一经 extract_command_base 归一（小写、无路径/后缀），
        // 与 check_command / classify_command_risk 同一匹配语义。
        let cmd_name = extract_command_base(command);

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

/// 词法归一化路径：折叠 `.` 与 `..` 组件（纯字符串语义，不访问文件系统）。
///
/// 供 `canonicalize` 失败的 fallback 使用——目标不存在时 `..` 也必须折叠，
/// 否则前缀匹配会被 `sub/../..` 之类的路径欺骗（T0-1：白名单逃逸 /
/// 黑名单漏报）。有根路径的 `..` 不越过根/盘符（`C:\..` = `C:\`、
/// `/..` = `/`），无根相对路径越界 `..` 保留（`../` 语义）。
pub(crate) fn lexical_normalize(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;

    let mut out = std::path::PathBuf::new();
    // 用 has_root（而非 is_absolute）：Windows 上 `/xx` 有根但无盘符
    // （is_absolute=false），`..` 越过根仍需折叠为根。
    let has_root = path.has_root();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::ParentDir) => out.push(comp.as_os_str()),
                // 根/盘符之上（或空栈）：有根路径忽略，无根路径保留
                _ => {
                    if !has_root {
                        out.push(comp.as_os_str());
                    }
                }
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
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
        // Windows 上 POSIX 根（如 "/"）会被 canonicalize 解析为当前盘根（如 F:\\），
        // 导致整盘被黑名单误拒；此类条目在本平台无对应语义，跳过。
        if is_meaningless_blocked_dir(dir) {
            continue;
        }
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

/// 解析路径为规范化形式（沙箱判定的路径侧口径）：
///
/// 1. 全路径 `canonicalize` 成功 → 物理解析结果（跟随符号链接）；
/// 2. 失败（目标不存在，如写新文件）→ 先词法折叠 `.`/`..`，再对**最近的
///    已存在祖先**做物理解析后拼接剩余组件——`..` 与符号链接都无法欺骗
///    前缀匹配（T0-1：修复前只做 `cwd.join(path)` 不折叠 `..`，
///    `sub/../..` 可逃出白名单 / 混入黑名单）。
fn resolve_canonical(path: &std::path::Path) -> std::path::PathBuf {
    if let Ok(c) = path.canonicalize() {
        return normalize_path_for_check(&c);
    }

    let absolute = std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf());
    let lexical = lexical_normalize(&absolute);

    // 从完整路径向上找第一个可物理解析的祖先，拼接剩余组件。
    let mut probe: &std::path::Path = &lexical;
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(base) = probe.canonicalize() {
            let mut out = normalize_path_for_check(&base);
            for name in rest.iter().rev() {
                out.push(name);
            }
            return out;
        }
        match (probe.parent(), probe.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                probe = parent;
            }
            // 无父级（盘根/根）或无名组件：退回词法结果（异常形态兜底）
            _ => return lexical,
        }
    }
}

/// 规范化目录用于前缀比较：`canonicalize` 失败回退**词法归一化**原值
/// （配置目录含 `.`/`..` 时保持可比较），并剥离 Windows verbatim 前缀。
fn normalize_dir_for_check(dir: &std::path::Path) -> std::path::PathBuf {
    normalize_path_for_check(
        &dir.canonicalize()
            .unwrap_or_else(|_| lexical_normalize(dir)),
    )
}

/// 黑名单目录条目在本平台是否无意义（应跳过）。
///
/// Windows 上 POSIX 根路径（如 `"/"`）无对应目录语义，且 `canonicalize`
/// 会把它解析为**当前驱动器根**（如 `F:\\`），前缀匹配将拒绝整个盘——
/// 配置从 POSIX 迁移而来时必然误伤。仅含根组件的条目直接跳过。
#[cfg(target_os = "windows")]
fn is_meaningless_blocked_dir(dir: &std::path::Path) -> bool {
    matches!(
        dir.components().collect::<Vec<_>>().as_slice(),
        [std::path::Component::RootDir]
    )
}

/// 非 Windows 平台无此问题（"/" 就是根，禁止根是合法意图）。
#[cfg(not(target_os = "windows"))]
fn is_meaningless_blocked_dir(_dir: &std::path::Path) -> bool {
    false
}

impl SecurityPolicy {
    /// 检查命令是否被允许执行。
    pub fn check_command(&self, command: &str) -> Result<(), TianyanError> {
        // Permissive mode: skip all checks
        if self.safety_mode == SafetyMode::Permissive {
            return Ok(());
        }

        // 检测命令链和命令替换元字符，防止注入
        // （放宽模式跳过：仅保留黑名单兜底）
        if self.safety_mode != SafetyMode::Relaxed && Self::has_shell_metacharacters(command) {
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
            // 匹配语义：单词条目按命令名精确匹配（兼容既有行为）；
            // 多词条目（如 `rm -rf /`、`del /s`）按整条命令前缀匹配——
            // 使"全目录删除"类黑名单真正生效（此前仅首词比较导致永不命中）。
            let hit = if blocked_lower.contains(' ') {
                command
                    .to_lowercase()
                    .trim_start()
                    .starts_with(&blocked_lower)
            } else {
                pure_name == blocked_lower
            };
            if hit {
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
    /// 阻止的模式：`&&`（命令链）、`||`（条件链）、`` ` ``（反引号替换）、
    /// `$(`（命令替换）。`;` 在 POSIX sh 与 Windows PowerShell 中都是语句
    /// 分隔符，一律阻止（Windows 切换 PowerShell 后不再豁免——cmd 时代
    /// `;` 无分隔语义，PS 时代有，防注入一致化）。
    /// 单个 `&` 不阻止：它同时是 URL/参数的常见字符，误伤大于收益
    /// （真正的链式注入由 `&&` 覆盖；PS 的调用运算符 `&` 由解释器
    /// 白名单兜底）。
    /// 管道 `|` 和重定向 `>` `<` 不在阻止范围内，因为它们是合法的
    /// 单命令操作。
    fn has_shell_metacharacters(command: &str) -> bool {
        command.contains("&&")
            || command.contains("||")
            || command.contains('`')
            || command.contains("$(")
            || command.contains(';')
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

    #[cfg(target_os = "windows")]
    #[test]
    fn test_check_path_posix_root_blacklist_does_not_block_whole_drive() {
        // Windows 回归：黑名单含 POSIX 根 "/" 时不得把整盘（当前盘根）误拒。
        // canonicalize("/") 会解析为当前驱动器根（如 F:\），旧实现前缀匹配
        // 拒绝一切路径；修复后跳过该条目。
        let dir = tempdir().unwrap();
        let p = dir.path().join("x.txt");
        std::fs::write(&p, "x").unwrap();
        let policy = policy_with(vec![], vec![PathBuf::from("/")]);
        assert!(
            policy.check_path(&p).is_ok(),
            "POSIX 根黑名单在 Windows 上不应误拒工作目录文件"
        );
    }

    #[test]
    fn test_check_path_existing_blocked_dir_still_blocked() {
        // 真实黑名单目录（非根组件）仍生效
        let dir = tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        let path = blocked.join("x.txt");
        std::fs::write(&path, "x").unwrap();
        let err = policy_with(vec![], vec![blocked.clone()])
            .check_path(&path)
            .unwrap_err();
        assert!(err.to_string().contains("黑名单"));
    }

    // ── T0-1：`..` 归一化（目标不存在时的沙箱绕过回归）──────────────────

    /// T0-1 主回归（白名单逃逸）：目标不存在时 `..` 必须参与归一化——
    /// 修复前 canonicalize 失败回退不折叠 `..`，`ghost/../..` 可欺骗
    /// 字符串前缀匹配逃出白名单（判定放行、实际落在白名单之外）。
    #[test]
    fn test_check_path_rules_rejects_parent_dir_escape_when_target_missing() {
        let dir = tempdir().unwrap();
        let allowed = dir.path().join("allowed");
        std::fs::create_dir_all(&allowed).unwrap();
        // ghost 与 escape.txt 均不存在：canonicalize 失败走 fallback
        let escape = allowed
            .join("ghost")
            .join("..")
            .join("..")
            .join("escape.txt");
        assert_eq!(
            check_path_rules(&escape, std::slice::from_ref(&allowed), &[]),
            PathCheckOutcome::NotAllowed
        );
    }

    /// T0-1 主回归（黑名单漏报）：路径经已存在目录 + `..` 混入黑名单目录，
    /// 目标不存在时必须折叠 `..` 后命中黑名单——修复前前缀匹配漏报放行，
    /// 而实际 OS 解析会落进黑名单目录。
    #[test]
    fn test_check_path_rules_blocks_parent_dir_path_into_blocked_dir() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let blocked = dir.path().join(".git");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&blocked).unwrap();
        // config 不存在 → fallback 路径；src 存在使 OS 解析必然落入 .git
        let sneaky = src.join("..").join(".git").join("config");
        assert!(matches!(
            check_path_rules(&sneaky, &[], std::slice::from_ref(&blocked)),
            PathCheckOutcome::Blocked(_)
        ));
    }

    /// 正向回归：`..` 折叠后仍落在白名单内 → 放行（归一化不得误伤合法穿越）。
    #[test]
    fn test_check_path_rules_allows_parent_dir_within_allowed() {
        let dir = tempdir().unwrap();
        let allowed = dir.path().join("allowed");
        std::fs::create_dir_all(allowed.join("sub")).unwrap();
        let path = allowed.join("sub").join("..").join("new.txt");
        assert_eq!(
            check_path_rules(&path, std::slice::from_ref(&allowed), &[]),
            PathCheckOutcome::Allowed
        );
    }

    /// 词法归一化基础语义：`.`/`..` 折叠、绝对路径不越根、相对越界保留。
    #[test]
    fn test_lexical_normalize_collapses_dot_and_dotdot() {
        use std::path::Path;
        assert_eq!(lexical_normalize(Path::new("a/../b")), PathBuf::from("b"));
        assert_eq!(lexical_normalize(Path::new("a/./b")), PathBuf::from("a/b"));
        assert_eq!(
            lexical_normalize(Path::new("a/../../b")),
            PathBuf::from("../b"),
            "相对路径越界 `..` 保留"
        );
        // 绝对路径：`..` 不越过根（`/..` = `/`）
        assert_eq!(
            lexical_normalize(Path::new("/../noop")),
            PathBuf::from("/noop")
        );
        // 绝对路径多级折叠：`<base>/x/../..` = `<base>/..`
        let dir = tempdir().unwrap();
        let base = dir.path().to_path_buf();
        let up = base.join("x").join("..").join("..").join("y");
        assert_eq!(lexical_normalize(&up), base.parent().unwrap().join("y"));
    }

    #[test]
    fn test_shell_metacharacters_allow_single_ampersand() {
        // 单个 &（URL/参数常见字符）不再视为注入元字符
        let policy = policy_with(vec![], vec![]);
        assert!(policy.check_command("curl http://x?a=1&b=2").is_ok());
        assert!(policy.check_command("echo AT&T").is_ok());
    }

    #[test]
    fn test_shell_metacharacters_still_block_chains() {
        // 命令链/命令替换仍阻止
        let policy = policy_with(vec![], vec![]);
        assert!(policy.check_command("rm a && rm b").is_err());
        assert!(policy.check_command("a || b").is_err());
        assert!(policy.check_command("echo $(id)").is_err());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_shell_metacharacters_posix_semicolon_blocked() {
        // POSIX 上 ; 是分隔符，阻止
        let policy = policy_with(vec![], vec![]);
        assert!(policy.check_command("cd /tmp; rm x").is_err());
    }

    #[test]
    fn test_blocked_multiword_prefix_match() {
        // 多词黑名单（如 `rm -rf /`、`del /s`）按整条命令前缀匹配——
        // 此前仅首词比较导致这些条目永不命中。
        let policy = SecurityPolicy::from_config(&SecurityConfig {
            blocked_commands: vec![
                "rm -rf /".to_string(),
                "del /s".to_string(),
                "Remove-Item -Recurse".to_string(),
            ],
            ..Default::default()
        });
        // 全目录删除 → 命中
        assert!(policy.check_command("rm -rf /").is_err());
        assert!(policy.check_command("rm -rf /etc").is_err());
        assert!(policy.check_command("del /s /q C:\\x").is_err());
        assert!(policy.check_command("remove-item -recurse c:\\").is_err());
        // 单文件删除（非递归、非根目录）→ 放行（完全放开语义）
        assert!(policy.check_command("rm file.txt").is_ok());
        assert!(policy.check_command("del old.log").is_ok());
    }

    #[test]
    fn test_relaxed_mode_skips_metachar_but_keeps_blocklist() {
        // 放宽模式：跳过元字符/解释器检查，但黑名单仍生效
        let policy = SecurityPolicy::from_config(&SecurityConfig {
            safety_mode: SafetyMode::Relaxed,
            blocked_commands: vec!["rm -rf /".to_string()],
            ..Default::default()
        });
        // 链式命令不再被元字符拦截
        assert!(policy.check_command("git status && git log").is_ok());
        assert!(policy.check_command("echo $(id)").is_ok());
        // 黑名单仍然命中
        assert!(policy.check_command("rm -rf /").is_err());
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

    #[test]
    fn test_classify_command_risk_single_source() {
        // Blocked 优先（rm/del/format 双重约束）
        assert_eq!(classify_command_risk("rm"), CommandRisk::Blocked);
        assert_eq!(classify_command_risk("del"), CommandRisk::Blocked);
        assert_eq!(classify_command_risk("rmdir"), CommandRisk::Blocked);
        assert_eq!(classify_command_risk("shutdown"), CommandRisk::Blocked);
        // 危险但不在默认黑名单（用户自定义移除后可确认执行）
        assert_eq!(classify_command_risk("fdisk"), CommandRisk::Dangerous);
        assert_eq!(classify_command_risk("mkfs"), CommandRisk::Dangerous);
        assert_eq!(classify_command_risk("dd"), CommandRisk::Dangerous);
        // 中等风险
        assert_eq!(classify_command_risk("git"), CommandRisk::Moderate);
        assert_eq!(classify_command_risk("cargo"), CommandRisk::Moderate);
        assert_eq!(classify_command_risk("docker"), CommandRisk::Moderate);
        // 低风险/未知
        assert_eq!(classify_command_risk("echo"), CommandRisk::Low);
        assert_eq!(classify_command_risk("ls"), CommandRisk::Low);
        // 归一后输入（extract_command_base 已保证小写无后缀）：路径形式不再误判
        assert_eq!(classify_command_risk("rmdir"), CommandRisk::Blocked);
    }

    #[test]
    fn test_transform_command_normalizes_path_and_suffix() {
        let policy = policy_with(vec![], vec![]);
        // transform 模式才转换
        let mut p = policy.clone();
        p.safety_mode = SafetyMode::Transform;
        // 路径 + .exe 形式经 extract_command_base 归一后同样可转换（此前只认裸名）；
        // 目标路径用 Windows 形式（Unix 路径以 / 开头会被当 flag 过滤，属既有语义）
        assert!(p
            .transform_command("C:\\Tools\\rm.exe -rf C:\\temp\\x")
            .is_some());
        assert!(p.transform_command("rm -rf C:\\temp\\x").is_some());
        // Unix 路径目标被当 flag 过滤 → 拒绝转换（既有语义：宁可拒绝不误转换）
        assert!(p.transform_command("rm -rf /tmp/x").is_none());
        // 非危险命令不转换
        assert!(p.transform_command("ls -la").is_none());
    }
}

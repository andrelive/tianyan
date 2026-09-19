//! 命令执行底层（shell provider）：解析、探测、提示词（ADR-037）。
//!
//! **单一事实源**：`ShellSpec` = 执行信息（executable/args）+ 提示词（hint）
//! **同源产出**——`execute_command` 的执行侧（`build_command`）与工具描述
//! 注入侧（`current_hint`）读同一个 spec，从结构上杜绝"配置了 A 但提示还说 B"
//! 的漂移。
//!
//! **解析契约**（[`resolve`]）：产出**绝对路径**可执行（探测或验证后）或
//! **明确报错**——不留给运行期 PATH 解析的模糊地带（探测/执行漂移免疫）。
//!
//! **稳定性纪律**（同 `ToolRegistry::definitions` 的前缀缓存契约）：`install`
//! 后 spec 字节稳定——hint 只在配置变更时重解析，绝不每轮动态生成。
//!
//! **切换的提示词后果（明示，见 ADR-037）**：切换 shell → 工具表指纹变化 →
//! 下一轮重建请求前缀（一次缓存未命中）+ 一次主动压缩（消息足够时）。低频
//! 操作，可接受；配置注释与文档须保留此说明。

use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use crate::common::error::TianyanError;
use crate::config::ExecutorConfig;

/// shell 身份（配置值解析后的规范化枚举）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// PowerShell 7+（pwsh）。
    Pwsh,
    /// Windows PowerShell 5.1（powershell）。
    WindowsPowerShell,
    /// cmd.exe。
    Cmd,
    /// bash。
    Bash,
    /// sh（POSIX）。
    Sh,
    /// 用户自定义可执行。
    Custom,
}

impl ShellKind {
    /// 解析配置值（不含 auto——auto 在 [`resolve`] 中展开为具体 kind）。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pwsh" => Some(Self::Pwsh),
            "powershell" => Some(Self::WindowsPowerShell),
            "cmd" => Some(Self::Cmd),
            "bash" => Some(Self::Bash),
            "sh" => Some(Self::Sh),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    /// 规范名（配置值 / 日志 / 错误消息用）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pwsh => "pwsh",
            Self::WindowsPowerShell => "powershell",
            Self::Cmd => "cmd",
            Self::Bash => "bash",
            Self::Sh => "sh",
            Self::Custom => "custom",
        }
    }

    /// 内置 provider 的默认可执行名（Custom 无默认名）。
    fn default_program(self) -> Option<&'static str> {
        match self {
            Self::Pwsh => Some("pwsh"),
            Self::WindowsPowerShell => Some("powershell"),
            Self::Cmd => Some("cmd"),
            Self::Bash => Some("bash"),
            Self::Sh => Some("sh"),
            Self::Custom => None,
        }
    }

    /// 内置 provider 的固定参数模板（命令串追加在最后）。
    ///
    /// `-NoProfile -NonInteractive`：跳过用户配置加载并禁止交互提示
    /// （后台/自动化场景下防挂起）；`-Command` / `-c` 直接执行整条命令串。
    fn default_args(self) -> Vec<String> {
        match self {
            Self::Pwsh | Self::WindowsPowerShell => {
                vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                ]
            }
            Self::Cmd => vec!["/C".into()],
            Self::Bash | Self::Sh => vec!["-c".into()],
            Self::Custom => vec!["-c".into()],
        }
    }
}

/// 各 provider 的提示词模板（注入 `execute_command` 工具描述；模型语境）。
///
/// **注意**：这些文本是提示词前缀的一部分——修改会打掉前缀缓存（纪律见
/// 模块文档）；`tool_catalog --check` 的跨平台归一化遍历 [`HINT_TEMPLATES`]。
pub const HINT_PWSH: &str = "平台：Windows。Shell 是 PowerShell 7+（pwsh）——支持 && / || 命令链接；cmdlet（Out-File、Select-String、Get-ChildItem）与管道可用。";
/// 5.1 提示词：明确 `&&`/`||` 不支持（模型高频踩坑，实测多起）。
pub const HINT_WINDOWS_POWERSHELL: &str = "平台：Windows。Shell 是 Windows PowerShell 5.1，不是 cmd.exe——不支持 && / ||（请用 ; 分隔多条命令，或分多次调用）；cmdlet 与管道可用。";
/// cmd 提示词（如实描述；模型生成 cmd 命令能力一般，复杂场景建议换 PS）。
pub const HINT_CMD: &str = "平台：Windows。Shell 是 cmd.exe——命令用 & / && 链接（cmd 语义）；如需 PowerShell 语法请配置 [executor].shell。";
/// bash 提示词（完整 bash 语法）。
pub const HINT_BASH: &str =
    "平台：Unix。Shell 是 bash——支持完整 bash 语法（[[ ]]、数组、进程替换等）与标准管道/重定向。";
/// sh 提示词（POSIX 边界，附切换指路）。
pub const HINT_SH: &str = "平台：Unix。Shell 是 sh -c（POSIX）——仅 POSIX 语法（无 [[ ]]、数组；如需 bash 语法请配置 [executor].shell = \"bash\"）。";

/// 全部内置提示词模板（`tool_catalog` freshness 跨平台归一化遍历此表）。
pub const HINT_TEMPLATES: &[&str] = &[
    HINT_PWSH,
    HINT_WINDOWS_POWERSHELL,
    HINT_CMD,
    HINT_BASH,
    HINT_SH,
];

/// 解析后的执行底层（执行 + 提示词同源）。
#[derive(Debug, Clone)]
pub struct ShellSpec {
    /// 身份（hint 模板选择 / 错误消息 / 日志）。
    pub kind: ShellKind,
    /// 执行路径：`resolve` 产出（探测或验证后的绝对路径；边界兜底除外）。
    /// `build_command` 直接使用——不依赖运行期 PATH 解析。
    pub executable: PathBuf,
    /// 固定参数（命令串追加在最后）。
    pub args: Vec<String>,
    /// 提示词（模型语境；display 长名内嵌于模板，不设独立字段）。
    pub hint: String,
}

impl ShellSpec {
    /// 构造执行命令（含输出编码注入：PS 系与 cmd 的 UTF-8 处理）。
    pub fn build_command(&self, command: &str) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(&self.executable);
        for a in &self.args {
            c.arg(a);
        }
        c.arg(self.wrap_command(command));
        // Windows：隐藏子进程控制台窗口（防黑窗一闪；Unix 无操作）。
        crate::executor::command::hide_console_window(c)
    }

    /// 命令包装（输出编码注入，对命令语义透明）：
    /// - PS 系（pwsh / Windows PowerShell）：`[Console]::OutputEncoding` 置
    ///   UTF-8——修复"PS 5.1 默认以系统 locale（中文 = GBK）读取 native 程序
    ///   输出"导致的 git/rg/cargo 等中文输出乱码；
    /// - cmd：`chcp 65001` 切 UTF-8 代码页（静默）；
    /// - bash / sh：无需处理（UTF-8 常态）。
    ///
    /// 退出码不受影响：前置语句与原命令以 `;` / `&` 顺序执行，退出码取最后一条。
    fn wrap_command(&self, command: &str) -> String {
        match self.kind {
            ShellKind::Pwsh | ShellKind::WindowsPowerShell => {
                format!("[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; {command}")
            }
            ShellKind::Cmd => format!("chcp 65001>nul & {command}"),
            ShellKind::Bash | ShellKind::Sh | ShellKind::Custom => command.to_string(),
        }
    }
}

/// 解析执行底层（唯一入口）。
///
/// - `shell = "auto"`：Windows 探测 `pwsh` → `powershell`；Unix 探测 `bash`
///   → `sh`（PATH 搜索，命中记**绝对路径**）；
/// - 显式 provider（`pwsh` / `powershell` / `cmd` / `bash` / `sh`）：验证可执行
///   存在（`shell_program` 可覆盖默认程序，名或绝对路径皆可）——找不到**明确
///   报错**（不静默回退：静默回退会让用户以为在用 A 实际在用 B）；
/// - `custom`：`shell_program` 必填（名 → PATH 探测；路径 → 验证存在）。
///
/// 失败信息包含修复指引（安装 / 配置 `[executor].shell_program`）。
pub fn resolve(cfg: &ExecutorConfig) -> Result<ShellSpec, TianyanError> {
    let sel = cfg.shell.trim().to_ascii_lowercase();
    match sel.as_str() {
        "auto" | "" => resolve_auto(),
        "custom" => resolve_custom(cfg),
        name => {
            let kind = ShellKind::parse(name).ok_or_else(|| {
                TianyanError::invalid_input(format!(
                    "executor: shell 配置无效：未知标识「{name}」——支持 auto/pwsh/powershell/cmd/bash/sh/custom"
                ))
            })?;
            resolve_known(kind, cfg.shell_program.as_deref())
        }
    }
}

/// auto 探测：Windows `pwsh` → `powershell`；Unix `bash` → `sh`。
fn resolve_auto() -> Result<ShellSpec, TianyanError> {
    let candidates: &[ShellKind] = if cfg!(windows) {
        &[ShellKind::Pwsh, ShellKind::WindowsPowerShell]
    } else {
        &[ShellKind::Bash, ShellKind::Sh]
    };
    for kind in candidates {
        if let Some(program) = kind.default_program() {
            if let Some(path) = find_in_path(program) {
                return Ok(build_spec(
                    *kind,
                    path,
                    &kind.default_args(),
                    builtin_hint(*kind),
                ));
            }
        }
    }
    Err(TianyanError::not_found(format!(
        "executor: 自动探测未找到可用 shell（候选：{}）——请检查 PATH 或显式配置 [executor].shell",
        candidates
            .iter()
            .filter_map(|k| k.default_program())
            .collect::<Vec<_>>()
            .join("/")
    )))
}

/// 显式 provider（可含 shell_program 覆盖）。
fn resolve_known(
    kind: ShellKind,
    program_override: Option<&str>,
) -> Result<ShellSpec, TianyanError> {
    let program = program_override
        .map(str::to_string)
        .or_else(|| kind.default_program().map(str::to_string))
        .ok_or_else(|| {
            TianyanError::invalid_input(format!(
                "executor: shell = {} 缺少可执行程序（shell_program）",
                kind.as_str()
            ))
        })?;
    let executable = resolve_executable(&program, kind)?;
    Ok(build_spec(
        kind,
        executable,
        &kind.default_args(),
        builtin_hint(kind),
    ))
}

/// custom：shell_program 必填；args / hint 可自定义（缺省模板）。
fn resolve_custom(cfg: &ExecutorConfig) -> Result<ShellSpec, TianyanError> {
    let program = cfg.shell_program.as_deref().ok_or_else(|| {
        TianyanError::invalid_input(
            "executor: shell = custom 需配置 [executor].shell_program（可执行名或路径）"
                .to_string(),
        )
    })?;
    let executable = resolve_executable(program, ShellKind::Custom)?;
    let args = cfg
        .shell_args
        .clone()
        .unwrap_or_else(|| ShellKind::Custom.default_args());
    let hint = cfg.shell_hint.clone().unwrap_or_else(|| {
        format!(
            "Shell 是自定义可执行：{}（参数模板：{}）——语法请以该 shell 为准。",
            program,
            args.join(" ")
        )
    });
    Ok(build_spec(ShellKind::Custom, executable, &args, hint))
}

/// 解析"名或路径"为绝对路径：绝对路径 → 验证存在；名字 → PATH 探测。
fn resolve_executable(program: &str, kind: ShellKind) -> Result<PathBuf, TianyanError> {
    let p = PathBuf::from(program);
    if p.is_absolute() {
        if p.is_file() {
            return Ok(p);
        }
        return Err(TianyanError::not_found(format!(
            "executor: shell = {} 指定的可执行路径不存在：{program}",
            kind.as_str()
        )));
    }
    find_in_path(program).ok_or_else(|| {
        TianyanError::not_found(format!(
            "executor: shell = {} 未在 PATH 中找到可执行「{program}」——请安装，或用 [executor].shell_program 指定绝对路径",
            kind.as_str()
        ))
    })
}

/// 组装 spec（唯一构造出口）。
fn build_spec(kind: ShellKind, executable: PathBuf, args: &[String], hint: String) -> ShellSpec {
    ShellSpec {
        kind,
        executable,
        args: args.to_vec(),
        hint,
    }
}

/// 内置 hint（按 kind）。
fn builtin_hint(kind: ShellKind) -> String {
    match kind {
        ShellKind::Pwsh => HINT_PWSH.to_string(),
        ShellKind::WindowsPowerShell => HINT_WINDOWS_POWERSHELL.to_string(),
        ShellKind::Cmd => HINT_CMD.to_string(),
        ShellKind::Bash => HINT_BASH.to_string(),
        ShellKind::Sh => HINT_SH.to_string(),
        ShellKind::Custom => String::new(), // custom 在 resolve_custom 内生成
    }
}

/// 在 PATH 中查找可执行文件（返回绝对路径；Windows 依次尝试 `name.exe` / `name`）。
///
/// 简化取舍：仅检查"文件存在"，不校验 Unix 可执行位——PATH 目录里同名的
/// 非可执行文件属边缘场景，报错信息已提供 shell_program 逃生舱。
fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let exts: &[&str] = if cfg!(windows) { &[".exe", ""] } else { &[""] };
    for dir in std::env::split_paths(&path_var) {
        for ext in exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

// ── 全局单例（进程级：shell 是环境事实，与 RUNNING_CHILDREN 同类先例） ──

/// 当前生效的 spec（`install` 后字节稳定；未 install 时 lazy auto 探测）。
static SHELL: OnceLock<RwLock<Arc<ShellSpec>>> = OnceLock::new();

fn cell() -> &'static RwLock<Arc<ShellSpec>> {
    SHELL.get_or_init(|| {
        // lazy：未显式 install 时按默认配置（auto）探测；极端环境兜底不 panic。
        let spec = resolve(&ExecutorConfig::default()).unwrap_or_else(|_| fallback_spec());
        RwLock::new(Arc::new(spec))
    })
}

/// 边界兜底（只在 lazy 探测失败时使用：连 sh / powershell 都找不到的极端
/// 环境——不 panic 优先，用名字走平台默认，尽量可用）。
fn fallback_spec() -> ShellSpec {
    if cfg!(windows) {
        ShellSpec {
            kind: ShellKind::WindowsPowerShell,
            executable: PathBuf::from("powershell"),
            args: ShellKind::WindowsPowerShell.default_args(),
            hint: HINT_WINDOWS_POWERSHELL.to_string(),
        }
    } else {
        ShellSpec {
            kind: ShellKind::Sh,
            executable: PathBuf::from("sh"),
            args: ShellKind::Sh.default_args(),
            hint: HINT_SH.to_string(),
        }
    }
}

/// 当前 spec（读锁极短、Arc 共享）。
pub fn current() -> Arc<ShellSpec> {
    cell().read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 安装（装配层在配置加载后调用；配置热更新时重解析后再次调用）。
pub fn install(spec: ShellSpec) {
    *cell().write().unwrap_or_else(|e| e.into_inner()) = Arc::new(spec);
}

/// 当前 hint（`execute_command` 工具描述注入用）。
pub fn current_hint() -> String {
    current().hint.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(shell: &str) -> ExecutorConfig {
        ExecutorConfig {
            shell: shell.to_string(),
            ..ExecutorConfig::default()
        }
    }

    #[test]
    fn kind_parse_and_display() {
        for (s, k) in [
            ("pwsh", ShellKind::Pwsh),
            ("powershell", ShellKind::WindowsPowerShell),
            ("cmd", ShellKind::Cmd),
            ("bash", ShellKind::Bash),
            ("sh", ShellKind::Sh),
            ("custom", ShellKind::Custom),
        ] {
            assert_eq!(ShellKind::parse(s), Some(k));
            assert_eq!(k.as_str(), s);
        }
        assert_eq!(ShellKind::parse("auto"), None); // auto 不是 kind（resolve 展开）
        assert_eq!(ShellKind::parse("zsh"), None);
    }

    #[test]
    fn resolve_auto_picks_platform_provider_with_absolute_path() {
        let spec = resolve(&cfg("auto")).expect("auto 应解析成功");
        if cfg!(windows) {
            assert!(matches!(
                spec.kind,
                ShellKind::Pwsh | ShellKind::WindowsPowerShell
            ));
        } else {
            assert!(matches!(spec.kind, ShellKind::Bash | ShellKind::Sh));
        }
        // 探测命中应记绝对路径（契约）
        assert!(
            spec.executable.is_absolute(),
            "探测结果应为绝对路径：{:?}",
            spec.executable
        );
        assert!(!spec.hint.is_empty());
    }

    #[test]
    fn resolve_unknown_name_fails_with_guidance() {
        let err = resolve(&cfg("zsh")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("未知标识"), "{msg}");
        assert!(msg.contains("custom"), "应列出支持项：{msg}");
    }

    #[test]
    fn resolve_missing_program_fails_not_silent() {
        // 用一个必然不存在的程序名（显式 provider + 覆盖名）
        let mut c = cfg("bash");
        c.shell_program = Some("definitely-not-a-shell-12345".to_string());
        let err = resolve(&c).unwrap_err();
        assert!(err.to_string().contains("未在 PATH 中找到"), "{err}");
    }

    #[test]
    fn resolve_custom_requires_program() {
        let err = resolve(&cfg("custom")).unwrap_err();
        assert!(err.to_string().contains("shell_program"), "{err}");
    }

    #[test]
    fn resolve_custom_via_existing_program() {
        // 跨平台存在的程序：Unix 用 sh；Windows 用 powershell
        let program = if cfg!(windows) { "powershell" } else { "sh" };
        let mut c = cfg("custom");
        c.shell_program = Some(program.to_string());
        let spec = resolve(&c).expect("custom 应解析成功");
        assert_eq!(spec.kind, ShellKind::Custom);
        assert_eq!(spec.args, vec!["-c".to_string()]);
        assert!(
            spec.hint.contains(program),
            "自动 hint 应含程序名：{}",
            spec.hint
        );
    }

    #[test]
    fn resolve_explicit_absolute_path_program() {
        // 用当前环境真实存在的绝对路径（探测一个平台的基准程序）
        let base = if cfg!(windows) { "powershell" } else { "sh" };
        let abs = find_in_path(base).expect("基准程序应在 PATH 中");
        let mut c = cfg("custom");
        c.shell_program = Some(abs.to_string_lossy().into_owned());
        let spec = resolve(&c).expect("绝对路径应验证通过");
        assert!(spec.executable.is_absolute());
    }

    #[test]
    fn build_command_appends_command_last_and_wraps_ps_family() {
        let spec = ShellSpec {
            kind: ShellKind::WindowsPowerShell,
            executable: PathBuf::from("powershell"),
            args: vec!["-NoProfile".into(), "-Command".into()],
            hint: String::new(),
        };
        let c = spec.build_command("echo hi");
        let args: Vec<String> = c
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.len(), 3, "{args:?}");
        assert_eq!(args[0], "-NoProfile");
        assert_eq!(args[1], "-Command");
        // 命令必须是最后一个参数；PS 系前置输出编码注入
        assert!(args[2].contains("[Console]::OutputEncoding"), "{args:?}");
        assert!(args[2].trim_end().ends_with("echo hi"), "{args:?}");
    }

    #[test]
    fn build_command_cmd_wraps_chcp_and_unix_passthrough() {
        let cmd_spec = ShellSpec {
            kind: ShellKind::Cmd,
            executable: PathBuf::from("cmd"),
            args: vec!["/C".into()],
            hint: String::new(),
        };
        let args: Vec<String> = cmd_spec
            .build_command("dir")
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args[1].contains("chcp 65001"), "{args:?}");

        let sh_spec = ShellSpec {
            kind: ShellKind::Sh,
            executable: PathBuf::from("sh"),
            args: vec!["-c".into()],
            hint: String::new(),
        };
        let args: Vec<String> = sh_spec
            .build_command("ls -la")
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec!["-c".to_string(), "ls -la".to_string()],
            "Unix 直通不包装"
        );
    }

    #[test]
    fn install_and_current_roundtrip() {
        let spec = ShellSpec {
            kind: ShellKind::Sh,
            executable: PathBuf::from("/bin/sh"),
            args: vec!["-c".into()],
            hint: "测试 hint".to_string(),
        };
        install(spec);
        assert_eq!(current().kind, ShellKind::Sh);
        assert_eq!(current_hint(), "测试 hint");
        // 还原环境（其他测试依赖 fallback/先前状态不做强断言，仅确保可重装）
        let restored = resolve(&cfg("auto")).unwrap_or_else(|_| fallback_spec());
        install(restored);
    }
}

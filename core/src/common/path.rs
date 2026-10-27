//! 目录路径的**跨进程比较**口径。
//!
//! 「同一个数据目录」是桌面端「单 server 复用」的判据：客户端探测到本机已有天演
//! server 时，必须先确认那个 server 服务的就是**自己配置里的数据目录**——数据目录
//! 不同就绝不能复用（否则会读写别人的数据）。
//!
//! 这条判定有两个容易踩的坑，都由本模块统一收口：
//!
//! 1. **写法不同 ≠ 目录不同**：`D:\tianyan`、`d:\tianyan\`、`D:\tianyan\.\`、
//!    `D:\tianyan\logs\..` 说的都是同一个目录。用 `canonicalize` 解析真实路径，
//!    解析失败（目录还没建出来）时退化为「去尾部反斜杠 + 统一分隔符」。
//! 2. **Windows 路径大小写不敏感**：`D:\Tianyan` 与 `d:\tianyan` 是同一个目录，
//!    但字符串比较会判成不同 → 在 Windows 上统一转小写再比。
//!
//! 服务端 `/health` 的 `data_dir` 字段与客户端的探测逻辑**必须走同一函数**，
//! 否则两侧口径漂移会让「复用」静默失效（表现为第二个 server 照旧被启动）。

use std::path::{Path, PathBuf};

/// 规范化目录路径，用于跨进程字符串比较。
///
/// - 优先 `canonicalize`（解析 `..`、短路径、符号链接，得到绝对真实路径）；
/// - 失败时退化为原路径，但至少统一分隔符并去掉尾部反斜杠（**不**把它转成绝对路径：
///   相对路径的基准目录在另一进程里未知，强行拼接只会得到错误答案）；
/// - Windows 上转小写（大小写不敏感）。
pub fn normalize_dir(dir: &Path) -> String {
    let resolved: PathBuf = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut s = resolved.to_string_lossy().replace('/', "\\");
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    if cfg!(windows) {
        s = s.to_lowercase();
    }
    s
}

/// 两个目录是否指向同一位置（比较口径见 [`normalize_dir`]）。
///
/// 任一侧为空字符串时返回 `false`：空串来自「对端没报 data_dir」（旧版本 server，
/// 或 health 里取不到配置），此时**不能**当作匹配——宁可多起一个 server 也不能
/// 错连到别的数据目录。
pub fn same_dir(a: &str, b: &Path) -> bool {
    if a.trim().is_empty() {
        return false;
    }
    normalize_dir(Path::new(a)) == normalize_dir(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_is_idempotent_and_case_insensitive_on_windows() {
        let tmp = std::env::temp_dir();
        let once = normalize_dir(&tmp);
        let twice = normalize_dir(Path::new(&once));
        assert_eq!(once, twice, "规范化必须幂等");
        if cfg!(windows) {
            assert_eq!(once, once.to_lowercase(), "Windows 上应统一小写");
        }
    }

    #[test]
    fn test_trailing_separator_does_not_change_identity() {
        let tmp = std::env::temp_dir();
        let with_sep = tmp.join("");
        assert_eq!(normalize_dir(&tmp), normalize_dir(&with_sep));
    }

    #[test]
    fn test_dot_dot_resolves_to_same_dir() {
        // 目录必须真实存在，canonicalize 才会做解析
        let base = std::env::temp_dir();
        let nested = base.join("tianyan-normalize-probe");
        std::fs::create_dir_all(&nested).expect("建临时目录");
        let round_trip = nested.join("..").join("tianyan-normalize-probe");
        assert_eq!(normalize_dir(&nested), normalize_dir(&round_trip));
        let _ = std::fs::remove_dir_all(&nested);
    }

    #[test]
    fn test_same_dir_rejects_empty_foreign_report() {
        let tmp = std::env::temp_dir();
        assert!(!same_dir("", &tmp), "对端没报 data_dir 时不能判为同目录");
        assert!(!same_dir("   ", &tmp));
        assert!(same_dir(&normalize_dir(&tmp), &tmp));
    }

    #[test]
    fn test_different_dirs_are_not_same() {
        let a = std::env::temp_dir().join("tianyan-path-a");
        let b = std::env::temp_dir().join("tianyan-path-b");
        assert!(!same_dir(&normalize_dir(&a), &b));
    }
}

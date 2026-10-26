//! 会话**生效工作目录**的解析结果（三态 + 无可用目录）。
//!
//! ## 为什么需要「状态」而不只是 `Option<PathBuf>`
//!
//! 旧实现把所有情况压成一个 `Option<PathBuf>`：会话绑定的目录一旦失效（被删 /
//! 移走 / 未挂载），`is_dir()` 为假 → 与「从未绑定」**不可区分**。两侧因此都只能
//! 各自兜底：Agent 侧回退 `[agent] working_directory`，工具侧**静默回退进程 cwd**
//! （= 服务安装目录）。用户看到的现象是「模型对着一个不存在的目录反复试错」，
//! 而写操作实际落到了安装目录（安全风险）。
//!
//! 三态把「失效」变成**可命名的事实**：环境段据 [`WorkingDirectory::warning`]
//! 如实告知模型，相对路径解析据 [`WorkingDirectory::path`] 取真正可用的目录，
//! 两者来自**同一个值**——不会出现「告诉模型 A、实际按 B 干活」的分叉。
//!
//! ## 为什么是纯类型、放在 `common::types`
//!
//! 判定结果被三处消费：`Agent::resolve_working_directory`（快照捕获 / 回退重做）、
//! `ContextPort::load_injectable`（AGENTS.md 探测根 + 环境段文案）、
//! `ContextAssembler`（环境段渲染）。而 `context` 模块**不依赖** `agent`——
//! `InjectableContext` 当初正是为消除 `context` → `agent` 的倒置依赖才搬进
//! `common::types` 的（见 `injectable.rs` 模块文档）。本类型与它并列放在这里，
//! 同一个倒置不会重新出现；插件（`plugins/policy-context`）经
//! `tianyan::common::types::WorkingDirectory` 使用，符合「插件只依赖 core」。
//!
//! 纯数据 + 纯判定函数：除 `is_dir()` 探测外无任何副作用，可单测。

use std::path::{Path, PathBuf};

/// 会话生效工作目录的解析结果。
///
/// 区分「会话绑定失效」与「正常」：环境段必须如实告知模型，否则模型会
/// 对着一个不存在的目录反复试错（或以为它有效而继续写路径）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkingDirectory {
    /// 会话绑定的目录，真实存在
    Bound(PathBuf),
    /// 会话绑定的目录已失效（被删/移走/未挂载），回退到全局默认
    /// （`fallback` 为 `None` = 全局默认也没有可用目录）
    BoundInvalid {
        /// 会话 header 里绑定的那个（已不存在的）目录——警示文案要指名它
        bound: PathBuf,
        /// 实际用于相对路径解析的兜底目录（同样通过了 `is_dir()` 检查）
        fallback: Option<PathBuf>,
    },
    /// 会话未绑定，使用全局默认（该目录真实存在）
    Default(PathBuf),
    /// 没有任何可用目录（未绑定且无默认，或默认目录也不存在）
    Unavailable,
}

impl WorkingDirectory {
    /// 由「会话绑定的目录」与「全局默认目录」判定状态。
    ///
    /// **纯函数判定核心**：调用方只负责把两个候选目录读出来（一个来自
    /// `SessionHeader.working_directory`，一个来自 `[agent] working_directory`），
    /// 判定规则集中在此处——`Agent::resolve_working_directory` 因此只剩 I/O 取数，
    /// 判定本身可单测（含「绑定的目录被删」这类需要真实文件系统的事实）。
    ///
    /// 空路径（`""`）视为「未绑定」：历史数据里可能存了空串，`PathBuf::from("")`
    /// 的 `is_dir()` 为假，若照直判定会渲染出一条指名为空的警示（噪声）。
    pub fn resolve(bound: Option<&Path>, default_dir: Option<&Path>) -> Self {
        let bound = bound.filter(|p| !p.as_os_str().is_empty());
        // 兜底目录**同样要真实存在**：默认目录可能被删/未挂载，此时它不能当兜底
        // （否则相对路径会拼到一个不存在的目录上，错误信息还指向一个"看似有效"的路径）
        let fallback = default_dir.filter(|p| p.is_dir()).map(Path::to_path_buf);
        match bound {
            Some(p) if p.is_dir() => Self::Bound(p.to_path_buf()),
            Some(p) => Self::BoundInvalid {
                bound: p.to_path_buf(),
                fallback,
            },
            None => match fallback {
                Some(dir) => Self::Default(dir),
                None => Self::Unavailable,
            },
        }
    }

    /// 实际用于相对路径解析与 AGENTS.md 探测的目录（`Unavailable` → None）。
    ///
    /// 这是**唯一**该被用于「干活」的取值口：`BoundInvalid` 返回兜底目录
    /// （而不是那个已不存在的绑定目录），`Unavailable` 明确返回 `None`
    /// ——调用方据此失败或跳过，不允许再自行回退进程 cwd。
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Bound(p) | Self::Default(p) => Some(p.as_path()),
            Self::BoundInvalid { fallback, .. } => fallback.as_deref(),
            Self::Unavailable => None,
        }
    }

    /// 环境段警示文案（仅 `BoundInvalid` / `Unavailable` 有值）。
    ///
    /// 措辞只做一件事：让模型知道「这不是它的错觉，也不是它该反复试错的事」，
    /// 而是**该提醒用户**的装配问题。正常态（`Bound` / `Default`）返回 `None`
    /// ——每轮都在前缀里的段不能常驻噪声。
    pub fn warning(&self) -> Option<String> {
        match self {
            Self::Bound(_) | Self::Default(_) => None,
            Self::BoundInvalid {
                bound,
                fallback: Some(_),
            } => Some(format!(
                "⚠️ 本会话绑定的工作目录 `{}` 已不存在（可能被删除、移动或未挂载），\
                 已回退到全局默认工作目录。",
                bound.display()
            )),
            Self::BoundInvalid {
                bound,
                fallback: None,
            } => Some(format!(
                "⚠️ 本会话绑定的工作目录 `{}` 已不存在（可能被删除、移动或未挂载），\
                 且没有可用的默认工作目录——相对路径操作将失败。请让用户为该会话绑定工作区。",
                bound.display()
            )),
            Self::Unavailable => Some(
                "⚠️ 没有可用工作目录，相对路径操作将失败。请让用户为该会话绑定工作区。".to_string(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 绑定目录真实存在 → `Bound`，取该目录、无警示。
    #[test]
    fn test_bound_valid_dir() {
        let dir = tempfile::tempdir().unwrap();
        let wd = WorkingDirectory::resolve(Some(dir.path()), Some(Path::new("C:/fallback")));
        assert_eq!(wd, WorkingDirectory::Bound(dir.path().to_path_buf()));
        assert_eq!(wd.path(), Some(dir.path()), "绑定有效时取绑定目录");
        assert_eq!(wd.warning(), None, "正常态不得有警示（前缀噪声）");
    }

    /// 绑定后目录被删 → `BoundInvalid`，回退到默认目录（默认目录真实存在）。
    ///
    /// 判别力：旧实现这里与「未绑定」不可区分（都只剩 fallback），模型无从得知
    /// 自己绑的目录已经没了。
    #[test]
    fn test_bound_invalid_dir_falls_back_to_default() {
        let bound = tempfile::tempdir().unwrap();
        let fallback = tempfile::tempdir().unwrap();
        let bound_path = bound.path().to_path_buf();
        drop(bound); // 删掉绑定的目录（未挂载 / 被移走的等价形态）

        let wd = WorkingDirectory::resolve(Some(&bound_path), Some(fallback.path()));
        assert_eq!(
            wd,
            WorkingDirectory::BoundInvalid {
                bound: bound_path.clone(),
                fallback: Some(fallback.path().to_path_buf()),
            }
        );
        assert_eq!(
            wd.path(),
            Some(fallback.path()),
            "实际解析必须落到默认目录，而不是那个已不存在的绑定目录"
        );
        let warn = wd.warning().expect("绑定失效必须有警示");
        assert!(warn.contains("⚠️"), "警示必须显眼：{warn}");
        assert!(
            warn.contains(&bound_path.display().to_string()),
            "警示必须指名失效的绑定目录：{warn}"
        );
        assert!(warn.contains("已不存在"), "警示须说明事实：{warn}");
    }

    /// 绑定失效 + 默认目录**也不存在** → `BoundInvalid { fallback: None }`。
    ///
    /// 判别力：默认目录不做 `is_dir()` 检查时，这里会给出一个假的「可用」目录
    /// （把相对路径拼到不存在的目录上，报错信息还指向一个看似有效的路径）。
    #[test]
    fn test_bound_invalid_and_default_missing() {
        let bound = tempfile::tempdir().unwrap();
        let fallback = tempfile::tempdir().unwrap();
        let bound_path = bound.path().to_path_buf();
        let fallback_path = fallback.path().to_path_buf();
        drop(bound);
        drop(fallback); // 两个目录都不存在

        let wd = WorkingDirectory::resolve(Some(&bound_path), Some(&fallback_path));
        assert_eq!(
            wd,
            WorkingDirectory::BoundInvalid {
                bound: bound_path.clone(),
                fallback: None,
            },
            "默认目录不存在时不得充当兜底"
        );
        assert_eq!(wd.path(), None, "无可用目录 → None（调用方据此失败）");
        let warn = wd.warning().expect("必须警示");
        assert!(warn.contains(&bound_path.display().to_string()), "{warn}");
        assert!(
            warn.contains("没有可用的默认工作目录"),
            "警示须点明连兜底都没有：{warn}"
        );
    }

    /// 未绑定 + 默认目录存在 → `Default`。
    #[test]
    fn test_unbound_with_default() {
        let dir = tempfile::tempdir().unwrap();
        let wd = WorkingDirectory::resolve(None, Some(dir.path()));
        assert_eq!(wd, WorkingDirectory::Default(dir.path().to_path_buf()));
        assert_eq!(wd.path(), Some(dir.path()));
        assert_eq!(wd.warning(), None, "未绑定但有默认 = 正常态");
    }

    /// 未绑定 + 无默认 → `Unavailable`（唯一该让相对路径操作失败的态）。
    #[test]
    fn test_unavailable() {
        let wd = WorkingDirectory::resolve(None, None);
        assert_eq!(wd, WorkingDirectory::Unavailable);
        assert_eq!(wd.path(), None);
        let warn = wd.warning().expect("无可用目录必须警示");
        assert!(warn.contains("⚠️"), "{warn}");
        assert!(warn.contains("相对路径操作将失败"), "{warn}");
    }

    /// 空串绑定（历史数据 / 绑定异常）按「未绑定」处理，不渲染指名为空的警示。
    #[test]
    fn test_empty_bound_treated_as_unbound() {
        let dir = tempfile::tempdir().unwrap();
        let wd = WorkingDirectory::resolve(Some(Path::new("")), Some(dir.path()));
        assert_eq!(wd, WorkingDirectory::Default(dir.path().to_path_buf()));
        let wd = WorkingDirectory::resolve(Some(Path::new("")), None);
        assert_eq!(wd, WorkingDirectory::Unavailable);
    }

    /// 未绑定 + 默认目录不存在 → `Unavailable`（默认目录同样要过 `is_dir()`）。
    #[test]
    fn test_default_missing_is_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        drop(dir);
        assert_eq!(
            WorkingDirectory::resolve(None, Some(&path)),
            WorkingDirectory::Unavailable
        );
    }
}

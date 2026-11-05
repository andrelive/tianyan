//! 检索结果的「项目作用域」判定：抑制跨项目记忆污染。
//!
//! ## 问题（实测）
//!
//! VFS 没有项目维度：所有项目的记忆躺在一个池子里，只靠 query 语义相似度区分。
//! 实测一次天演开发轮的注入记忆里，有 2 条是 **DevScaffold 项目**的（讲 5180/5173
//! /8080 端口与目录结构）——用户做的是天演，与它毫无关系，却照样过了相关性门槛
//! 进了前缀。同领域项目之间天然互相「有点像」，靠调门槛分不开。
//!
//! ## 为什么用「内容里的绝对路径」而不是给条目加 project 字段
//!
//! 加元数据字段要改 `EntryMetadata` 契约、重算向量（payload 里带 metadata）、
//! 再给存量条目逐条打标；而**项目归属的真实信号本来就在内容里**——用户记忆条目
//! 几乎都带绝对路径（`F:\\work\\git\\tianyan`、`F:\\work\\git\\framework\\DevScaffold`、
//! `F:\\work\\GNSS`…）。读内容本来就要做（`load_content_for_results`），这里只是
//! 在已读到的文本上多跑一次判定：**零结构改动、零额外 IO、零 embedding 成本**。
//!
//! ## 边界（不假装它更强）
//!
//! 内容完全不含路径的条目返回 [`ScopeMatch::Unknown`]（中性，不加不减）——这类
//! 条目多为跨项目通用经验（工具行为、协作偏好），**本来就不该被隔离**。因此本
//! 机制只会压掉「明确写着别的项目路径」的条目，不会误伤通用知识。

use std::path::{Path, PathBuf};

/// 内容与当前工作目录的项目关系。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeMatch {
    /// 内容里出现了当前项目路径 → 与本轮任务相关。
    SameProject,
    /// 内容里只出现其它项目路径 → 跨项目条目（降权）。
    OtherProject,
    /// 内容不含绝对路径，或没有可用的当前目录 → 无法判断（中性）。
    Unknown,
}

/// 跨项目条目的分数折扣。
///
/// 取 0.55 而不是直接丢弃：有些跨项目经验可迁移（「另一个 Rust 项目也踩过这个坑」）。
/// 与相关性门槛（组合分 0.48）配合后，0.6 分左右的跨项目条目会落到 0.33 —— 低于
/// 门槛而被丢弃；真正高相关（0.9+）的跨项目条目仍能留下，那通常确实是通用经验。
pub const OTHER_PROJECT_FACTOR: f32 = 0.55;

/// 判定 `content` 与 `current_root` 的项目关系。
pub fn classify_scope(content: &str, current_root: &Path) -> ScopeMatch {
    let current = normalize_path(current_root);
    if current.is_empty() {
        return ScopeMatch::Unknown;
    }

    let mut saw_path = false;
    let mut saw_other = false;
    for raw in extract_absolute_paths(content) {
        saw_path = true;
        let p = normalize_path(Path::new(raw));
        if is_under(&p, &current) {
            // 命中当前项目即可提前返回：同时提到两个项目的条目，多半是本项目在
            // 与别的项目做对照（如「对比 A 与 B」），属于相关条目
            return ScopeMatch::SameProject;
        }
        saw_other = true;
    }

    if saw_path && saw_other {
        ScopeMatch::OtherProject
    } else {
        ScopeMatch::Unknown
    }
}

/// `path` 是否位于 `root` 之下（**按路径段**比较，不是字符串前缀）。
///
/// 只用 `starts_with` 会把 `F:\work\git\tianyan-other` 误判成当前项目
/// `F:\work\git\tianyan` 的子路径。
fn is_under(path: &str, root: &str) -> bool {
    if path == root {
        return true;
    }
    path.strip_prefix(root)
        .is_some_and(|rest| rest.starts_with('\\'))
}

/// 作用域折扣因子。
pub fn scope_factor(scope: ScopeMatch) -> f32 {
    match scope {
        ScopeMatch::OtherProject => OTHER_PROJECT_FACTOR,
        // 同项目不加成：加成会让「提到本项目」压过「内容真正相关」，用相关性排序即可
        ScopeMatch::SameProject | ScopeMatch::Unknown => 1.0,
    }
}

/// 归一化路径用于前缀比较：统一小写、`/` → `\`、去掉尾部反斜杠。
///
/// Windows 路径大小写不敏感，且用户/模型写 `F:/work` 与 `F:\work` 混杂。
fn normalize_path(path: &Path) -> String {
    let mut s = path.to_string_lossy().to_lowercase().replace('/', "\\");
    while s.ends_with('\\') && s.len() > 3 {
        s.pop();
    }
    s
}

/// 从文本里提取 Windows 绝对路径（`C:\...` / `D:/...`）。
///
/// 手写扫描而不引 `regex`：只需要一条固定模式，且要保证返回的是 `&str` 切片
/// （不分配）；终止符覆盖路径在自然语言里常见的收尾（空白、引号、中英文标点、
/// 括号、markdown 反引号与表格竖线）。
fn extract_absolute_paths(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 3 < bytes.len() {
        // 盘符必须是「词首的单个字母」：否则 `http://…` 里的 `p:/` 会被当成盘符路径
        let at_word_start = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let is_drive = bytes[i].is_ascii_alphabetic()
            && at_word_start
            && bytes[i + 1] == b':'
            && (bytes[i + 2] == b'\\' || bytes[i + 2] == b'/');
        if !is_drive {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i + 3;
        while j < bytes.len() {
            if is_path_terminator(bytes[j]) {
                break;
            }
            j += 1;
        }
        if j > start + 3 {
            out.push(&text[start..j]);
        }
        i = j + 1;
    }
    out
}

/// 路径终止字节（遇到即停）。非 ASCII 字节一律视为终止：路径里出现中文时
/// 宁可截断（保守判定成「不含本项目路径」）也不要吞掉后续自然语言。
fn is_path_terminator(b: u8) -> bool {
    b >= 0x80
        || matches!(
            b,
            b' ' | b'\t'
                | b'\n'
                | b'\r'
                | b'"'
                | b'\''
                | b'`'
                | b','
                | b';'
                | b':'
                | b'('
                | b')'
                | b'['
                | b']'
                | b'{'
                | b'}'
                | b'<'
                | b'>'
                | b'|'
                | b'*'
                | b'#'
        )
}

/// 由当前工作目录构造「项目根」。
///
/// 直接取工作目录本身：用户的多项目布局是 `F:\work\git\tianyan`、
/// `F:\work\git\framework\DevScaffold` 这类**互为兄弟**的目录，用工作目录做前缀
/// 判定即可区分；不需要（也无法可靠地）再往上找「仓库根」。
pub fn project_root(working_directory: Option<&Path>) -> Option<PathBuf> {
    working_directory
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from(r"F:\work\git\tianyan")
    }

    #[test]
    fn extracts_windows_paths() {
        let text = "参考 `F:\\work\\git\\tianyan\\core\\src` 与 F:/work/GNSS，还有 C:\\x";
        let got = extract_absolute_paths(text);
        assert!(got.iter().any(|p| p.contains("tianyan")), "{got:?}");
        assert!(got.iter().any(|p| p.contains("GNSS")), "{got:?}");
        assert!(got.iter().any(|p| p.starts_with("C:")), "{got:?}");
    }

    #[test]
    fn ignores_non_absolute_and_urls() {
        let got = extract_absolute_paths("http://example.com:8080/a 与 src/main.rs");
        assert!(
            got.is_empty(),
            "URL 与相对路径都不该被当成 Windows 绝对路径: {got:?}"
        );
    }

    #[test]
    fn same_project_when_path_under_root() {
        let text = "天演 `F:\\work\\git\\tianyan\\core\\src\\vfs` 的改动";
        assert_eq!(classify_scope(text, &root()), ScopeMatch::SameProject);
    }

    #[test]
    fn other_project_when_only_foreign_path() {
        let text = "DevScaffold 端口：docs-site 5180、frontend 5173（F:\\work\\git\\framework\\DevScaffold）";
        assert_eq!(classify_scope(text, &root()), ScopeMatch::OtherProject);
    }

    #[test]
    fn unknown_without_paths() {
        let text = "只把稳定、跨会话可复用的偏好写入记忆，临时性内容不写";
        assert_eq!(classify_scope(text, &root()), ScopeMatch::Unknown);
    }

    #[test]
    fn mixed_projects_counts_as_same() {
        // 同时提到本项目与其它项目（对照分析）→ 与本项目相关
        let text =
            "对比 F:\\work\\git\\tianyan 与 F:\\work\\git\\framework\\DevScaffold 的构建脚本";
        assert_eq!(classify_scope(text, &root()), ScopeMatch::SameProject);
    }

    #[test]
    fn case_and_separator_insensitive() {
        let text = "路径 F:/WORK/GIT/TIANYAN/core 下";
        assert_eq!(classify_scope(text, &root()), ScopeMatch::SameProject);
    }

    #[test]
    fn factor_discounts_other_project_only() {
        assert_eq!(scope_factor(ScopeMatch::OtherProject), OTHER_PROJECT_FACTOR);
        assert_eq!(scope_factor(ScopeMatch::SameProject), 1.0);
        assert_eq!(scope_factor(ScopeMatch::Unknown), 1.0);
    }

    #[test]
    fn root_prefix_is_not_substring_match() {
        // `F:\work\git\tianyan-other` 不是当前项目（前缀比较按路径段，不是子串）
        let text = "F:\\work\\git\\tianyan-other\\src";
        assert_eq!(classify_scope(text, &root()), ScopeMatch::OtherProject);
    }
}

//! 语义 slug 规整：把模型给出的条目 id 规整成可用作 URI 文件名的 kebab-case。
//!
//! 为什么要集中在这里：演化任务（写入侧）与运维工具（存量重命名侧）都要把
//! 自由文本 id 变成文件名。两处各写一份必然漂移——`core` 是被依赖方，规整规则
//! 只应有一份实现。

/// 语义 slug 的最大长度（进 URI 文件名，太长会撑爆界面与日志）。
pub const MAX_SLUG_LEN: usize = 48;

/// 语义 slug 的最小长度（太短没有辨识度，`ab` 这类无法表达含义）。
pub const MIN_SLUG_LEN: usize = 3;

/// 把自由文本 id 规整为可用作 URI 文件名的**语义 slug**。
///
/// 返回 `None` 表示不可用，调用方应回退到时间戳命名。判据刻意从严——命名的可读性
/// 正是这类改动目的，宽松接受只是把「不可读」从 `evo-{时间戳}` 换成另一种不可读：
///
/// - 只保留 `[a-z0-9]` 与作为分隔的 `-`（其余字符折成 `-`，连续 `-` 合并，
///   首尾 `-` 去掉）。**中文因此会被丢弃**：URI 里中文要被百分号编码，文件名既不
///   可读，也和现有全英文 slug（`doc-audience-layering` 等）不一致；
/// - 规整后长度须在 [`MIN_SLUG_LEN`]..=[`MAX_SLUG_LEN`]。
///
/// 例：`Apply_Patch 定位失败` → `Some("apply-patch")`；`用户偏好表格` → `None`。
pub fn sanitize_slug(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut last_is_dash = true; // 置真 → 开头的 `-` 被吞掉
    for ch in raw.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_is_dash = false;
        } else if !last_is_dash {
            out.push('-');
            last_is_dash = true;
        }
    }
    let slug = out.trim_matches('-');
    if slug.len() < MIN_SLUG_LEN || slug.len() > MAX_SLUG_LEN {
        return None;
    }
    Some(slug.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_to_kebab_case() {
        assert_eq!(
            sanitize_slug("Apply_Patch 定位失败"),
            Some("apply-patch".to_string())
        );
        assert_eq!(
            sanitize_slug("--user  prefers  tables--"),
            Some("user-prefers-tables".to_string())
        );
    }

    #[test]
    fn rejects_unusable() {
        assert_eq!(sanitize_slug("用户偏好表格"), None, "纯中文无可用字符");
        assert_eq!(sanitize_slug("ab"), None, "过短");
        assert_eq!(sanitize_slug(&"a".repeat(MAX_SLUG_LEN + 1)), None, "过长");
        assert_eq!(sanitize_slug(""), None);
        assert_eq!(sanitize_slug("---"), None);
    }

    #[test]
    fn accepts_boundaries() {
        assert_eq!(sanitize_slug("ev0"), Some("ev0".to_string()));
        let max = "a".repeat(MAX_SLUG_LEN);
        assert_eq!(sanitize_slug(&max), Some(max));
    }
}

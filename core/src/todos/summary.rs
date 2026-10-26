//! 待办清单的**上下文注入渲染**（P0：让清单每轮都进请求）。
//!
//! 此前待办只存在于 store 与 UI 面板里——模型必须**主动**调 `todo list` 才知道
//! 自己规划过什么。实测症状：面板里 5 项有 4 项已完成，模型却继续按「待开始」
//! 推进（做着做着就忘了）。解法与 `opencode-todolist` 一致：每轮把当前清单
//! 渲染成极紧凑的一段注入上下文（见 [`render_todos_summary`]）。
//!
//! **注入位置**由 `ContextAssembler` 决定（最后一条 user 消息之前，不进系统
//! 前缀段——待办每轮都在变，进前缀会让 prompt 缓存每轮失效，ADR-004）。
//!
//! 这段文本每轮都进请求，所以渲染规则以「省 token」为第一约束：
//! - 不输出 id（整表替换语义下模型不需要 id）；
//! - `description` 压成单行并截断到 [`DESCRIPTION_LIMIT`] 字符；
//! - 空清单返回 `None`（整段不注入，不占 token）。

use super::{TodoItem, TodoStatus};

/// 单条描述在注入文本里的字符上限（超出截断加省略号）。
const DESCRIPTION_LIMIT: usize = 60;

/// 按状态计数：`(pending, in_progress, completed)`。
///
/// 注入摘要的头部（「进行中 N」）与 `todo` 工具返回值的 `counts` 字段共用
/// 这一个计数口径——两处各算一遍曾出现「面板说 1 项进行中、工具返回说 0」。
pub fn count_by_status(items: &[TodoItem]) -> (usize, usize, usize) {
    let pending = items
        .iter()
        .filter(|i| i.status == TodoStatus::Pending)
        .count();
    let in_progress = items
        .iter()
        .filter(|i| i.status == TodoStatus::InProgress)
        .count();
    let completed = items
        .iter()
        .filter(|i| i.status == TodoStatus::Completed)
        .count();
    (pending, in_progress, completed)
}

/// 渲染「当前待办」注入段；空清单返回 `None`（不注入整段）。
///
/// 纯函数（无 I/O、无状态）：同一份清单每次渲染逐字节一致。内容本身每轮可变
/// （模型每完成一步就重发整表），所以它**不能**进系统前缀段。
///
/// 输出形如：
///
/// ```text
/// ## 当前待办（3 项，进行中 1）
/// - [ ] 工作目录 L1+L2+L3 —— ToolContext 统一 fallback
/// - [•] 内联 FileChipNode + 消息区 code 渲染器
/// - [x] 真机验证 Lexical 中文 IME
/// 完成一步就重发整表并把它标 completed——不要等最后一起标。
/// ```
///
/// 全部 completed 时**仍然渲染**：模型需要看到「都做完了」，才会去清空清单
/// （`todos: []`）而不是继续找活干。尾行是硬提醒——实测模型倾向攒到最后一起
/// 标完成，中间过程对用户与自身都不可见。
pub fn render_todos_summary(items: &[TodoItem]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let (_, in_progress, _) = count_by_status(items);
    let mut text = format!(
        "## 当前待办（{} 项，进行中 {}）\n",
        items.len(),
        in_progress
    );
    for item in items {
        text.push_str("- ");
        text.push_str(status_mark(item.status));
        text.push(' ');
        // 标题同样压成单行：模型偶尔把换行写进标题，会破坏「一项一行」的块结构
        text.push_str(&squash_whitespace(&item.title));
        if let Some(description) = item
            .description
            .as_deref()
            .map(squash_whitespace)
            .filter(|d| !d.is_empty())
        {
            text.push_str(" —— ");
            text.push_str(&truncate_chars(&description, DESCRIPTION_LIMIT));
        }
        text.push('\n');
    }
    text.push_str("完成一步就重发整表并把它标 completed——不要等最后一起标。\n");
    Some(text)
}

/// 状态符号（与 DSH / OpenCode 的观感一致：方框勾选框）。
fn status_mark(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Pending => "[ ]",
        TodoStatus::InProgress => "[•]",
        TodoStatus::Completed => "[x]",
    }
}

/// 把描述里的换行/连续空白压成单空格并去掉首尾空白。
///
/// 描述可能是多行文本：直接拼进清单会让一条待办占好几行（每轮都进请求，
/// 且破坏「一项一行」的可读性）。
fn squash_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 按**字符**（非字节）截断，超出上限时补省略号。
fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todos::TodoPriority;
    use crate::todos::TodoStatus;

    /// 造一条待办（只填渲染关心的字段）。
    fn item(id: &str, title: &str, status: TodoStatus, description: Option<&str>) -> TodoItem {
        TodoItem {
            id: id.to_string(),
            title: title.to_string(),
            description: description.map(str::to_string),
            status,
            priority: TodoPriority::Medium,
            goal_id: None,
            parent_id: None,
            session_id: Some("s-1".to_string()),
            created_at: 0,
            updated_at: 0,
            completed_at: None,
            due_at: None,
        }
    }

    /// 三种状态各一条 → 状态符号必须逐一对应 `[ ]` / `[•]` / `[x]`。
    ///
    /// 判别力：符号错了或全用同一个，模型就看不出哪些还没做（等于没注入）。
    #[test]
    fn test_render_three_status_marks() {
        let items = vec![
            item("1", "待办项", TodoStatus::Pending, None),
            item("2", "进行项", TodoStatus::InProgress, None),
            item("3", "完成项", TodoStatus::Completed, None),
        ];
        let text = render_todos_summary(&items).expect("非空清单必须渲染");

        assert!(
            text.starts_with("## 当前待办（3 项，进行中 1）\n"),
            "{text}"
        );
        assert!(text.contains("- [ ] 待办项\n"), "{text}");
        assert!(text.contains("- [•] 进行项\n"), "{text}");
        assert!(text.contains("- [x] 完成项\n"), "{text}");
        // 不输出 id（整表替换不需要；省 token）
        assert!(!text.contains("1 "), "不得输出条目 id：{text}");
        // 尾行硬提醒（模型攒到最后一起标完成的实测倾向）
        assert!(
            text.trim_end()
                .ends_with("完成一步就重发整表并把它标 completed——不要等最后一起标。"),
            "缺尾行提醒：{text}"
        );
        // 纯函数：同入参同输出
        assert_eq!(render_todos_summary(&items).as_deref(), Some(text.as_str()));
    }

    /// 空清单 → `None`（整段不注入，不占 token）。
    #[test]
    fn test_render_empty_is_none() {
        assert!(render_todos_summary(&[]).is_none());
    }

    /// 全部 completed 时仍然渲染——模型要知道「都做完了」才会清空清单。
    #[test]
    fn test_render_all_completed_still_rendered() {
        let items = vec![
            item("1", "甲", TodoStatus::Completed, None),
            item("2", "乙", TodoStatus::Completed, None),
        ];
        let text = render_todos_summary(&items).unwrap();
        assert!(text.contains("（2 项，进行中 0）"), "{text}");
        assert_eq!(text.matches("[x]").count(), 2, "{text}");
    }

    /// 描述截断到 60 字符（按字符不按字节），并压成单行。
    #[test]
    fn test_render_description_truncated_to_60_chars() {
        let long: String = "测".repeat(100);
        let items = vec![item("1", "带描述项", TodoStatus::Pending, Some(&long))];
        let text = render_todos_summary(&items).unwrap();
        let line = text
            .lines()
            .find(|l| l.starts_with("- [ ] 带描述项"))
            .expect("应有该条待办行");
        let desc = line
            .split_once("—— ")
            .expect("描述须以「 —— 」接在标题后")
            .1;
        assert_eq!(
            desc.chars().count(),
            DESCRIPTION_LIMIT + 1,
            "60 字符 + 省略号：{desc}"
        );
        assert!(desc.ends_with('…'), "{desc}");
        assert!(desc.chars().take(60).all(|c| c == '测'), "{desc}");

        // 多行描述压成单行（一项一行，每轮进请求）
        let multiline = vec![item(
            "2",
            "多行描述",
            TodoStatus::Pending,
            Some("第一行\n第二行\t第三行"),
        )];
        let text = render_todos_summary(&multiline).unwrap();
        let line = text
            .lines()
            .find(|l| l.starts_with("- [ ] 多行描述"))
            .unwrap();
        assert_eq!(line, "- [ ] 多行描述 —— 第一行 第二行 第三行");
        // 空白描述等于没描述（不渲染空尾巴）
        let blank = vec![item("3", "空描述", TodoStatus::Pending, Some("  \n "))];
        let text = render_todos_summary(&blank).unwrap();
        assert!(text.contains("- [ ] 空描述\n"), "{text}");

        // 标题里的换行同样被压平（否则一条待办会占好几行，破坏块结构）
        let titled = vec![item("4", "标题\n带换行", TodoStatus::Pending, None)];
        let text = render_todos_summary(&titled).unwrap();
        assert!(text.contains("- [ ] 标题 带换行\n"), "{text}");
        assert_eq!(
            text.lines().filter(|l| l.starts_with("- ")).count(),
            1,
            "一条待办只占一行：{text}"
        );
    }

    /// 计数口径（注入摘要头部与工具返回值 counts 共用）。
    #[test]
    fn test_count_by_status() {
        let items = vec![
            item("1", "a", TodoStatus::Pending, None),
            item("2", "b", TodoStatus::InProgress, None),
            item("3", "c", TodoStatus::InProgress, None),
            item("4", "d", TodoStatus::Completed, None),
        ];
        assert_eq!(count_by_status(&items), (1, 2, 1));
        assert_eq!(count_by_status(&[]), (0, 0, 0));
    }
}

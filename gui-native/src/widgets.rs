//! 跨页面复用的小组件：表单外壳、统计卡片、记忆块。
//!
//! 只放「两处以上使用」的构件；只在一页出现的构件留在那个页面的渲染文件里。

use crate::theme::colors;
use crate::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::text::TextView;

/// 表单单行输入框的外壳（去自带外观 + 统一边框底色；与设置页编辑框同一手法）。
pub(crate) fn field_wrap(input: &Entity<InputState>) -> gpui_kit::AnyElement {
    div()
        .w_full()
        .rounded_md()
        .border_1()
        .border_color(rgb(colors::BORDER()))
        .bg(rgb(colors::BG_PRIMARY()))
        .px_2()
        .py_1()
        .child(Input::new(input).appearance(false))
        .into_any_element()
}

/// 洞察页的一个统计卡（标题 + 主值 + 副说明）。
pub(crate) fn stat_card(label: &'static str, value: String, sub: String) -> gpui_kit::AnyElement {
    div()
        .flex_1()
        .min_w(px(130.))
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(rgb(colors::BORDER()))
        .bg(rgb(colors::BG_SECONDARY()))
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(px(10.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child(label),
        )
        .child(
            div()
                .text_size(px(18.))
                .text_color(rgb(colors::TEXT_PRIMARY()))
                .child(value),
        )
        .child(
            div()
                .text_size(px(10.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child(sub),
        )
        .into_any_element()
}

/// 表单多行输入框的外壳（同上）。
pub(crate) fn field_wrap_area(input: &Entity<TextareaState>) -> gpui_kit::AnyElement {
    div()
        .w_full()
        .rounded_md()
        .border_1()
        .border_color(rgb(colors::BORDER()))
        .bg(rgb(colors::BG_PRIMARY()))
        .px_2()
        .py_1()
        .child(Textarea::new(input).appearance(false))
        .into_any_element()
}

/// 把选中的路径展开成待上传文件列表（目录递归；上限 200 个——防手滑导入整盘）。
///
/// 两个入口共用：路径输入（单路径）与系统文件选择器（多选，可含目录）。
pub(crate) fn collect_ingest_files(paths: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    const MAX_FILES: usize = 200;
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = paths.iter().filter(|p| p.exists()).cloned().collect();
    while let Some(p) = stack.pop() {
        if p.is_file() {
            files.push(p);
        } else if p.is_dir() {
            let Ok(entries) = std::fs::read_dir(&p) else {
                continue;
            };
            for entry in entries.flatten() {
                let child = entry.path();
                if child.is_dir() {
                    stack.push(child);
                } else {
                    files.push(child);
                }
            }
        }
        if files.len() >= MAX_FILES {
            break;
        }
    }
    files.truncate(MAX_FILES);
    files
}

/// 文件引用 chip（正文里 `` `@file:"<路径>"` `` 的可视化形态）：图标 + 文件名。
///
/// 两处共用：**输入框草稿区**（外围再包一个 × 删掉它）与**用户消息气泡**（只读）。
/// 只显示**文件名**而不是全路径——全路径长到能把一行撑满（用户实测截图报的
/// 「输入框里一长串 `@file:"F:\work\…"` 原文」）；而路径本身在发送时照常进正文，
/// 模型读到的仍是完整路径，界面上省略的部分不影响语义。
///
/// `on_bubble` = 落在用户消息的强调色气泡上（深底反白）：三套主题的气泡都是
/// 深色（蓝 / 棕），所以用固定白色半透明底与边框，而不是主题里的 `BG_SECONDARY`
/// （那是浅色面，贴上去会变成一块亮斑）。
pub(crate) fn file_chip(path: &str, on_bubble: bool) -> gpui_kit::AnyElement {
    let name = std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string());
    let (fg, bg, border) = if on_bubble {
        (colors::BUBBLE_TEXT(), 0xffffff24, 0xffffff59)
    } else {
        (
            colors::TEXT_SECONDARY(),
            colors::BG_SECONDARY(),
            colors::BORDER(),
        )
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px_2()
        .py(px(3.))
        .rounded_md()
        .border_1()
        .border_color(rgba(border))
        .bg(rgba(bg))
        .text_size(px(11.))
        .text_color(rgb(fg))
        .child(
            div().flex_shrink_0().child(
                svg()
                    .path(IconName::FileText.path())
                    .size(px(11.))
                    .text_color(rgb(fg)),
            ),
        )
        // 文件名最多 28 字（含扩展名）：再长就 `text_ellipsis` 交给布局裁
        .child(
            div()
                .max_w(px(240.))
                .text_ellipsis()
                .child(crate::format::ellipsis(&name, 28)),
        )
        .into_any_element()
}

/// 记忆详情里的一段（L0 摘要 / L1 概览 / L2 全文）。
///
/// 正文走 **Markdown 渲染**：记忆是以 markdown 存进 VFS 的（`# 记忆: xxx` +
/// `- **类别**: fact` + `## 内容` 小节，自演化综述还带列表与加粗）。纯文本渲染
/// 时用户看到的是满屏 `#`、`**`、`- `，等于读源码而不是读文档。
///
/// 注：`TextView::markdown` **不继承父级 `text_color`**（它用组件主题的
/// foreground），所以只能放在普通/次级背景上 —— 消息里的用户气泡是深棕底，
/// 那里不能用（`msg.rs` 里有同一坑的记录）。记忆详情页在这条线内。
pub(crate) fn memory_block(label: &'static str, text: String) -> gpui_kit::AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(px(10.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child(label),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child(TextView::markdown((label, 0usize), text)),
        )
        .into_any_element()
}

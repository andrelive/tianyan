//! 跨页面复用的小组件：表单外壳、统计卡片、记忆块。
//!
//! 只放「两处以上使用」的构件；只在一页出现的构件留在那个页面的渲染文件里。

use crate::*;
use crate::theme::colors;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};

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

/// 记忆详情里的一段（L0 摘要 / L1 概览 / L2 全文）。
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
                .child(text),
        )
        .into_any_element()
}

#![allow(dead_code)]

//! 输入框：带**行内文件 chip** 的自绘编辑器（对齐 vite 的 composer 形态）。
//!
//! ⚠️ 施工中：本文件的 [`Buffer`]（编辑核心 + 原子区间）与单测已落地并被验证，
//! 但**尚未接上** GPUI（渲染 / IME / 集成在下一阶段）。接入前 `dead_code` 是预期的。
//!
//! 为什么不用 `gpui-component` 的 `Textarea`：它是**纯文本控件** —— 内部只有一条
//! shaped text，行内自定义元素没有任何钩子（`set_highlighter` / `TextDecoration`
//! 都只对 `LayoutMode::CodeEditor` 生效）。于是「文件引用和文字排在同一行、且渲染
//! 成 chip」只能自绘：本模块自己做布局（软换行 + 行内原子块）、绘制（文本 shaping +
//! chip 矩形 + 光标 + 选区）与输入（键盘 + IME）。
//!
//! 设计要点（改动前先读）：
//! - **真相源仍是纯文本**：文件引用在正文里就是 `` `@file:"<绝对路径>"` ``（与 Web 版
//!   序列化口径完全一致，后端零改动）。chips 不另存一份状态，而是每次从文本里
//!   **扫描**出来（[`file_refs`]）——文本一改就自然跟着变，不会出现「两份数据漂移」。
//! - **原子区间**：扫描出的区间在编辑时是**不可分割**的（光标不进入其内部，退格在
//!   区间末尾按一次删掉整个引用）。
//! - **状态与绘制分离**：[`Buffer`] 是纯逻辑（文本 + 光标 + 选区，可单测），
//!   [`ComposerEditor`] 是 GPUI 元素（布局 + 绘制），[`ComposerState`] 把两者粘起来
//!   并承载焦点 / IME / 撤销栈。

use std::ops::Range;

/// 文件引用在正文里的开头标记（与 Web 版 `serializeInlineChip` 同口径）。
pub(crate) const FILE_REF_OPEN: &str = "`@file:\"";

/// 文件引用在正文里的结尾标记。
pub(crate) const FILE_REF_CLOSE: &str = "\"`";

/// 输入框里一次「文件引用」的文本区间与路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileRef {
    /// 在正文里的字节区间（含首尾的反引号与引号）。
    pub(crate) range: Range<usize>,
    /// 引用到的绝对路径。
    pub(crate) path: String,
}

/// 扫描正文里的 `` `@file:"<路径>"` ``，返回按位置升序、互不重叠的区间。
///
/// 与 [`crate::msg::split_file_chips`] 同一匹配口径（那里是渲染、这里是编辑的
/// 原子区间）；两边都只认**带反引号的行内形态**——手打的 `@file:"x"` 不产生 chip，
/// 免得用户写着写着文字被吞。
pub(crate) fn file_refs(text: &str) -> Vec<FileRef> {
    let mut out: Vec<FileRef> = Vec::new();
    let mut from = 0usize;
    while let Some(i) = text[from..].find(FILE_REF_OPEN) {
        let start = from + i;
        let body = start + FILE_REF_OPEN.len();
        let Some(j) = text[body..].find(FILE_REF_CLOSE) else {
            break;
        };
        let end = body + j + FILE_REF_CLOSE.len();
        let path = &text[body..body + j];
        if !path.is_empty() && !path.contains('`') {
            out.push(FileRef {
                range: start..end,
                path: path.to_string(),
            });
        }
        from = end;
    }
    out
}

/// 光标位置是否落在某个原子区间**内部**（不含两端）。
fn inside_atomic(spans: &[FileRef], pos: usize) -> Option<&FileRef> {
    spans
        .iter()
        .find(|s| pos > s.range.start && pos < s.range.end)
}

/// 退格时：`pos` 是否正好在某个原子区间的末尾（要整段删掉）。
fn atomic_ending_at(spans: &[FileRef], pos: usize) -> Option<&FileRef> {
    spans.iter().find(|s| s.range.end == pos)
}

/// 删除（Delete）时：`pos` 是否正好在某个原子区间的开头。
fn atomic_starting_at(spans: &[FileRef], pos: usize) -> Option<&FileRef> {
    spans.iter().find(|s| s.range.start == pos)
}

/// 输入框的编辑核心：纯文本 + 光标 + 选区锚点。
///
/// 不依赖 any gpui 类型 —— 编辑语义（含原子区间的跳过与整段删除）全靠单测钉住，
/// 绘制与事件在 [`ComposerState`] 里。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Buffer {
    /// 正文（真相源）。
    pub(crate) text: String,
    /// 光标位置（字节偏移，恒落在字符边界上）。
    pub(crate) cursor: usize,
    /// 选区锚点（字节偏移）；`None` = 没有选区。选中范围是 `anchor..cursor` 的归一化。
    pub(crate) anchor: Option<usize>,
}

impl Buffer {
    /// 用一段文本初始化（光标落在末尾）。
    pub(crate) fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.len();
        Self {
            text,
            cursor,
            anchor: None,
        }
    }

    /// 归一化后的选区（`start..end`；无选区时 `start == end`）。
    pub(crate) fn selection(&self) -> Range<usize> {
        match self.anchor {
            Some(a) if a != self.cursor => a.min(self.cursor)..a.max(self.cursor),
            _ => self.cursor..self.cursor,
        }
    }

    /// 是否有选区。
    pub(crate) fn has_selection(&self) -> bool {
        let sel = self.selection();
        sel.start != sel.end
    }

    /// 当前文本里的文件引用区间。
    pub(crate) fn file_refs(&self) -> Vec<FileRef> {
        file_refs(&self.text)
    }

    /// 把任意字节位置吸附到**字符边界**且**不落在原子区间内部**。
    fn snap(&self, pos: usize) -> usize {
        let mut pos = pos.min(self.text.len());
        while pos > 0 && !self.text.is_char_boundary(pos) {
            pos -= 1;
        }
        let spans = self.file_refs();
        if let Some(span) = inside_atomic(&spans, pos) {
            // 落在引用内部 → 贴到最近的边界（比光标停在中间更符合直觉；删除时整段走）
            return if pos - span.range.start <= span.range.end - pos {
                span.range.start
            } else {
                span.range.end
            };
        }
        pos
    }

    /// 设置光标（`extend` = 保留锚点，即扩展选区）。
    pub(crate) fn set_cursor(&mut self, pos: usize, extend: bool) {
        let pos = self.snap(pos);
        if !extend {
            self.anchor = None;
        } else if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        self.cursor = pos;
    }

    /// 全选。
    pub(crate) fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
    }

    /// 删掉选区，返回是否删了东西。
    pub(crate) fn delete_selection(&mut self) -> bool {
        let sel = self.selection();
        if sel.start == sel.end {
            return false;
        }
        self.text.replace_range(sel.clone(), "");
        self.cursor = sel.start;
        self.anchor = None;
        true
    }

    /// 在光标处插入一段文本（有选区时先替换选区）。
    pub(crate) fn insert(&mut self, s: &str) {
        self.delete_selection();
        let at = self.cursor.min(self.text.len());
        self.text.insert_str(at, s);
        self.cursor = at + s.len();
        self.anchor = None;
    }

    /// 插入一个文件引用（原子块）。
    pub(crate) fn insert_file_ref(&mut self, path: &str) {
        self.insert(&format!("{FILE_REF_OPEN}{path}{FILE_REF_CLOSE}"));
    }

    /// 退格：有选区先删选区；否则删一个字 / 整个原子块。
    pub(crate) fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor == 0 {
            return;
        }
        let spans = self.file_refs();
        if let Some(span) = atomic_ending_at(&spans, self.cursor) {
            // 光标贴在引用末尾 → 一次删掉整个引用
            let start = span.range.start;
            let end = span.range.end;
            self.text.replace_range(start..end, "");
            self.cursor = start;
            self.anchor = None;
            return;
        }
        let start = self.prev_char_boundary(self.cursor);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.anchor = None;
    }

    /// 删除（Delete 键）：有选区先删选区；否则删后一个字 / 整个原子块。
    pub(crate) fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor >= self.text.len() {
            return;
        }
        let spans = self.file_refs();
        if let Some(span) = atomic_starting_at(&spans, self.cursor) {
            let start = span.range.start;
            let end = span.range.end;
            self.text.replace_range(start..end, "");
            self.anchor = None;
            return;
        }
        let end = self.next_char_boundary(self.cursor);
        self.text.replace_range(self.cursor..end, "");
        self.anchor = None;
    }

    /// 光标左/右移一格（跳过原子块内部；跨过整个引用算一格）。
    pub(crate) fn move_h(&mut self, forward: bool, extend: bool) {
        let prev_anchor = self.anchor;
        let mut pos = self.cursor;
        if !extend && self.has_selection() {
            // 有选区且不扩展 → 光标收到选区对应端
            let sel = self.selection();
            pos = if forward { sel.end } else { sel.start };
        } else if forward {
            if pos < self.text.len() {
                let spans = self.file_refs();
                // 若正处在某个引用起点，一次跨到引用末尾
                if let Some(span) = atomic_starting_at(&spans, pos) {
                    pos = span.range.end;
                } else {
                    pos = self.next_char_boundary(pos);
                    if let Some(span) = inside_atomic(&spans, pos) {
                        pos = span.range.end;
                    }
                }
            }
        } else if pos > 0 {
            let spans = self.file_refs();
            if let Some(span) = atomic_ending_at(&spans, pos) {
                pos = span.range.start;
            } else {
                pos = self.prev_char_boundary(pos);
                if let Some(span) = inside_atomic(&spans, pos) {
                    pos = span.range.start;
                }
            }
        }
        if extend {
            if prev_anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = pos;
    }

    /// 行首 / 行尾（`Ctrl+Home` / `Ctrl+End` 的整段版在调用方处理）。
    pub(crate) fn move_line_edge(&mut self, to_end: bool, extend: bool) {
        let start = self.line_start(self.cursor);
        let end = self.line_end(self.cursor);
        if extend && self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        } else if !extend {
            self.anchor = None;
        }
        self.cursor = if to_end { end } else { start };
    }

    /// 当前行的起始字节（`\n` 之后）。
    pub(crate) fn line_start(&self, pos: usize) -> usize {
        let pos = self.floor_boundary(pos);
        self.text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    /// 当前行的结束字节（`\n` 之前）。
    pub(crate) fn line_end(&self, pos: usize) -> usize {
        let pos = self.floor_boundary(pos);
        self.text[pos..]
            .find('\n')
            .map(|i| pos + i)
            .unwrap_or(self.text.len())
    }

    /// 向下取整到字符边界（切片前必做：字节索引可能落在多字节字符中间）。
    pub(crate) fn floor_boundary(&self, pos: usize) -> usize {
        let mut p = pos.min(self.text.len());
        while p > 0 && !self.text.is_char_boundary(p) {
            p -= 1;
        }
        p
    }

    /// 上一个字符边界。
    pub(crate) fn prev_char_boundary(&self, pos: usize) -> usize {
        let mut p = pos.min(self.text.len());
        if p == 0 {
            return 0;
        }
        p -= 1;
        while p > 0 && !self.text.is_char_boundary(p) {
            p -= 1;
        }
        p
    }

    /// 下一个字符边界。
    pub(crate) fn next_char_boundary(&self, pos: usize) -> usize {
        let mut p = pos.min(self.text.len());
        if p >= self.text.len() {
            return self.text.len();
        }
        p += 1;
        while p < self.text.len() && !self.text.is_char_boundary(p) {
            p += 1;
        }
        p
    }

    /// 设置整段文本（光标落到末尾，清选区）。
    pub(crate) fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.anchor = None;
    }
}

#[cfg(test)]
mod tests {
    //! 编辑语义的回归：原子区间（文件引用）必须「整体跳过、整体删除」，
    //! 否则退格退到一半会留下 `` `@file:"F:\wo `` 这种半截引用，发送时被当成普通文本。
    use super::*;

    fn buf(text: &str, cursor: usize) -> Buffer {
        Buffer {
            text: text.to_string(),
            cursor,
            anchor: None,
        }
    }

    #[test]
    fn file_refs_scans_inline_markers() {
        let text = r#"看下 `@file:"C:\a b\c.md"` 和 `@file:"/x/y.rs"`"#;
        let refs = file_refs(text);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].path, r"C:\a b\c.md");
        assert_eq!(refs[1].path, "/x/y.rs");
        // 区间正好覆盖完整的标记（含反引号与引号）
        assert_eq!(&text[refs[0].range.clone()], r#"`@file:"C:\a b\c.md"`"#);
    }

    #[test]
    fn hand_typed_marker_without_backticks_is_plain_text() {
        let text = r#"把 @file:"x.ts" 放这里"#;
        assert!(file_refs(text).is_empty());
        // 退格只删一个字（不会整段消失）
        let mut b = buf(text, text.len());
        b.backspace();
        assert_eq!(b.text.chars().count(), text.chars().count() - 1);
    }

    #[test]
    fn unclosed_marker_is_not_atomic() {
        let text = "把 `@file:\"x.ts 放这里";
        assert!(file_refs(text).is_empty());
    }

    #[test]
    fn backspace_at_ref_end_removes_whole_ref() {
        let text = r#"看 `@file:"C:\a.md"` 后面"#;
        let refs = file_refs(text);
        let end = refs[0].range.end;
        let mut b = buf(text, end);
        b.backspace();
        assert_eq!(b.text, "看  后面");
        // 「看 」= 3 字节 + 1 个空格 = 4 字节（按字节算，不是按字算）
        assert_eq!(b.cursor, 4, "光标落在引用原来的起始处");
    }

    #[test]
    fn delete_at_ref_start_removes_whole_ref() {
        let text = r#"看 `@file:"C:\a.md"` 后面"#;
        let refs = file_refs(text);
        let start = refs[0].range.start;
        let mut b = buf(text, start);
        b.delete();
        assert_eq!(b.text, "看  后面");
        assert_eq!(b.cursor, start);
    }

    #[test]
    fn cursor_never_lands_inside_ref() {
        let text = r#"ab`@file:"C:\a.md"`cd"#;
        let refs = file_refs(text);
        let inside = refs[0].range.start + 3;
        let b = buf(text, inside);
        let snapped = b.snap(inside);
        assert!(
            snapped == refs[0].range.start || snapped == refs[0].range.end,
            "应吸附到边界，实际 {snapped}"
        );
    }

    #[test]
    fn horizontal_move_jumps_over_ref() {
        let text = r#"ab`@file:"C:\a.md"`cd"#;
        let refs = file_refs(text);
        let mut b = buf(text, 2); // "ab" 之后、引用之前
        b.move_h(true, false);
        assert_eq!(b.cursor, refs[0].range.end, "右移一格应跨过整个引用");
        b.move_h(false, false);
        assert_eq!(b.cursor, refs[0].range.start, "左移一格同理");
    }

    #[test]
    fn insert_replaces_selection() {
        let mut b = buf("hello world", 5);
        b.anchor = Some(0);
        b.cursor = 5;
        b.insert("HI");
        assert_eq!(b.text, "HI world");
        assert_eq!(b.cursor, 2);
        assert!(!b.has_selection());
        // 顺带确认 selection() 归一化（anchor 在 cursor 右侧时也对）
        let mut b2 = buf("abcdef", 1);
        b2.anchor = Some(4);
        let sel: Range<usize> = b2.selection();
        assert_eq!(sel, 1..4);
    }

    #[test]
    fn insert_file_ref_serializes_like_web() {
        let mut b = Buffer::default();
        b.insert("看这个 ");
        b.insert_file_ref(r"C:\a b\c.md");
        assert_eq!(b.text, "看这个 `@file:\"C:\\a b\\c.md\"`");
        assert_eq!(file_refs(&b.text).len(), 1);
    }

    #[test]
    fn line_start_end_are_newline_bounded() {
        // 每个汉字 3 字节：「第一行」= 9 字节，`\n` 在 9，第二行起点 = 10
        let b = buf("第一行\n第二行\n第三行", 13);
        assert_eq!(b.line_start(13), 10);
        assert_eq!(b.line_end(13), 19);
        // 落在多字节字符中间也不能 panic（向下取整到字符边界）
        assert_eq!(b.line_start(12), 10);
    }
}

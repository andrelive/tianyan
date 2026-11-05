//! ANSI 控制序列剥离。
//!
//! ## 为什么需要
//!
//! 转义码对**终端**有意义（颜色、高亮、光标位置），对**工具结果**只有坏处。
//! 实测：PowerShell 的 `Select-String` 会给每处匹配加反向高亮（`ESC[7m…ESC[0m`），
//! 这些字节随 stdout 原样回传给模型，结果里尽是 `\u001b[7m` ——
//! 既占 token，又让用户误以为输出里有乱码或报错（一次 209 秒的测试输出被这些
//! 序列淹了）。同样的命令在终端里跑，高亮是给人看的；而命令工具返回的是纯文本，
//! 控制码不承载它需要的任何信息。
//!
//! ## 覆盖范围
//!
//! - **CSI**：`ESC [ … 终止字节(0x40–0x7E)` —— 颜色、加粗、光标移动、清屏；
//! - **OSC**：`ESC ] … BEL` 或 `ESC ] … ESC \` —— 窗口标题、超链接；
//! - 其它单字节转义：只吞掉 `ESC` 本身，保留后续可读字符（宁可保留也不要吞内容）。
//!
//! 不含 `ESC` 的输入**零拷贝**返回（`Cow::Borrowed`），不给正常输出加分配成本。

use std::borrow::Cow;

/// 剥离 ANSI 转义序列；输入不含 `ESC` 时零拷贝返回原文。
pub fn strip_ansi(input: &str) -> Cow<'_, str> {
    if !input.contains('\u{1b}') {
        return Cow::Borrowed(input);
    }

    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        match chars.peek() {
            // CSI：ESC [ 参数/中间字节 终止字节
            Some('[') => {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                }
            }
            // OSC：ESC ] … BEL 或 ESC \
            Some(']') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' {
                        // `ESC \`（ST）终止；吞掉反斜杠
                        chars.next();
                        break;
                    }
                }
            }
            // 其它转义：吞掉 ESC，其余字符照常输出
            _ => {}
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_sgr_highlight() {
        let input = "test marks_\u{1b}[7mfailed\u{1b}[0m_review ok";
        assert_eq!(strip_ansi(input), "test marks_failed_review ok");
    }

    #[test]
    fn strips_cursor_and_clear_sequences() {
        let input = "\u{1b}[2J\u{1b}[1;1Hdone";
        assert_eq!(strip_ansi(input), "done");
    }

    #[test]
    fn strips_osc_title() {
        let input = "\u{1b}]0;window title\u{7}content";
        assert_eq!(strip_ansi(input), "content");
        let input2 = "\u{1b}]8;;http://x\u{1b}\\label";
        assert_eq!(strip_ansi(input2), "label");
    }

    #[test]
    fn borrows_when_no_escape() {
        let input = "plain output";
        assert!(matches!(strip_ansi(input), Cow::Borrowed(_)));
        assert_eq!(strip_ansi(input), "plain output");
    }

    #[test]
    fn keeps_readable_text_after_stray_escape() {
        // 孤立 ESC 后跟普通字符：只吞 ESC，内容不丢
        assert_eq!(strip_ansi("a\u{1b}b"), "ab");
    }

    #[test]
    fn preserves_utf8_content() {
        let input = "\u{1b}[31m中文输出\u{1b}[0m 正常";
        assert_eq!(strip_ansi(input), "中文输出 正常");
    }
}

//! 二进制内容嗅探原语。
//!
//! 供 executor（文件读取预览）与 snapshot（diff 二进制判定）复用的
//! 纯函数——历史上存在两份逐字近似的实现（`executor::actions::sniff_binary`、
//! `snapshot::looks_binary`），统一收敛于此（ADR-007 共享基础设施归属被依赖方先例）。

/// 启发式判断内容是否为二进制：含 NUL 字节，或不可打印控制字符
/// （除 `\n` `\r` `\t` 外 < 0x20 及 0x7f）占比严格超过 30%。
///
/// UTF-8 多字节字符（≥ 0x80）不计为控制字符，中文文本不会被误判。
/// 空内容返回 `false`。
pub fn sniff_binary(bytes: &[u8]) -> bool {
    let mut non_printable = 0usize;
    for &b in bytes {
        if b == 0 {
            return true;
        }
        if (b < 0x20 || b == 0x7f) && b != b'\n' && b != b'\r' && b != b'\t' {
            non_printable += 1;
        }
    }
    !bytes.is_empty() && non_printable * 100 > bytes.len() * 30
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nul_byte_is_binary() {
        assert!(sniff_binary(b"ab\x00cd"));
    }

    #[test]
    fn test_empty_is_not_binary() {
        assert!(!sniff_binary(b""));
    }

    #[test]
    fn test_utf8_text_is_not_binary() {
        assert!(!sniff_binary("hello world\n第二行\t缩进\r\n".as_bytes()));
    }

    #[test]
    fn test_cjk_text_is_not_binary() {
        assert!(!sniff_binary("中文文本不会误判为二进制".as_bytes()));
    }

    #[test]
    fn test_control_chars_above_threshold() {
        // 31/100 控制字符 > 30% → 二进制
        let mut bytes = vec![b' '; 100];
        for b in bytes.iter_mut().take(31) {
            *b = 0x01;
        }
        assert!(sniff_binary(&bytes));
    }

    #[test]
    fn test_control_chars_at_threshold_not_binary() {
        // 恰好 30/100 = 30% 非严格超过 → 非二进制（严格大于语义）
        let mut bytes = vec![b' '; 100];
        for b in bytes.iter_mut().take(30) {
            *b = 0x01;
        }
        assert!(!sniff_binary(&bytes));
    }

    #[test]
    fn test_whitespace_controls_excluded() {
        // \n \r \t 不计为控制字符
        let bytes = b"\n\r\t\n\r\t\n\r\t";
        assert!(!sniff_binary(bytes));
    }

    #[test]
    fn test_del_is_control() {
        // 0x7f (DEL) 计入控制字符
        let mut bytes = vec![b' '; 10];
        for b in bytes.iter_mut().take(4) {
            *b = 0x7f;
        }
        assert!(sniff_binary(&bytes));
    }
}

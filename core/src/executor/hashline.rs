//! 行锚点工具：为 apply_edit 生成/解析 `N#ID|content` 形式的行锚点。
//!
//! 锚点 ID 是对行内容做空白不敏感哈希得到的 2 位十六进制短码，
//! 使编辑工具能容忍行首缩进/尾随空白的细微差异。

/// 计算行的空白不敏感哈希。
///
/// 剥离行内所有空白字符后，对剩余字节计算 FNV-1a 32 位哈希，
/// 返回低字节的 2 位小写十六进制形式（如 `"3f"`）。
/// 结果跨运行确定，相同内容哈希稳定。
pub fn line_hash(line: &str) -> String {
    let stripped: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    let mut hash: u32 = 0x811c_9dc5;
    for byte in stripped.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{:02x}", (hash & 0xff) as u8)
}

/// 生成 `N#ID|content` 锚点行（`n` 为 1 起始的行号）。
pub fn format_line(n: usize, id: &str, content: &str) -> String {
    format!("{n}#{id}|{content}")
}

/// 解析 `N#ID|content` 前缀，返回 `(行号, ID)`。
///
/// 行内不存在有效的 `#` .. `|` 模式（或数字前缀无法解析为 `usize`）时返回 `None`。
pub fn parse_anchor(line: &str) -> Option<(usize, String)> {
    let hash_pos = line.chars().position(|c| c == '#')?;
    let mut id = String::new();
    for c in line.chars().skip(hash_pos + 1) {
        if c == '|' {
            let n: usize = line
                .chars()
                .take(hash_pos)
                .collect::<String>()
                .parse()
                .ok()?;
            return Some((n, id));
        }
        id.push(c);
    }
    None
}

/// 便捷组合：`format_line(n, &line_hash(content), content)`。
pub fn hash_line_pair(n: usize, content: &str) -> String {
    format_line(n, &line_hash(content), content)
}

#[cfg(test)]
#[path = "hashline_tests.rs"]
mod tests;

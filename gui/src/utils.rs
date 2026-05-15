/// 格式化 ISO 时间戳为日期显示（仅日期部分）
pub fn format_date(iso: &str) -> String {
    iso.split('T').next().unwrap_or(iso).to_string()
}

/// 格式化 ISO 时间戳为日期时间显示
pub fn format_datetime(iso: &str) -> String {
    iso.replace('T', " ")
        .split('.')
        .next()
        .unwrap_or(iso)
        .to_string()
}

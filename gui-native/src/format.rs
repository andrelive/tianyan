//! 纯格式化与解析工具：时间 / 数字 / 字节数 / 文本缩写。
//!
//! 全部是「值进、字符串出」的小函数，不碰 `ClientApp` 状态。
//! 放一处的理由：这些口径散在渲染代码里时，同一个「几秒/几分钟」会出现两种写法。

use crate::theme::colors;

/// 本地生成会话 id：服务端 `resolve_or_create_session` 接受前端传入的 id
/// 并在不存在时按它新建（`server/src/api/chat/services.rs`），因此新会话
/// 不必等 `POST /chat/stream` 的响应——事件按 `session_id` 路由，id 必须
/// 在**发起前**就确定，否则首批流事件会被当成别的会话丢掉。
pub(crate) fn new_session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("session-{nanos:x}")
}
/// 千分位分组（`1234567` → `1,234,567`；对齐 Web 版 `formatNumber`）。
pub(crate) fn group_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// 毫秒 → 人类可读（`820` → `820 ms`，`1530` → `1.5 s`）。
pub(crate) fn format_ms(ms: i64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.1} s", ms as f64 / 1000.0)
    }
}

/// 生成速度（tok/s）= 输出 token / **纯生成时长**（扣掉首 token 等待）。
///
/// 分母扣 TTFT 是 Web 版的口径：首包等待不属于生成阶段，否则慢首包会把速度
/// 一起拖低，两个指标就失去正交性。
pub(crate) fn tokens_per_sec(ttft_ms: Option<i64>, duration_ms: i64, completion: u64) -> Option<u64> {
    let gen_ms = duration_ms - ttft_ms.unwrap_or(0);
    if completion == 0 || gen_ms <= 0 {
        return None;
    }
    Some((completion as f64 / (gen_ms as f64 / 1000.0)).round() as u64)
}

/// 当前 Unix 时间（秒）。
pub(crate) fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `HH:MM` → `(小时, 分钟)`（时区换算用；解析不出来按 0 处理）。
pub(crate) fn split_hhmm(s: &str) -> (i64, i64) {
    let hh = s.get(0..2).and_then(|v| v.parse().ok()).unwrap_or(0);
    let mm = s.get(3..5).and_then(|v| v.parse().ok()).unwrap_or(0);
    (hh, mm)
}

/// 解析 RFC3339 时间戳为 Unix 秒。
///
/// 手写而不引 `chrono`：左栏只需要「多久以前」这一档精度，为它加一个依赖不值。
/// 支持 `...Z`（UTC）、`...+08:00` / `...-05:00`（换算到 UTC）与小数秒
/// （`2026-10-27T06:45:44.885328Z` —— 小数部分丢弃）。
pub(crate) fn parse_rfc3339_secs(s: &str) -> Option<i64> {
    if s.len() < 19 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let minute: i64 = s.get(14..16)?.parse().ok()?;
    let second: i64 = s.get(17..19)?.parse().ok()?;
    // 公历 → 距 1970-01-01 的天数（Howard Hinnant 的 days_from_civil）
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut secs = days * 86_400 + hour * 3_600 + minute * 60 + second;
    // 时区在**小数秒之后**：`...55.122+08:00` / `...44.885328Z`。别只看第 20 个
    // 字节——后端的时间戳两种形态都有（UTC 带 Z 的与本地带 +08:00 的混用）。
    let tail = s.get(19..).unwrap_or("");
    if let Some(pos) = tail.find(['+', '-']) {
        let tz = &tail[pos..];
        if let Some(rest) = tz.strip_prefix('+') {
            let (hh, mm) = split_hhmm(rest);
            secs -= hh * 3_600 + mm * 60;
        } else if let Some(rest) = tz.strip_prefix('-') {
            let (hh, mm) = split_hhmm(rest);
            secs += hh * 3_600 + mm * 60;
        }
    }
    Some(secs)
}

/// 相对时间文案（对齐 Tauri 版 `formatRelativeTime` 的口径）。
///
/// 超过一周就退化成年月日（`MM/DD`），与 Web 版一致 —— 「37 天前」不如日期可读。
pub(crate) fn format_relative(ts: &str) -> String {
    let Some(secs) = parse_rfc3339_secs(ts) else {
        return String::new();
    };
    let delta = now_unix_secs() - secs;
    if delta < 60 {
        "刚刚".to_string()
    } else if delta < 3_600 {
        format!("{} 分钟前", delta / 60)
    } else if delta < 86_400 {
        format!("{} 小时前", delta / 3_600)
    } else if delta < 86_400 * 7 {
        format!("{} 天前", delta / 86_400)
    } else {
        ts.get(5..10)
            .map(|m| m.replace('-', "/"))
            .unwrap_or_default()
    }
}

/// 工作目录 → 分组展示名（末级目录名；默认组显示「默认」）。
pub(crate) fn group_label(workdir: &str) -> String {
    if workdir.is_empty() {
        return "默认".to_string();
    }
    workdir
        .split(['\\', '/'])
        // `rfind` 而不是 `filter().last()`：`last()` 会把整条迭代器跑完，
        // `rfind` 从尾部找第一个满足条件的立刻返回
        .rfind(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| workdir.to_string())
}

/// 单行省略（ListItem 硬约束：每项必须等高，多行会溢出重叠）
pub(crate) fn ellipsis(s: &str, max_chars: usize) -> String {
    let n = s.chars().count();
    if n <= max_chars {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max_chars).collect::<String>())
    }
}

/// RFC3339 → `MM-DD HH:MM`（面板列表只要到分钟；不引 chrono）。
pub(crate) fn short_stamp(s: &str) -> String {
    match (s.get(5..10), s.get(11..16)) {
        (Some(d), Some(t)) => format!("{d} {t}"),
        _ => s.to_string(),
    }
}

/// 秒 → 人类可读间隔（`1800` → `每 30 分钟`；对齐 Web 版 `formatInterval`）。
pub(crate) fn format_interval(secs: u64) -> String {
    if secs >= 604_800 && secs.is_multiple_of(604_800) {
        format!("每 {} 周", secs / 604_800)
    } else if secs >= 86_400 && secs.is_multiple_of(86_400) {
        format!("每 {} 天", secs / 86_400)
    } else if secs >= 3_600 && secs.is_multiple_of(3_600) {
        format!("每 {} 小时", secs / 3_600)
    } else if secs >= 60 && secs.is_multiple_of(60) {
        format!("每 {} 分钟", secs / 60)
    } else {
        format!("每 {secs} 秒")
    }
}

/// Unix 秒 → 相对当前时间的短描述（`-1800` → `30 分钟后` / `+1800` → `30 分钟前`）。
///
/// 定时任务的「上次 / 下次」用相对时间比绝对时间戳有用：间隔制下关心的是
/// 「还有多久跑」而不是「几点几分」。精度只到分钟/小时/天，不引 chrono。
pub(crate) fn relative_from_now(ts: i64) -> String {
    let diff = ts - now_unix_secs();
    let abs = diff.abs();
    if abs < 60 {
        return "刚刚".to_string();
    }
    let unit = if abs < 3_600 {
        format!("{} 分钟", abs / 60)
    } else if abs < 86_400 {
        format!("{} 小时", abs / 3_600)
    } else {
        format!("{} 天", abs / 86_400)
    };
    if diff >= 0 {
        format!("{unit}后")
    } else {
        format!("{unit}前")
    }
}

/// 秒 → 粗粒度描述（`2303` → `38 分钟`；「下次还有多久」用）。
pub(crate) fn format_secs_short(secs: u64) -> String {
    if secs < 60 {
        format!("{secs} 秒")
    } else if secs < 3_600 {
        format!("{} 分钟", secs / 60)
    } else if secs < 86_400 {
        format!("{} 小时", secs / 3_600)
    } else {
        format!("{} 天", secs / 86_400)
    }
}

/// 风险等级 → 颜色（审批页与对话横幅共用口径）。
pub(crate) fn risk_color(risk: &str) -> u32 {
    match risk.to_lowercase().as_str() {
        "low" => colors::SUCCESS(),
        "medium" => colors::WARNING(),
        _ => colors::ERROR(),
    }
}

/// 两个数据目录字符串是否指向同一处（忽略大小写、斜杠方向、尾部斜杠与 `\\?\` 前缀）。
///
/// 用途：设置页「连接」栏目的「配置 vs 服务实际」一致性提示，以及自动化的
/// `health.dir_matches_config` 断言。
///
/// **必须剥掉 `\\?\`（verbatim）前缀**：`/health` 的 `data_dir` 走
/// `common::path::normalize_dir`，Windows 上带 `\\?\`；而配置里用户写的是
/// `D:\tianyan`。不剥前缀就会误报「改了配置还没重启」（实测踩到）。
///
/// 不做完整路径语义（`..` / 符号链接不展开）——规范化口径的权威实现在
/// `core::common::path`，客户端不重复实现它，这里只服务于「看起来是不是同一个」。
pub(crate) fn same_dir(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let raw = s.trim();
        // UNC 的 verbatim 形态 `\\?\UNC\server\share` 对应普通形态 `\\server\share`
        let unc = raw
            .strip_prefix(r"\\?\UNC\")
            .map(|rest| format!(r"\\{rest}"));
        let plain = match unc {
            Some(p) => p,
            None => raw.strip_prefix(r"\\?\").unwrap_or(raw).to_string(),
        };
        plain.replace('/', "\\").trim_end_matches('\\').to_lowercase()
    };
    norm(a) == norm(b)
}

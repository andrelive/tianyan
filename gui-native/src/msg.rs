//! 消息解析与渲染：把后端 `structured message` 变成可折叠的界面块。
//!
//! 两个层次：
//! - **解析**：`parse_segments` / `parse_msg_usage` / `parse_msg_images` —— JSON → 结构；
//! - **渲染**：`render_msg_body` 及其子函数（思考块 / 工具卡 / 页脚 / 折叠态）。
//!
//! 折叠态放 `Rc<RefCell>` 而不是 `ClientApp` 字段：`gpui_list` 的渲染闭包签名是
//! `FnMut(usize, &mut Window, &mut App)`，**拿不到** `&ClientApp`；而点击回调又必须
//! 能把状态写回并触发重绘——共享句柄 + `AppRef` 是这套框架下不引入第二份真相的写法。

use crate::theme::colors;
use crate::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::text::TextView;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// 用户消息里图片的显示上限（对齐 vite `max-w-[240px] max-h-[240px]`）。
///
/// 宽度取**气泡内宽**：气泡上限 640 − `px_4` 两侧各 16 = 608。宽度上限比高度
/// 宽松是有意的——横截图（1024×640）按高度 240 缩放后是 384×240，正是 vite 的
/// 观感；窄窗口下再由气泡自身收窄。
const BUBBLE_IMG_MAX_W: f32 = 608.0;
const BUBBLE_IMG_MAX_H: f32 = 240.0;

/// 从消息 JSON 取 `usage`；`prompt_tokens == 0` 视为没有（老消息 / 提供商未上报）。
pub(crate) fn parse_msg_usage(msg: &serde_json::Value) -> Option<MsgUsage> {
    let u = msg.get("usage")?;
    let prompt_tokens = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    if prompt_tokens == 0 {
        return None;
    }
    Some(MsgUsage {
        prompt_tokens,
        completion_tokens: u
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_read: u.get("cache_read").and_then(|v| v.as_u64()).unwrap_or(0),
    })
}

/// 消息里的图片 data URL 列表（仅用户消息；空 = 没有图片）。
pub(crate) fn parse_msg_images(msg: &serde_json::Value) -> Vec<String> {
    msg.get("images")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// 后端 `timestamp`（RFC3339）→ 本地 `HH:mm:ss`；缺失/非法返回 `None`。
///
/// 注：历史消息后端**不下发** `timestamp`（`ChatMessage.timestamp` 恒 None），
/// 所以历史消息不显示时间——与 vite 一致（它只在流式轮次开始时本地打点）。
pub(crate) fn parse_msg_ts(msg: &serde_json::Value) -> Option<String> {
    let raw = msg.get("timestamp").and_then(|v| v.as_str())?;
    let dt = chrono::DateTime::parse_from_rfc3339(raw).ok()?;
    Some(
        dt.with_timezone(&chrono::Local)
            .format("%H:%M:%S")
            .to_string(),
    )
}

/// 当前本地时钟 `HH:mm:ss`（新一轮 assistant 消息打点用）。
pub(crate) fn now_clock() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// 本地图片文件 → data URL（`data:image/png;base64,...`）。
///
/// 后端 `ChatRequest.message.images` 收的就是这个形式（vite 直传 data URL）。
/// 非图片扩展名 / 读失败 / 超过 8MB 返回 `None`（该张跳过，不阻断整批）。
pub(crate) fn image_path_to_data_url(path: &std::path::Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => return None,
    };
    let bytes = std::fs::read(path).ok()?;
    let (mime, bytes) = normalize_image_for_api(mime, &bytes)?;
    bytes_to_data_url(&mime, &bytes)
}

/// 后端接受的图片 MIME 白名单（与 `server/src/api/chat/types.rs` 的
/// `ACCEPTED_IMAGE_DATA_URL_PREFIXES` 同一口径）。
pub(crate) const ACCEPTED_IMAGE_MIMES: [&str; 5] = [
    "image/png",
    "image/jpeg",
    "image/jpg",
    "image/webp",
    "image/gif",
];

/// 任意图片字节 → **后端认的** `(mime, 字节)`。
///
/// 为什么需要：Windows 的剪贴板截图（Win+Shift+S / PrintScreen）给的是 **BMP**
/// （`CF_DIB`），而后端 data URL 白名单只有 PNG/JPEG/WebP/GIF —— 原样发
/// `data:image/bmp;...` 被 400 拒掉（实测：「不支持的图片格式（仅支持
/// PNG/JPEG/WebP/GIF）」）。非白名单格式在此解码后重编码成 PNG：无损，且
/// 截图 BMP 转 PNG 通常小一个量级。
///
/// 解码失败（数据损坏 / 缺对应 decoder）返回 `None`，调用方跳过该张不阻断整批。
pub(crate) fn normalize_image_for_api(mime: &str, bytes: &[u8]) -> Option<(String, Vec<u8>)> {
    if ACCEPTED_IMAGE_MIMES.contains(&mime) {
        return Some((mime.to_string(), bytes.to_vec()));
    }
    let decoded = image::load_from_memory(bytes).ok()?;
    let mut out = std::io::Cursor::new(Vec::new());
    decoded.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(("image/png".to_string(), out.into_inner()))
}

/// 图片字节 → data URL（`data:image/png;base64,...`）。
///
/// 两个入参都从 **剪贴板粘贴**（`ClipboardEntry::Image` 直接给原始字节 + 格式）与
/// **文件选择**共用：后端 `ChatRequest.message.images` 收的就是 data URL。
///
/// 8MB 上限：超大图既拖慢 base64 与 SSE 传输，也会撑爆模型上下文——超限直接跳过
/// （不阻断整批）。
pub(crate) fn bytes_to_data_url(mime: &str, bytes: &[u8]) -> Option<String> {
    use base64::Engine as _;
    if bytes.is_empty() || bytes.len() > 8 * 1024 * 1024 {
        return None;
    }
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// 解 data URL（`data:image/png;base64,...`）→ `(格式, 原始字节)`。
///
/// GPUI 的 `img()` 不认 data URL，但 `Image::from_bytes` 直接吃编码后的字节
/// （内部按 `ImageFormat` 解码），所以**不需要落盘**。非法/不支持的格式返回
/// `None`——图片退化为不渲染，不影响文字与气泡。
pub(crate) fn decode_data_url(src: &str) -> Option<(gpui_kit::ImageFormat, Vec<u8>)> {
    use base64::Engine as _;
    let rest = src.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    if !meta.contains(";base64") {
        return None;
    }
    let mime = meta.split(';').next().unwrap_or_default();
    let format = match mime {
        "image/png" => gpui_kit::ImageFormat::Png,
        "image/jpeg" | "image/jpg" => gpui_kit::ImageFormat::Jpeg,
        "image/webp" => gpui_kit::ImageFormat::Webp,
        "image/gif" => gpui_kit::ImageFormat::Gif,
        "image/bmp" => gpui_kit::ImageFormat::Bmp,
        _ => return None,
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .ok()?;
    Some((format, bytes))
}

/// 读一张编码图片的**原始像素尺寸**（只解头部，不解全图）。
///
/// 为什么不用 `image::load_from_memory`：那个要整张解码（大截图 ~10ms），而这里
/// 是**渲染期**按需调用，每条消息每帧都可能命中，必须只读 header。
pub(crate) fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.into_dimensions().ok()
}

/// 按「宽高都不超过上限」等比缩放，返回**要显式设置的** `(w, h)`（px）。
///
/// 为什么必须自己算，而不是交给 `.max_w()/.max_h()`：
/// GPUI 的 `img()` 在 `size` 为 Auto 时会把元素尺寸设成**图片原始像素**
/// （实测算过：1024×640 的截图 → `style.size = 1024×640`），随后 `max_w` 与
/// `max_h` 是**两次独立钳制**（各自取 min），并不等比。于是
/// `max_w(240) + max_h(240)` 得到的是 **240×240 的正方框**，图片再按
/// `ObjectFit::Contain` 居中画成 240×150 —— 上下各 45px 空白（用户实测
/// 「图片上下有大量空白」）。显式 `w`/`h` 才是真正的等比缩放。
///
/// 小图不放大（`scale` 上限 1.0），与 vite 的 `max-w/max-h` 同语义。
pub(crate) fn fit_image_size(w: u32, h: u32, max_w: f32, max_h: f32) -> (f32, f32) {
    let w = w.max(1) as f32;
    let h = h.max(1) as f32;
    let scale = (max_w / w).min(max_h / h).min(1.0);
    ((w * scale).max(1.0).round(), (h * scale).max(1.0).round())
}

/// 只拼 `text` 段（**不含**思考与工具）——`MsgRow.text` 的取值口径。
///
/// 历史教训（务必别改回去）：这里曾经是「把 segments 全拍平成一段可读文本」，
/// 思考段渲染成 Markdown 引用块 `> 🧠 思考：…`。当思考块末尾紧接正文时，
/// CommonMark 的 **lazy continuation** 会把正文第一段并进引用块——实测症状是
/// 「思考块里吞掉了回复的第一段」，正文直到第一个空行才出现；同时 `「**」`
/// 这类行内标记也被当引用块内的懒续行处理，加粗渲染随之错位。
///
/// 根因不是「拼接顺序」，而是**用一个纯文本字段承载了分块信息**。所以：
/// `MsgRow.text` 只放正文（宽度估算 / 纯文本回退用），分块真相源是 `MsgRow.segs`。
pub(crate) fn segments_body_text(msg: &serde_json::Value) -> String {
    let Some(arr) = msg.get("segments").and_then(|v| v.as_array()) else {
        return msg
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
    };
    let mut out = String::new();
    for seg in arr {
        if seg.get("type").and_then(|v| v.as_str()) != Some("text") {
            continue;
        }
        let Some(t) = seg.get("text").and_then(|v| v.as_str()) else {
            continue;
        };
        if t.is_empty() {
            continue;
        }
        // 相邻 text 段之间补一个换行（后端本会把连续正文并成一段，这里只是兜底）
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(t);
    }
    // 一个 text 段都没有（只有思考、只有工具调用，或老数据只写了顶层 thinking）：
    // 回退到 `content`。**这个兜底不能省**——修复前是「拼全部段」，那种消息至少
    // 还有思考文本可显示；只拼 text 段会让它的 `MsgRow.text` 变成空串，而
    // `render_msg_body` 在 `segs` 为空时正是靠 `text` 兜底渲染的（实测表现：
    // 消息整条空白，smoke 里「最后一条消息有正文」直接红）。
    if out.trim().is_empty() {
        return msg
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
    }
    out
}

/// 把消息 JSON 的 `segments` 解析成展示块。
///
/// ADR-002：正文的单一事实源是 `segments`（不是 `content`——后者只用于请求方向）。
/// 与 [`segments_body_text`] 的区别：那个只取正文段拼成一段纯文本（`MsgRow.text`
/// 的取值口径，给宽度估算与纯文本回退用），
/// 这个保留块类型，交给渲染层分别画成 Markdown / 灰字思考 / 工具卡片。
/// 解析消息的时间线段与工具调用（含结果）。
///
/// 返回 `(段, 工具调用)`：段只负责**顺序**，工具结果挂在消息级 `tool_calls`
/// （后端 `ToolCallWithResult`），渲染时按 `id` 关联 —— 与 Web 版
/// `SegmentBlocks` 的合并口径一致。
pub(crate) fn parse_segments(msg: &serde_json::Value) -> (Vec<Seg>, Vec<ToolCall>) {
    let mut segs: Vec<Seg> = Vec::new();
    if let Some(arr) = msg.get("segments").and_then(|v| v.as_array()) {
        for seg in arr {
            match seg.get("type").and_then(|v| v.as_str()).unwrap_or_default() {
                "text" => {
                    if let Some(t) = seg.get("text").and_then(|v| v.as_str()) {
                        if !t.trim().is_empty() {
                            segs.push(Seg::Text(t.to_string()));
                        }
                    }
                }
                "thinking" | "reasoning" => {
                    if let Some(t) = seg.get("text").and_then(|v| v.as_str()) {
                        if !t.trim().is_empty() {
                            segs.push(Seg::Thinking(t.to_string()));
                        }
                    }
                }
                "tool" => {
                    // 真实结构：{ type: "tool", tool_call: { id, name, arguments, presentation } }
                    let tc = seg.get("tool_call");
                    segs.push(Seg::Tool {
                        id: tc
                            .and_then(|v| v.get("id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        name: tc
                            .and_then(|v| v.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("工具")
                            .to_string(),
                        arguments: tc
                            .and_then(|v| v.get("arguments"))
                            .map(json_as_string)
                            .unwrap_or_default(),
                        presentation: tc
                            .and_then(|v| v.get("presentation"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("generic")
                            .to_string(),
                    });
                }
                _ => {}
            }
        }
    }
    let mut tools: Vec<ToolCall> = Vec::new();
    if let Some(arr) = msg.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in arr {
            let Some(id) = tc.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            tools.push(ToolCall {
                id: id.to_string(),
                name: tc
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("工具")
                    .to_string(),
                arguments: tc.get("arguments").map(json_as_string).unwrap_or_default(),
                presentation: tc
                    .get("presentation")
                    .and_then(|v| v.as_str())
                    .unwrap_or("generic")
                    .to_string(),
                result: tc
                    .get("result")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                duration_ms: tc.get("duration_ms").and_then(|v| v.as_i64()),
                error: tc
                    .get("error")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
            });
        }
    }

    // 顶层 `thinking` 回退（vite `MessageBubble` 同口径：用 `message.thinking`）。
    //
    // 后端消息既有顶层 `thinking` 字段、也有 `segments[type=thinking]`；但**早期
    // 消息只写了顶层 `thinking`**（`segments` 为空/空串）。只认 `segments` 的话，
    // 这些历史会话的思考块会凭空消失，而 Web 版正常显示——同一个会话在两个客户端
    // 里看起来不一样。放最前：思考总在正文之前。
    let has_thinking_seg = segs.iter().any(|s| matches!(s, Seg::Thinking(_)));
    if !has_thinking_seg {
        if let Some(t) = msg.get("thinking").and_then(|v| v.as_str()) {
            if !t.trim().is_empty() {
                segs.insert(0, Seg::Thinking(t.to_string()));
            }
        }
    }

    (segs, tools)
}

/// 百分号编码（查询参数值用；路径里常有空格、中文、反斜杠）。
pub(crate) fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 剥掉 hashline 行前缀（`12#AB|内容` → `内容`）。
///
/// 那是给模型做行定位用的标识（读文件工具返回的原样格式），界面展示不需要；
/// 非该格式的行原样保留，避免误伤正文里本来就有的 `|`。
pub(crate) fn strip_hashline(content: &str) -> String {
    content
        .lines()
        .map(|line| match line.split_once('|') {
            Some((head, rest)) => match head.split_once('#') {
                Some((num, _id)) if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) => {
                    rest.to_string()
                }
                _ => line.to_string(),
            },
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 文件大小 → 人类可读（`1.2 KB` / `23.4 MB` / `25.3 GB`），列表里做次要信息。
pub(crate) fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB * KB {
        format!("{:.1} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// 角色来源 → `(展示名, 颜色)`（对齐 Web 版 `SOURCE_LABEL` / `SOURCE_STYLE`：
/// 内置蓝 / 用户配置绿 / 学习演化紫）。
pub(crate) fn source_of(source: &str) -> (&'static str, u32) {
    match source {
        "builtin" => ("内置", colors::ACCENT()),
        "user" => ("用户配置", colors::SUCCESS()),
        "learned" => ("学习演化", 0xa855f7),
        _ => ("未知", colors::TEXT_TERTIARY()),
    }
}

/// JSON 值 → 字符串：字符串原样取，对象/数组序列化（工具参数可能两种形态）。
pub(crate) fn json_as_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 工具展示意图 → `(图标, 标签)`。
///
/// 与 Web 版 `tool-presentation.ts` / `TOOL_PRESENTATION_LABELS` 逐项对齐：
/// 后端在工具注册时声明 `presentation`，前端只做投影 —— 同一个工具在主对话流
/// 与子代理活动流里必须叫同一个名字（两处各维护一套是口径漂移的经典来源）。
pub(crate) fn presentation_of(presentation: &str) -> (IconName, &'static str) {
    match presentation {
        "read" => (IconName::FileText, "读取文件"),
        "write" => (IconName::FilePlus, "写入文件"),
        "terminal" => (IconName::Terminal, "执行命令"),
        "diff" => (IconName::PenLine, "编辑文件"),
        "search" => (IconName::Search, "搜索"),
        "web" => (IconName::Globe, "浏览网页"),
        "skill" => (IconName::Brain, "调用技能"),
        "knowledge" => (IconName::FilePlus, "导入知识"),
        "delegate" => (IconName::GitBranch, "委托子代理"),
        "code" => (IconName::Code, "代码分析"),
        "image" => (IconName::Image, "查看图片"),
        _ => (IconName::Wrench, "工具"),
    }
}

/// 毫秒 → 人类可读耗时（≥1s 用秒、两位小数），对齐 Web 版 `formatDuration`。
pub(crate) fn format_duration(ms: i64) -> String {
    if ms >= 1000 {
        format!("{:.2}s", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}
/// 工具参数 JSON 美化（缩进 2）。
///
/// 非法 JSON **原样返回**——宁可显示原始参数，也不要把内容吞掉（对齐 Web 版
/// `formatArguments`）。`apply_patch` 这类工具的参数是包在 JSON 里的 diff 文本，
/// 美化后换行才可读。
pub(crate) fn format_arguments(raw: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| raw.to_string()),
        Err(_) => raw.to_string(),
    }
}

/// 从工具参数里提取「一句话目标」（窄列单行展示用）。
///
/// 只挑常见字段、**不做语义推断**：猜错比截断更误导，取不到就把原始参数压成
/// 一行截断（与 Web 版 `summarizeToolTarget` 同口径）。`apply_patch` 的参数是
/// diff 信封而不是 JSON，单独走 `*** Update File: <路径>` 提取。
pub(crate) fn summarize_tool_target(raw: &str) -> String {
    const FIELDS: [&str; 13] = [
        "path",
        "file_path",
        "filePath",
        "pattern",
        "query",
        "command",
        "cmd",
        "url",
        "title",
        "task",
        "goal",
        "description",
        "name",
    ];
    fn clip(text: &str) -> String {
        let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if one.chars().count() > 80 {
            format!("{}…", one.chars().take(80).collect::<String>())
        } else {
            one
        }
    }
    for marker in ["*** Update File:", "*** Add File:", "*** Delete File:"] {
        if let Some(pos) = raw.find(marker) {
            let line = raw[pos + marker.len()..].lines().next().unwrap_or_default();
            if !line.trim().is_empty() {
                return clip(line.trim());
            }
        }
    }
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw) {
        for field in FIELDS {
            if let Some(serde_json::Value::String(v)) = map.get(field) {
                if !v.trim().is_empty() {
                    return clip(v.trim());
                }
            }
        }
        for v in map.values() {
            if let serde_json::Value::String(s) = v {
                if !s.trim().is_empty() {
                    return clip(s.trim());
                }
            }
        }
        return String::new();
    }
    clip(raw.trim())
}

/// 把折叠键写回 view 并重绘（`gpui_list` 的渲染闭包只有 `&mut App`）。
pub(crate) fn toggle_expand(
    app: &AppRef,
    key: String,
    which: fn(&mut ExpandState) -> &mut std::collections::HashSet<String>,
    cx: &mut App,
) {
    let weak = app.lock().ok().and_then(|g| g.clone());
    if let Some(w) = weak {
        let _ = w.update(cx, |this: &mut ClientApp, cx| {
            let mut state = this.expand.borrow_mut();
            let set = which(&mut state);
            if !set.remove(&key) {
                set.insert(key.clone());
            }
            cx.notify();
        });
    }
}

/// 可折叠的思考块（对齐 Web 版 `ThinkingBlock`）。
///
/// 收起态在 header 上单行显示思考输出的**最后一行**（流式期间实时更新）——
/// 用户既能一眼看到模型在想什么，又不会被大段思考撑满屏幕。
pub(crate) fn render_thinking(
    mi: usize,
    si: usize,
    text: String,
    expand: &ExpandRef,
    app: &AppRef,
) -> AnyElement {
    let key = format!("t:{mi}:{si}");
    let open = expand.borrow().thinking.contains(&key);
    let banner = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_string();
    let chevron = if open {
        IconName::ChevronDown
    } else {
        IconName::ChevronRight
    };
    let mut head = div()
        .id(("think-head", mi * 1000 + si))
        .px_2()
        .py_1()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .child(
            svg()
                .path(chevron.path())
                .size(px(12.))
                .text_color(rgb(colors::TEXT_TERTIARY())),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(13.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child("思考过程"),
        );
    if !open && !banner.is_empty() {
        head = head.child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .text_ellipsis()
                .overflow_hidden()
                .italic()
                .text_size(px(13.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child(banner),
        );
    }
    let app2 = app.clone();
    let key2 = key.clone();
    let head = head.on_click(move |_, _, cx| {
        toggle_expand(&app2, key2.clone(), |s| &mut s.thinking, cx);
    });
    let mut card = div()
        .rounded_lg()
        .border_1()
        .border_color(rgb(colors::BORDER()))
        .bg(rgb(colors::BG_TERTIARY()))
        .overflow_hidden()
        .child(head);
    if open {
        card = card.child(
            div()
                .px_3()
                .pb_2()
                .italic()
                .text_size(px(13.))
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child(text),
        );
    }
    card.into_any_element()
}

/// 工具调用卡片（对齐 Web 版 `ToolCallCard`）：头部徽标 + 目标摘要 + 状态，
/// 展开看完整参数；结果单独一层折叠（`tool.result` 是完整内容，不截断）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_tool_card(
    mi: usize,
    si: usize,
    name: String,
    arguments: String,
    presentation: String,
    tool: Option<&ToolCall>,
    expand: &ExpandRef,
    app: &AppRef,
) -> AnyElement {
    let args_key = format!("a:{mi}:{si}");
    let result_key = format!("r:{mi}:{si}");
    let (args_open, result_open) = {
        let state = expand.borrow();
        (
            state.tool_args.contains(&args_key),
            state.tool_result.contains(&result_key),
        )
    };
    let (icon, label) = presentation_of(&presentation);
    let error = tool.and_then(|t| t.error.clone());
    let failed = error.is_some();
    let has_result = tool.and_then(|t| t.result.as_ref()).is_some();
    let duration = tool.and_then(|t| t.duration_ms);
    let target = summarize_tool_target(&arguments);

    let mut head = div()
        .id(("tool-head", mi * 1000 + si))
        .px_2()
        .py_1()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .child(
            svg()
                .path(icon.path())
                .size(px(14.))
                .text_color(rgb(colors::TEXT_TERTIARY())),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(13.))
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child(label),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(13.))
                .text_color(rgb(colors::TEXT_PRIMARY()))
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .text_ellipsis()
                .overflow_hidden()
                .text_size(px(12.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child(target),
        );
    // 状态角标：失败红 ✗ / 有结果绿 ✓ / 还没结果时省略号
    if failed {
        head = head.child(
            svg()
                .path(IconName::CircleX.path())
                .size(px(13.))
                .text_color(rgb(colors::ERROR())),
        );
    } else if has_result {
        head = head.child(
            svg()
                .path(IconName::CircleCheck.path())
                .size(px(13.))
                .text_color(rgb(colors::SUCCESS())),
        );
    } else {
        head = head.child(
            div()
                .text_size(px(13.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child("…"),
        );
    }
    if let Some(ms) = duration {
        head = head.child(
            div()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child(format_duration(ms)),
        );
    }
    let app_args = app.clone();
    let key_args = args_key.clone();
    let head = head.on_click(move |_, _, cx| {
        toggle_expand(&app_args, key_args.clone(), |s| &mut s.tool_args, cx);
    });

    let mut card = div()
        .rounded_lg()
        .border_1()
        .border_color(rgb(if failed {
            colors::ERROR()
        } else {
            colors::BORDER()
        }))
        .bg(rgb(colors::BG_SECONDARY()))
        .overflow_hidden()
        .child(head);
    if let Some(err) = error {
        card = card.child(
            div()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(rgb(colors::ERROR()))
                .text_size(px(12.))
                .text_color(rgb(colors::ERROR()))
                .child(err),
        );
    }
    if args_open {
        card = card.child(
            div()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(rgb(colors::BORDER()))
                .max_h(px(180.))
                .overflow_hidden()
                .text_size(px(12.))
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child(format_arguments(&arguments)),
        );
    }
    if let Some(result) = tool.and_then(|t| t.result.clone()) {
        let chevron = if result_open {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };
        let app_res = app.clone();
        let key_res = result_key.clone();
        let result_head = div()
            .id(("tool-result-head", mi * 1000 + si))
            .px_2()
            .py_1()
            .border_t_1()
            .border_color(rgb(colors::BORDER()))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .child(
                svg()
                    .path(chevron.path())
                    .size(px(12.))
                    .text_color(rgb(colors::TEXT_TERTIARY())),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("工具结果"),
            )
            .on_click(move |_, _, cx| {
                toggle_expand(&app_res, key_res.clone(), |s| &mut s.tool_result, cx);
            });
        card = card.child(result_head);
        if result_open {
            card = card.child(
                div()
                    .px_3()
                    .pb_2()
                    .max_h(px(320.))
                    .overflow_hidden()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child(result),
            );
        }
    }
    card.into_any_element()
}

/// 用户气泡末尾的操作行：复制 / 回退（仅已落库消息）+ 右对齐时间。
///
/// 单独成函数是为了**控制 `render_msg_body` 的栈帧大小**——GPUI 的
/// prepaint/paint 是递归的，每层帧都计入栈；这段内联回去会让单帧翻倍
/// （实测：内联版在启动首帧就 `has overflowed its stack`）。
pub(crate) fn render_user_footer(
    msg: &MsgRow,
    mi: usize,
    app: &AppRef,
    session_id: &str,
) -> AnyElement {
    let mut footer = div().w_full().flex().flex_row().items_center().gap_2();
    let copy_text = msg.text.to_string();
    footer = footer.child(
        div()
            .id(("msg-copy", mi))
            .px_1()
            .rounded_md()
            .cursor_pointer()
            .opacity(0.65)
            .child(
                svg()
                    .path(IconName::Copy.path())
                    .size(px(11.))
                    .text_color(rgb(colors::BUBBLE_TEXT())),
            )
            .on_click(move |_, _, cx: &mut App| {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(copy_text.clone()));
            }),
    );
    // 回退只对**已落库**的用户消息开放（本地占位没有服务端 id）
    if !msg.id.is_empty() {
        let app_for_rb = app.clone();
        let sid = session_id.to_string();
        let mid = msg.id.clone();
        let text = msg.text.to_string();
        footer = footer.child(
            div()
                .id(("msg-rollback", mi))
                .px_1()
                .rounded_md()
                .cursor_pointer()
                .opacity(0.65)
                .child(
                    svg()
                        .path(IconName::Undo2.path())
                        .size(px(11.))
                        .text_color(rgb(colors::BUBBLE_TEXT())),
                )
                .on_click(move |_, _, cx| {
                    if let Some(w) = app_for_rb.lock().ok().and_then(|g| g.clone()) {
                        let (sid2, mid2, text2) = (sid.clone(), mid.clone(), text.clone());
                        let _ = w.update(cx, |this: &mut ClientApp, cx| {
                            this.request_rollback(sid2, mid2, text2, cx);
                        });
                    }
                }),
        );
    }
    footer = footer.child(div().flex_1());
    if let Some(ts) = &msg.ts {
        footer = footer.child(div().text_size(px(11.)).opacity(0.7).child(ts.clone()));
    }
    footer.into_any_element()
}

/// assistant 消息末尾的操作行：复制 + 时间（低对比度小字）。
///
/// 同 [`render_user_footer`]：拆出来只为压 `render_msg_body` 的栈帧。
pub(crate) fn render_assistant_footer(msg: &MsgRow, mi: usize) -> AnyElement {
    let mut footer = div().w_full().flex().flex_row().items_center().gap_2();
    if !msg.text.trim().is_empty() {
        let copy_text = msg.text.to_string();
        footer = footer.child(
            div()
                .id(("msg-copy-a", mi))
                .px_1()
                .rounded_md()
                .cursor_pointer()
                .opacity(0.65)
                .child(
                    svg()
                        .path(IconName::Copy.path())
                        .size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .on_click(move |_, _, cx: &mut App| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(copy_text.clone()));
                }),
        );
    }
    footer = footer.child(div().flex_1());
    if let Some(ts) = &msg.ts {
        footer = footer.child(
            div()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child(ts.clone()),
        );
    }
    footer.into_any_element()
}

/// 单条消息的正文（口径对齐 Web 版 `SegmentBlocks`）。
///
/// **相邻同类段合并**（thinking / text 各自连成一块）：模型一次输出常被切成
/// 多个同类增量，逐段渲染会让同一条思考碎成十几个小卡片。
pub(crate) fn render_msg_body(
    msg: &MsgRow,
    mi: usize,
    expand: &ExpandRef,
    app: &AppRef,
    session_id: &str,
) -> AnyElement {
    let is_user = msg.role == "user" && !msg.compression;
    if is_user {
        // 用户消息：强调色底 + 反白字，**右对齐**（vite `justify-end` +
        // `bg-blue-500 text-white rounded-2xl` 同形态）。
        //
        // 两个坑，都实测过：
        // ① 外层必须 `w_full()`——否则这一行按内容收缩，`justify_end()` 无从推挤，
        //    气泡仍贴在左边；
        // ② 正文**不能**走 `TextView::markdown`——它内部用组件主题的
        //    `foreground`（羊皮纸 = 深棕），不继承父级 `text_color`，落在深棕
        //    气泡上等于看不见。用户消息本就是短句纯文本，直接渲染文字即可。
        return div()
            .w_full()
            .px_4()
            .py_1()
            .flex()
            .flex_row()
            .justify_end()
            .child({
                let mut bubble = div()
                    // 气泡内纵向排：图片 → 正文 → 时间（vite 同序，时间在气泡内末尾）
                    .flex()
                    .flex_col()
                    .gap_2()
                    .rounded_2xl()
                    // 右下角小圆角（vite `rounded-br-sm`）：气泡指向发送者
                    .rounded_br_sm()
                    .bg(rgb(colors::BUBBLE_BG()))
                    .text_color(rgb(colors::BUBBLE_TEXT()))
                    .px_4()
                    .py_2()
                    .whitespace_normal();
                // 图片（vite：图在文字上方，单张 240px 上限）：data URL 解码成
                // GPUI `Image` 直接渲染，**不落盘**（`Image::from_bytes` 吃字节）
                if !msg.images.is_empty() {
                    let mut imgs = div().flex().flex_row().flex_wrap().gap_2();
                    for (k, src) in msg.images.iter().enumerate() {
                        let Some((format, bytes)) = decode_data_url(src) else {
                            continue;
                        };
                        // 尺寸必须在 `from_bytes` **之前**读：它把 bytes move 走
                        // 显式等比尺寸（见 `fit_image_size` 的注释：只给
                        // `max_w`+`max_h` 会被 GPUI 钳成正方框，上下留白）。
                        // 宽度上限取气泡内宽（气泡 640 − `px_4` 两侧 32 = 608），
                        // 高度上限 240 对齐 vite 的 `max-h-[240px]`。
                        let sized = image_dimensions(&bytes)
                            .map(|(w, h)| fit_image_size(w, h, BUBBLE_IMG_MAX_W, BUBBLE_IMG_MAX_H));
                        let arc = std::sync::Arc::new(gpui_kit::Image::from_bytes(format, bytes));
                        // 点击放大（对齐 vite `ZoomableImage`）：把 Arc 交给
                        // ClientApp，根层渲染全屏遮罩 + 大图
                        let app_for_img = app.clone();
                        let arc_for_click = arc.clone();
                        imgs = imgs.child(
                            div()
                                .id(("msg-img", mi * 100 + k))
                                .cursor_pointer()
                                .child(match sized {
                                    Some((w, h)) => {
                                        gpui_kit::img(gpui_kit::ImageSource::Image(arc))
                                            .w(px(w))
                                            .h(px(h))
                                            .rounded_lg()
                                    }
                                    // 尺寸读不出来（未知编码）时退回旧写法：
                                    // 会有留白，但至少图还在
                                    None => gpui_kit::img(gpui_kit::ImageSource::Image(arc))
                                        .max_w(px(BUBBLE_IMG_MAX_W))
                                        .max_h(px(BUBBLE_IMG_MAX_H))
                                        .rounded_lg(),
                                })
                                .on_click(move |_, _, cx| {
                                    if let Some(w) = app_for_img.lock().ok().and_then(|g| g.clone())
                                    {
                                        let _ = w.update(cx, |this: &mut ClientApp, cx| {
                                            this.zoomed_image = Some(arc_for_click.clone());
                                            cx.notify();
                                        });
                                    }
                                }),
                        );
                    }
                    bubble = bubble.child(imgs);
                }
                // 正文：`trim_end()` 去掉 `segments_body_text` 拼接时补的尾部换行——
                // 否则气泡内文字与时间之间会凭空多一个空行（用户截图指出的那条）
                let body_text = msg.text.trim_end().to_string();
                if !body_text.is_empty() {
                    bubble = bubble.child(div().child(body_text));
                }
                // 气泡末尾的操作行（复制 / 回退 + 时间）单独成函数：GPUI 递归
                // prepaint 时每层帧都算进栈，把它内联进本函数会让单帧翻倍
                // （实测会 `has overflowed its stack`）。
                bubble = bubble.child(render_user_footer(msg, mi, app, session_id));
                // 宽度：短消息随内容（「你好」那种窄气泡），长消息给 640 上限。
                //
                // 实测记录（省得下次再试一圈）：本机 GPUI 下 `max_w` **压不住**
                // 宽度——`max_w` / `w_full + max_w` / 再加 `min_w_0` 三种写法都会
                // 撑满整列（900+px）；显式 `w(px(640.))` 在宽窗口下生效，窗口收窄
                // 时按父宽收缩（用户实测「缩短宽度也能正常显示」）。
                // 判据用**半角单位估算**（中文 1 字 ≈ 2 半角），60 单位 ≈ 30 汉字：
                // 阈值定 78 时 `请写一篇 800 字…`（77 单位）差 1 没触发，看着就像
                // 「改了没效果」——阈值不要卡在临界值上。
                let est_units: usize = msg
                    .text
                    .chars()
                    .map(|c| if (c as u32) > 0x2E80 { 2 } else { 1 })
                    .sum();
                if est_units > 60 {
                    bubble = bubble.w(px(640.)).min_w_0();
                }
                bubble
            })
            .into_any_element();
    }
    let mut body = div().flex().flex_col().gap_2();
    if msg.compression {
        body = body.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .rounded_full()
                        .border_1()
                        .border_color(rgb(colors::BORDER()))
                        .bg(rgb(colors::BG_HOVER()))
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child("压缩点"),
                )
                .child(div().flex_1().h(px(1.)).bg(rgb(colors::BORDER()))),
        );
    }
    if msg.compression {
        // 压缩点只做**分隔符**：摘要正文是给模型看的，界面上不铺开（与 Web 版
        // `MessageBubble.tsx` 的「压缩点」胶囊同口径）——几百字摘要会把窗口占满，
        // 反而更难看清「压缩点到底在哪」。
    } else if msg.segs.is_empty() {
        if !msg.text.is_empty() {
            body = body.child(TextView::markdown(("msg", mi), msg.text.clone()));
        }
    } else {
        let mut i = 0usize;
        while i < msg.segs.len() {
            match &msg.segs[i] {
                Seg::Thinking(_) => {
                    let start = i;
                    let mut merged = String::new();
                    while i < msg.segs.len() {
                        match &msg.segs[i] {
                            Seg::Thinking(t) => {
                                merged.push_str(t);
                                i += 1;
                            }
                            _ => break,
                        }
                    }
                    body = body.child(render_thinking(mi, start, merged, expand, app));
                }
                Seg::Text(_) => {
                    let start = i;
                    let mut merged = String::new();
                    while i < msg.segs.len() {
                        match &msg.segs[i] {
                            Seg::Text(t) => {
                                merged.push_str(t);
                                i += 1;
                            }
                            _ => break,
                        }
                    }
                    body = body.child(TextView::markdown(("msg", mi * 1000 + start), merged));
                }
                Seg::Tool {
                    id,
                    name,
                    arguments,
                    presentation,
                } => {
                    // 按 id 关联结果；无 id 时退回「同名且无 id」的调用（历史数据）
                    let tool = msg.tool_calls.iter().find(|t| &t.id == id).or_else(|| {
                        msg.tool_calls
                            .iter()
                            .find(|t| t.id.is_empty() && &t.name == name)
                    });
                    // 段里的参数/展示意图可能缺失（老数据只存了 name），用 `tool_calls`
                    // 里那份兜底：展示意图决定卡片图标与标签，不能默认成「工具」
                    let args = if arguments.is_empty() {
                        tool.map(|t| t.arguments.clone()).unwrap_or_default()
                    } else {
                        arguments.clone()
                    };
                    let pres = match tool {
                        Some(t) if !t.presentation.is_empty() => t.presentation.clone(),
                        _ => presentation.clone(),
                    };
                    body = body.child(render_tool_card(
                        mi,
                        i,
                        name.clone(),
                        args,
                        pres,
                        tool,
                        expand,
                        app,
                    ));
                    i += 1;
                }
            }
        }
    }
    body = body.child(render_assistant_footer(msg, mi));
    div().px_4().py_2().w_full().child(body).into_any_element()
}
/// 消息区里可折叠块的展开状态（思考块 / 工具参数 / 工具结果）。
///
/// 放 `Rc<RefCell>` 而不是放在 `ClientApp` 上：`gpui_list` 的渲染闭包签名是
/// `FnMut(usize, &mut Window, &mut App)`，**拿不到** `&ClientApp`；而点击回调
/// 又必须能把状态写回去并触发重绘 —— 共享句柄 + `AppRef` 是这套框架下唯一
/// 不引入第二份真相的写法。
#[derive(Default)]
pub(crate) struct ExpandState {
    pub(crate) thinking: std::collections::HashSet<String>,
    pub(crate) tool_args: std::collections::HashSet<String>,
    pub(crate) tool_result: std::collections::HashSet<String>,
    /// 子代理活动流里展开的那一行（Web 版同口径：同时只展开一个）
    pub(crate) activity: Option<String>,
}

pub(crate) type ExpandRef = Rc<std::cell::RefCell<ExpandState>>;

/// 反向弱引用（点击回调里 `update` 回 view）。
pub(crate) type AppRef = Arc<Mutex<Option<WeakEntity<ClientApp>>>>;

#[cfg(test)]
mod image_mime_tests {
    //! 「图片进后端前必须转成白名单格式」的回归（用户实测 400：`data:image/bmp`）。
    //!
    //! 判据是纯函数（字节进、字节出），不需要 UI、不需要剪贴板、不需要 LLM ——
    //! 用单测钉住比每次手动截图粘贴可靠得多。
    // ⚠️ 不要 `use super::*`：bin crate 根是 `main.rs`，glob 会拉进整棵模块树，
    // `#[test]` 宏展开直接撞 `recursion_limit`（与 `step_boundary_tests` 同一个坑）。
    use crate::msg::normalize_image_for_api;

    /// 2×2 / 24bpp 的最小 BMP（Windows 截图走的就是这条路：剪贴板给 `CF_DIB`）。
    fn tiny_bmp() -> Vec<u8> {
        let (w, h) = (2i32, 2i32);
        let row = (w as usize * 3).div_ceil(4) * 4;
        let pixel_bytes = (row * h as usize) as u32;
        let file_size = 54 + pixel_bytes;
        let mut b = Vec::with_capacity(file_size as usize);
        b.extend_from_slice(b"BM");
        b.extend_from_slice(&file_size.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&54u32.to_le_bytes());
        b.extend_from_slice(&40u32.to_le_bytes());
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&h.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&24u16.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&pixel_bytes.to_le_bytes());
        b.extend_from_slice(&2835u32.to_le_bytes());
        b.extend_from_slice(&2835u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.resize(file_size as usize, 0x40);
        b
    }

    #[test]
    fn bmp_is_converted_to_png() {
        let (mime, out) = normalize_image_for_api("image/bmp", &tiny_bmp())
            .expect("BMP 必须能解码（剪贴板截图就是 BMP）");
        assert_eq!(mime, "image/png", "非白名单格式必须转成 PNG");
        assert_eq!(
            &out[..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
            "输出必须是真正的 PNG（magic 校验）"
        );
    }

    #[test]
    fn whitelisted_mime_passes_through_untouched() {
        let bytes = vec![1u8, 2, 3, 4];
        for mime in ["image/png", "image/jpeg", "image/webp", "image/gif"] {
            let (out_mime, out) = normalize_image_for_api(mime, &bytes).unwrap();
            assert_eq!(out_mime, mime);
            assert_eq!(out, bytes, "白名单格式不得改写字节");
        }
    }

    #[test]
    fn garbage_is_rejected_instead_of_leaking_through() {
        assert!(
            normalize_image_for_api("image/bmp", b"not an image at all").is_none(),
            "解码失败必须返回 None，不能把原始字节当图片发出去"
        );
    }
}

#[cfg(test)]
mod image_layout_tests {
    //! 消息图片尺寸的回归（用户报的「图片上下大量空白」）。
    //!
    //! 为什么用单测：真因是 **GPUI 的布局算术**（`img()` 把 size 设成原始像素，
    //! `max_w`/`max_h` 各自独立钳制 → 正方框 + Contain 居中），这个结论无法靠
    //! 界面断言稳定表达；判据本身是纯函数，单测能钉住「等比、不放大、不超上限」。
    //! ⚠️ 不写 `use super::*`（bin crate 根是 main.rs，glob 会撞 recursion_limit）。
    use crate::msg::{fit_image_size, image_dimensions};

    #[test]
    fn wide_screenshot_scales_by_height() {
        // 1024×640 的截图：高 240 是约束边 → 384×240（正是 vite 的观感）
        assert_eq!(fit_image_size(1024, 640, 608.0, 240.0), (384.0, 240.0));
    }

    #[test]
    fn tall_image_scales_by_height_too() {
        // 640×1024 竖图：仍按 240 高 → 150×240
        assert_eq!(fit_image_size(640, 1024, 608.0, 240.0), (150.0, 240.0));
    }

    #[test]
    fn small_image_is_not_upscaled() {
        // 比上限小的图保持原尺寸（vite `max-w/max-h` 同语义，小图不该被拉糊）
        assert_eq!(fit_image_size(100, 80, 608.0, 240.0), (100.0, 80.0));
    }

    #[test]
    fn ultra_wide_scales_by_width() {
        // 3000×500：宽度先到上限 → 608×101（不会因高度约束被拉高）
        let (w, h) = fit_image_size(3000, 500, 608.0, 240.0);
        assert_eq!(w, 608.0);
        assert!((h - 101.0).abs() <= 1.0, "高度应约为 101，实际 {h}");
    }

    #[test]
    fn degenerate_input_never_yields_zero() {
        let (w, h) = fit_image_size(0, 0, 608.0, 240.0);
        assert!(w >= 1.0 && h >= 1.0, "零尺寸必须兜成 1px，否则布局会 panic");
    }

    #[test]
    fn png_dimensions_are_read_from_header() {
        // 只读 header：2×3 的红点图
        let img = image::RgbaImage::from_pixel(2, 3, image::Rgba([255, 0, 0, 255]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("编码 PNG");
        assert_eq!(image_dimensions(&buf), Some((2, 3)));
        assert_eq!(image_dimensions(b"not an image"), None);
    }
}

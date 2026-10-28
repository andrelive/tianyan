//! 天演原生桌面客户端（GPUI）。
//!
//! 与 `gui-vite`（Tauri + React）**并存**的第二个客户端，连同一个内嵌 axum server，
//! 走同一套 HTTP/SSE 协议。gui-vite 退役前 `tauri/` 必须保留。
//!
//! 当前进度：**11 个一级入口全部实现**（会话 / 文件 / 技能 / 子智能体 / 工具 /
//! 知识 / 记忆 / 审批 / 定时任务 / 洞察 / 设置），另有内建 GUI 自动化端口
//! （ADR-052）：`TIANYAN_GUI_AUTOMATION_PORT` 设置时监听本机回环，供 agent 读
//! 界面状态、触发语义动作，取代「DPI 抓屏 + 坐标点击」的进程外验证。
//!
//! 未完成：托盘 / 通知 / 全局快捷键 / updater 编排 / MSI 代码签名。

#![recursion_limit = "512"]

use futures::StreamExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::text::TextView;
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::component::Root;
use gpui_kit::*;
use gpui_kit::{list as gpui_list, ListAlignment, ListState as GpuiListState};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::runtime::Runtime;

mod automation;

const SERVER_HOST: &str = "127.0.0.1";
const PREFERRED_PORT: u16 = 3000;
/// 单屏渲染的消息条数上限（无虚拟化时控制 Markdown 布局量，否则滚动卡顿）
const RENDER_LIMIT: usize = 60;
/// 左栏组内初始可见会话数（对齐 Tauri 版 `SESSIONS_INITIAL_VISIBLE`）
const SESSIONS_INITIAL_VISIBLE: usize = 3;
/// 「查看更多」单次要展开的条数（`SESSIONS_LOAD_STEP`）
const SESSIONS_LOAD_STEP: usize = 10;
/// 「最近活跃」段的时间窗口（24h）与组数上限
const ACTIVE_GROUP_WINDOW_SECS: i64 = 24 * 3_600;
const ACTIVE_GROUP_LIMIT: usize = 3;

/// 本地生成会话 id：服务端 `resolve_or_create_session` 接受前端传入的 id
/// 并在不存在时按它新建（`server/src/api/chat/services.rs`），因此新会话
/// 不必等 `POST /chat/stream` 的响应——事件按 `session_id` 路由，id 必须
/// 在**发起前**就确定，否则首批流事件会被当成别的会话丢掉。
fn new_session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("session-{nanos:x}")
}
/// 千分位分组（`1234567` → `1,234,567`；对齐 Web 版 `formatNumber`）。
fn group_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// 毫秒 → 人类可读（`820` → `820 ms`，`1530` → `1.5 s`）。
fn format_ms(ms: i64) -> String {
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
fn tokens_per_sec(ttft_ms: Option<i64>, duration_ms: i64, completion: u64) -> Option<u64> {
    let gen_ms = duration_ms - ttft_ms.unwrap_or(0);
    if completion == 0 || gen_ms <= 0 {
        return None;
    }
    Some((completion as f64 / (gen_ms as f64 / 1000.0)).round() as u64)
}

/// 当前 Unix 时间（秒）。
fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `HH:MM` → `(小时, 分钟)`（时区换算用；解析不出来按 0 处理）。
fn split_hhmm(s: &str) -> (i64, i64) {
    let hh = s.get(0..2).and_then(|v| v.parse().ok()).unwrap_or(0);
    let mm = s.get(3..5).and_then(|v| v.parse().ok()).unwrap_or(0);
    (hh, mm)
}

/// 解析 RFC3339 时间戳为 Unix 秒。
///
/// 手写而不引 `chrono`：左栏只需要「多久以前」这一档精度，为它加一个依赖不值。
/// 支持 `...Z`（UTC）、`...+08:00` / `...-05:00`（换算到 UTC）与小数秒
/// （`2026-10-27T06:45:44.885328Z` —— 小数部分丢弃）。
fn parse_rfc3339_secs(s: &str) -> Option<i64> {
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
fn format_relative(ts: &str) -> String {
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
fn group_label(workdir: &str) -> String {
    if workdir.is_empty() {
        return "默认".to_string();
    }
    workdir
        .split(['\\', '/'])
        .filter(|s| !s.is_empty())
        .last()
        .map(|s| s.to_string())
        .unwrap_or_else(|| workdir.to_string())
}

fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("创建 tokio runtime 失败"))
}

/// 在 tokio runtime 上取一段 JSON（GPUI 的 executor 是 smol 系，跨 runtime await
/// `JoinHandle` 是安全的）。
async fn fetch_json(url: String) -> Result<serde_json::Value, String> {
    runtime()
        .spawn(async move {
            let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
            resp.json::<serde_json::Value>()
                .await
                .map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(format!("join: {e}")))
}

/// POST 一段 JSON 并取回 JSON（同样跑在 tokio runtime 上，跨 runtime await 安全）。
async fn post_json(url: String, body: serde_json::Value) -> Result<serde_json::Value, String> {
    runtime()
        .spawn(async move {
            let resp = reqwest::Client::new()
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!("HTTP {status}: {text}"));
            }
            serde_json::from_str::<serde_json::Value>(&text).map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(format!("join: {e}")))
}

/// DELETE 一个资源（只看状态码，不解析响应体）：删会话用。
async fn delete_json(url: String) -> Result<(), String> {
    runtime()
        .spawn(async move {
            let resp = reqwest::Client::new()
                .delete(&url)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(format!("HTTP {status}: {text}"));
            }
            Ok(())
        })
        .await
        .unwrap_or_else(|e| Err(format!("join: {e}")))
}

/// 在 GPUI executor 上等一段时间：借 tokio runtime 睡（与 `fetch_json` 同一手法，
/// 避免引入 GPUI 自己的 timer 类型）。
async fn sleep_secs(secs: u64) {
    let _ = runtime()
        .spawn(async move { tokio::time::sleep(Duration::from_secs(secs)).await })
        .await;
}

/// PUT 一段 JSON（会话级模型选择等）：与 [`post_json`] 同一手法。
async fn put_json(url: String, body: serde_json::Value) -> Result<serde_json::Value, String> {
    runtime()
        .spawn(async move {
            let resp = reqwest::Client::new()
                .put(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!("HTTP {status}: {text}"));
            }
            serde_json::from_str::<serde_json::Value>(&text).map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(format!("join: {e}")))
}

/// 在字节缓冲里找子串（SSE 分帧用；`str::find` 在增量缓冲上有 char 边界风险，
/// 且 UTF-8 多字节字符可能被 chunk 切断——必须**按字节**分帧后再解码）
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 后台读统一事件流（`GET /api/v1/events`，ADR-028 常驻水管）。
///
/// `POST /api/v1/chat/stream` 只是开关，流式输出全部经这条常驻 SSE 下发
/// （`server/src/api/chat/handlers.rs`），所以订阅必须**独立于发送**常驻。
/// 断线（server 未起 / 重启）按 1s 间隔重连；`tx` 关闭即退出（窗口销毁）。
fn spawn_event_reader(port: u16, tx: tokio::sync::mpsc::UnboundedSender<serde_json::Value>) {
    let _ = runtime().spawn(async move {
        let client = reqwest::Client::new();
        let url = format!("http://{SERVER_HOST}:{port}/api/v1/events");
        loop {
            if tx.is_closed() {
                return;
            }
            let resp = match client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => {
                    println!("[事件] 订阅失败（1s 后重试）：{e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            println!("[事件] 已订阅 {}（HTTP {}）", url, resp.status());
            let mut stream = resp.bytes_stream();
            let mut buf: Vec<u8> = Vec::new();
            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        println!("[事件] 读取中断：{e}");
                        break;
                    }
                };
                buf.extend_from_slice(&bytes);
                while let Some(pos) = find_subslice(&buf, b"\n\n") {
                    let frame: Vec<u8> = buf.drain(..pos + 2).collect();
                    let frame = String::from_utf8_lossy(&frame);
                    // SSE 多行 data 按规范用 \n 拼接（axum 的 Event::data 目前是单行）
                    let mut data = String::new();
                    for line in frame.lines() {
                        if let Some(rest) = line.strip_prefix("data:") {
                            if !data.is_empty() {
                                data.push('\n');
                            }
                            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                        }
                    }
                    if data.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<serde_json::Value>(&data) {
                        Ok(v) => {
                            if tx.send(v).is_err() {
                                return;
                            }
                        }
                        Err(e) => println!("[事件] 解析失败：{e}"),
                    }
                }
            }
            println!("[事件] 连接结束，1s 后重连");
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}

/// 天演主题色（与 `gui-vite/src/index.css` 的 `.dark` 变量逐值对齐）。
///
/// 为什么不直接用 gpui-component 的 `Theme`：两套色板不同，混用会出现「侧栏是天演的
/// 深蓝、消息区是库的灰黑」这种割裂感。用户要求与现役 Tauri 版视觉一致，所以这里以
/// CSS 变量为单一事实源；`Theme::change(Dark)` 仍然保留（组件库内部控件需要它）。
mod colors {
    /// 页面底色（`--color-bg-primary`）
    pub const BG_PRIMARY: u32 = 0x1a1a2e;
    /// 侧栏 / 次级面（`--color-bg-secondary`）
    pub const BG_SECONDARY: u32 = 0x16213e;
    /// 三级面（`--color-bg-tertiary`）
    pub const BG_TERTIARY: u32 = 0x0f3460;
    /// hover 底色（`--color-bg-hover`）
    pub const BG_HOVER: u32 = 0x1a1a3e;
    /// 主文字（`--color-text-primary`）
    pub const TEXT_PRIMARY: u32 = 0xe4e4e7;
    /// 次文字（`--color-text-secondary`）
    pub const TEXT_SECONDARY: u32 = 0xa1a1aa;
    /// 弱文字（`--color-text-tertiary`）
    pub const TEXT_TERTIARY: u32 = 0x71717a;
    /// 分隔线（`--color-border`）
    pub const BORDER: u32 = 0x27272a;
    /// 强调色（`--color-accent`）
    pub const ACCENT: u32 = 0x3b82f6;
    /// 成功（`--color-success`）
    pub const SUCCESS: u32 = 0x22c55e;
    /// 错误（`--color-error`）
    pub const ERROR: u32 = 0xef4444;
    /// 警告（`--color-warning`）
    pub const WARNING: u32 = 0xeab308;
    /// 一级导航栏宽度（`--sidebar-collapsed-width`：64px 图标 rail）
    pub const RAIL_WIDTH: f32 = 64.;
    /// 会话列表宽度（`--sidebar-width`：280px）
    pub const LIST_WIDTH: f32 = 280.;
    /// 会话内容列宽度（Tailwind `max-w-4xl` = 896px）：消息列、底部状态条与输入卡片
    /// 三者必须同宽同左边界——`gui-vite` 的 `ChatPanel` 用 `w-full max-w-4xl mx-auto px-4`，
    /// `ChatInput` 用 `mx-auto max-w-4xl`，靠同一个 max-width 对齐。
    pub const CHAT_MAX_W: f32 = 896.;
}

/// 一级导航项（与 `gui-vite/src/components/sidebar/Sidebar.tsx` 的 `NAV_ITEMS` 逐项对齐）。
///
/// 第一列是 rail 上显示的 **Lucide 图标**：`gpui_kit::assets::IconName` 就是完整 Lucide
/// 目录（1800+ 个，`gpui-kit-assets` 内嵌 SVG），与 Tauri 版 `lucide-react` 同一套图形，
/// 逐项对齐（MessageSquare / FileText / Wrench / Users / Hammer / BookOpen /
/// MemoryStick / ShieldCheck / CalendarClock / Gauge / Settings）。第三列是路由目标，
/// 未实现的入口渲染占位页。
const NAV_ITEMS: [(IconName, &str, View); 11] = [
    (IconName::MessageSquare, "会话", View::Chat),
    (IconName::FileText, "文件", View::Workspace),
    (IconName::Wrench, "技能", View::Skills),
    (IconName::Users, "子智能体", View::Roles),
    (IconName::Hammer, "工具", View::Tools),
    (IconName::BookOpen, "知识", View::Knowledge),
    (IconName::MemoryStick, "记忆", View::Memory),
    (IconName::ShieldCheck, "审批", View::Approval),
    (IconName::CalendarClock, "定时任务", View::Scheduled),
    (IconName::Gauge, "洞察", View::Insights),
    (IconName::Settings, "设置", View::Settings),
];

/// 当前视图（一级导航的 11 个入口；已实现的只有「会话」与「设置」）
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    /// 会话：列表 + 消息 + 输入
    Chat,
    /// 文件（workspace）
    Workspace,
    /// 技能
    Skills,
    /// 子智能体（角色）
    Roles,
    /// 工具
    Tools,
    /// 知识
    Knowledge,
    /// 记忆
    Memory,
    /// 审批
    Approval,
    /// 定时任务
    Scheduled,
    /// 洞察
    Insights,
    /// 设置
    Settings,
}

impl View {
    /// 全部一级入口（顺序即侧栏顺序）。
    const ALL: [View; 11] = [
        View::Chat,
        View::Workspace,
        View::Skills,
        View::Roles,
        View::Tools,
        View::Knowledge,
        View::Memory,
        View::Approval,
        View::Scheduled,
        View::Insights,
        View::Settings,
    ];

    /// 稳定标识（自动化 API 的**对外契约值**：`state.view` 与 `switch_tab` 的参数）。
    /// 改名等于改契约，验证脚本会跟着断。
    fn key(self) -> &'static str {
        match self {
            View::Chat => "chat",
            View::Workspace => "workspace",
            View::Skills => "skills",
            View::Roles => "roles",
            View::Tools => "tools",
            View::Knowledge => "knowledge",
            View::Memory => "memory",
            View::Approval => "approval",
            View::Scheduled => "scheduled",
            View::Insights => "insights",
            View::Settings => "settings",
        }
    }

    /// 由标识反查（未知标识返回 `None`，报错交给调用方）。
    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.key() == key)
    }
}

/// 设置页的模型行（只取展示与切换需要的字段）。
#[derive(Clone, Debug)]
struct ModelRow {
    provider: String,
    name: String,
    /// 能力标签（kebab-case，逗号拼接；`chat` / `vision` / `text-embedding` …）
    capabilities: String,
    context: Option<u64>,
    /// 是否为当前 chat 首选模型（`preferences.chat` 命中）
    is_chat: bool,
}

/// 单行省略（ListItem 硬约束：每项必须等高，多行会溢出重叠）
fn ellipsis(s: &str, max_chars: usize) -> String {
    let n = s.chars().count();
    if n <= max_chars {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max_chars).collect::<String>())
    }
}

/// RFC3339 → `MM-DD HH:MM`（面板列表只要到分钟；不引 chrono）。
fn short_stamp(s: &str) -> String {
    match (s.get(5..10), s.get(11..16)) {
        (Some(d), Some(t)) => format!("{d} {t}"),
        _ => s.to_string(),
    }
}

/// 秒 → 人类可读间隔（`1800` → `每 30 分钟`；对齐 Web 版 `formatInterval`）。
fn format_interval(secs: u64) -> String {
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
fn relative_from_now(ts: i64) -> String {
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

/// 定时任务的间隔预设（秒；`0` = 自定义）。对齐 Web 版 `INTERVAL_PRESETS`。
const SCHED_PRESETS: [(u64, &str); 7] = [
    (1800, "每 30 分钟"),
    (3600, "每 1 小时"),
    (21600, "每 6 小时"),
    (43200, "每 12 小时"),
    (86400, "每天"),
    (604800, "每周"),
    (0, "自定义…"),
];

/// 表单单行输入框的外壳（去自带外观 + 统一边框底色；与设置页编辑框同一手法）。
fn field_wrap(input: &Entity<InputState>) -> gpui_kit::AnyElement {
    div()
        .w_full()
        .rounded_md()
        .border_1()
        .border_color(rgb(colors::BORDER))
        .bg(rgb(colors::BG_PRIMARY))
        .px_2()
        .py_1()
        .child(Input::new(input).appearance(false))
        .into_any_element()
}

/// 洞察页的一个统计卡（标题 + 主值 + 副说明）。
fn stat_card(label: &'static str, value: String, sub: String) -> gpui_kit::AnyElement {
    div()
        .flex_1()
        .min_w(px(130.))
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(rgb(colors::BORDER))
        .bg(rgb(colors::BG_SECONDARY))
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(px(10.))
                .text_color(rgb(colors::TEXT_TERTIARY))
                .child(label),
        )
        .child(
            div()
                .text_size(px(18.))
                .text_color(rgb(colors::TEXT_PRIMARY))
                .child(value),
        )
        .child(
            div()
                .text_size(px(10.))
                .text_color(rgb(colors::TEXT_TERTIARY))
                .child(sub),
        )
        .into_any_element()
}

/// 秒 → 粗粒度描述（`2303` → `38 分钟`；「下次还有多久」用）。
fn format_secs_short(secs: u64) -> String {
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
fn risk_color(risk: &str) -> u32 {
    match risk.to_lowercase().as_str() {
        "low" => colors::SUCCESS,
        "medium" => colors::WARNING,
        _ => colors::ERROR,
    }
}

/// 表单多行输入框的外壳（同上）。
fn field_wrap_area(input: &Entity<TextareaState>) -> gpui_kit::AnyElement {
    div()
        .w_full()
        .rounded_md()
        .border_1()
        .border_color(rgb(colors::BORDER))
        .bg(rgb(colors::BG_PRIMARY))
        .px_2()
        .py_1()
        .child(Textarea::new(input).appearance(false))
        .into_any_element()
}

/// 把选中的路径展开成待上传文件列表（目录递归；上限 200 个——防手滑导入整盘）。
///
/// 两个入口共用：路径输入（单路径）与系统文件选择器（多选，可含目录）。
fn collect_ingest_files(paths: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
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
fn memory_block(label: &'static str, text: String) -> gpui_kit::AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(px(10.))
                .text_color(rgb(colors::TEXT_TERTIARY))
                .child(label),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(colors::TEXT_SECONDARY))
                .child(text),
        )
        .into_any_element()
}

// ===== 会话列表 =====

/// 会话（`GET /api/v1/sessions` 的子集，够左栏分组渲染）。
#[derive(Clone, Debug)]
struct SessionRow {
    id: String,
    title: String,
    /// 工作目录绝对路径；`""` = 未绑定（默认组）。后端字段 `working_directory` 可为 null。
    working_directory: String,
    /// RFC3339 时间戳（列表展示相对时间）
    updated_at: String,
}

/// 左栏的一行 —— **分组行 / 会话行 / 「查看更多」行混排**。
///
/// 与 `SessionRow`（原始会话数据）分开：一行的「是什么」由折叠状态、选中态、
/// 后台任务数共同决定，不能从会话数据直接推出来。
#[derive(Clone, Debug)]
enum SessionListRow {
    /// 工作区分组行（目录名 + 会话数 + 折叠箭头）
    Group {
        key: String,
        label: String,
        count: usize,
        collapsed: bool,
    },
    /// 会话行
    Session {
        id: String,
        title: String,
        /// 副标题：相对时间（右对齐信息，空串则不画）
        meta: String,
        /// 该会话正在跑的后台任务数（>0 显示红色角标）
        tasks: usize,
        selected: bool,
    },
    /// 组内折叠的「查看更多 (n)」行
    More { key: String, hidden: usize },
}
/// 目录选择器选中的目录用在哪里。
#[derive(Clone, Copy, PartialEq)]
enum PickerTarget {
    /// 绑定到当前会话（`PUT /sessions/{id}/workspace`）
    Session,
    /// 作为下一个新会话的待绑定目录（本地状态，首条消息时下发）
    NewSession,
}

impl PickerTarget {
    /// 稳定标识（自动化 API 契约值）。
    fn key(self) -> &'static str {
        match self {
            PickerTarget::Session => "session",
            PickerTarget::NewSession => "new_session",
        }
    }
}

#[derive(Clone)]
/// 需要用户确认的模态对话框（自绘覆盖层，不依赖组件库 Dialog）。
enum Dialog {
    /// 删除会话确认
    DeleteSession { id: String, title: String },
    /// 重命名会话（输入框实体在 `ClientApp::dialog_input`）
    RenameSession { id: String },
    /// 新建会话并绑定工作目录（「＋新目录」；输入框实体同上）
    /// 目录选择器（逐级浏览后选定）
    WorkspacePicker { target: PickerTarget },
    /// 回退内置种子（`POST /roles/{name}/reset`）
    ResetRole { name: String },
    /// 退役删除角色（`DELETE /roles/{name}`）
    DeleteRole { name: String },
    /// 知识导入的文件选择弹窗（**GPUI 自绘**，不是 OS 对话框 —— 所以自动化点得到）
    KbFilePicker,
}

impl Dialog {
    /// 稳定标识（自动化 API 契约值；`state.dialog.kind`）。
    fn key(&self) -> &'static str {
        match self {
            Dialog::DeleteSession { .. } => "delete_session",
            Dialog::RenameSession { .. } => "rename_session",
            Dialog::WorkspacePicker { .. } => "workspace_picker",
            Dialog::ResetRole { .. } => "reset_role",
            Dialog::DeleteRole { .. } => "delete_role",
            Dialog::KbFilePicker => "kb_file_picker",
        }
    }
}

// 左栏行由 `ClientApp::rebuild_session_rows` 直接产生（行类型混排 + 行高不一，
// 虚拟列表的等高测量在这里只会帮倒忙：实测「查看更多」行会与上一会话行重叠）。
// ===== 消息（不再用虚拟列表：高度不定，几十到 200 条直接布局更简单可靠）=====

/// 会话目标行。
///
/// 注意 `GET /goals` 的元素是**嵌套结构**：`{ goal: {title, status, ...},
/// progress, todo_total, todo_done }`（`server/src/api/goals/handlers.rs` 现场组装，
/// 不是 core 的 `Goal` 平铺）。早先按平铺解析导致标题空白、进度丢失。
#[derive(Clone, Debug)]
struct GoalRow {
    title: String,
    status: String,
    /// 0-100（`todo_total == 0` 时为 0，表示「还没有子待办」）
    progress: u64,
    todo_total: u64,
    todo_done: u64,
}

/// 子智能体（角色）列表项：`GET /roles` 的 `roles[]`（`RoleSummary` 子集）。
#[derive(Clone, Debug)]
struct RoleRow {
    name: String,
    /// 来源：builtin / user / learned
    source: String,
    /// 状态：active / experimental
    status: String,
    version: u32,
    /// 职责一句话（系统提示首行）
    purpose: String,
    /// 工具白名单数量（None = 不限制）
    tool_count: Option<u64>,
    /// 模型（None = 回落主 Agent）
    model: Option<String>,
    /// `(调用次数, 成功率)`；无记录时 None
    usage: Option<(u32, f32)>,
}

/// 子智能体统计（`GET /roles/stats`）：页面顶部那条汇总。
#[derive(Clone, Debug, Default)]
struct RolesStats {
    total_calls: u32,
    /// 0-1
    success_rate: f32,
    /// `(任务类型, 次数)`
    by_task_type: Vec<(String, usize)>,
    /// `(角色名, 调用次数)`
    by_role: Vec<(String, u32)>,
}

/// 工作区目录树的一条（`GET /workspace/tree` 的 `entries[]`）。
#[derive(Clone, Debug)]
struct WsEntry {
    name: String,
    /// true = 目录（点进去），false = 文件（点开读）
    is_dir: bool,
    /// 相对工作区根的路径（如 `src/main.rs`）
    path: String,
    /// 文件大小（目录为 0）
    size: u64,
}

/// 工作区改动文件（`GET /workspace/diff` 的列表项）。
#[derive(Clone, Debug)]
struct WsDiffFile {
    /// 相对工作区根的路径
    path: String,
    /// modified / added / removed / binary / unchanged
    status: String,
    /// 旧文件行数（新增文件为 0）
    old_lines: u64,
    /// 新文件行数（删除文件为 0）
    new_lines: u64,
}

/// 角色详情（`GET /roles/{name}`）：完整系统提示 + 工具白名单。
#[derive(Clone, Debug, Default)]
struct RoleDetail {
    system_prompt: String,
    tools: Vec<String>,
}

/// 工具目录的一行（`GET /api/v1/tools`）。
///
/// 工具 = 原子化操作原语（系统内置），与「技能」（方法论）是两套概念：
/// 工具页展示可调用的原子操作，技能页展示 GEPA 学到的方法论。
#[derive(Clone, Debug)]
struct ToolRow {
    name: String,
    description: String,
    /// 参数 JSON Schema（详情页原样 pretty 打印，不另做表单）
    parameters: serde_json::Value,
}

/// 学习方法论技能（`GET /api/v1/skills` 的子集）。
///
/// 只列 `category == "custom"`：其余类是内置能力（文件/命令/网络），由工具面板覆盖。
/// 技能是**方法论文档**（无执行语义），所以只读查看，不做「运行」按钮。
#[derive(Clone, Debug)]
struct SkillRow {
    id: String,
    name: String,
    description: String,
    version: String,
    updated_at: Option<String>,
}

/// 技能使用统计的一行（`GET /api/v1/skills/stats` 的 `skills[]`）。
///
/// 方法论技能没有「执行成败」语义，这里只反映**采纳频率**与 handler 层异常。
#[derive(Clone, Debug)]
struct SkillStatRow {
    skill_id: String,
    total_calls: u64,
    success_calls: u64,
    avg_time_ms: f64,
}

/// 定时智能体任务（`GET /scheduled-tasks`）。
///
/// 间隔制（ADR-024）：`interval_secs` 是「距上次执行 ≥ 此值即触发」，
/// 服务未运行期间超期的任务在重启后补跑一次。时间戳是 **Unix 秒**（不是 RFC3339）。
#[derive(Clone, Debug)]
struct SchedTaskRow {
    id: String,
    name: String,
    interval_secs: u64,
    workspace: String,
    prompt: String,
    enabled: bool,
    /// 上次执行时间（RFC3339；统一口径——历史上这里是 epoch 秒）
    last_run_at: Option<String>,
    /// 下次预计执行时间（RFC3339）
    next_run_at: Option<String>,
    last_result: Option<String>,
}

/// 记忆条目（`GET /memory` 的 `memories[]`）。
///
/// 后端已把三类来源聚合齐（`memory/` 事实与决策、`user/` 画像、`agent/patterns` 模式），
/// 并只返回叶子（目录只是导航中间节点）。三层内容是 VFS 的 L0/L1/L2：
/// 列表看 L0（abstract），详情默认展 L1（overview），L2（detail）按需展开。
#[derive(Clone, Debug)]
struct MemoryRow {
    uri: String,
    name: String,
    relative_path: String,
    importance: f32,
    updated_at: String,
    tags: Vec<String>,
    abstract_text: Option<String>,
    overview_text: Option<String>,
    detail_text: Option<String>,
}

/// 洞察页总览（`GET /stats`）。
///
/// 四个追踪面：技能（方法论采纳）、工具（原子操作调用）、文档（VFS 访问）、搜索。
#[derive(Clone, Debug, Default)]
struct InsightStats {
    total_skills_tracked: u64,
    total_skill_calls: u64,
    total_tools_tracked: u64,
    total_tool_calls: u64,
    total_docs_tracked: u64,
    total_searches: u64,
}

/// 模型用量总计（`GET /usage/stats` 的 `total`）。
#[derive(Clone, Debug, Default)]
struct UsageTotal {
    calls: u64,
    uncached_input: u64,
    cached_input: u64,
    completion_tokens: u64,
    total_tokens: u64,
    cache_hit_rate: f64,
}

/// 模型用量分组（`grouped[]`：按 model 或 provider）。
#[derive(Clone, Debug)]
struct UsageGroup {
    group: String,
    calls: u64,
    total_tokens: u64,
    cache_hit_rate: f64,
}

/// 调度器任务状态（`GET /scheduler/status` 的 `tasks[]`）。
#[derive(Clone, Debug)]
struct SchedStatusRow {
    id: String,
    name: String,
    interval_secs: u64,
    run_count: u64,
    last_run_ago_secs: u64,
    next_due_in_secs: u64,
}

/// 审批自动规则（`config.auto_approval_rules[]` 的展示子集）。
#[derive(Clone, Debug)]
struct ApprovalRuleRow {
    name: String,
    decision: String,
    enabled: bool,
}

/// 审批审计记录（`recent_records[]` 的展示子集）。
///
/// 「结果」是三元：`None` = 还没执行完 / `Some(true)` = 执行成功 / `Some(false)` = 失败。
#[derive(Clone, Debug)]
struct ApprovalHistRow {
    description: String,
    risk: String,
    decision: String,
    session_id: String,
    result: Option<bool>,
}

/// 知识条目（`GET /knowledge/entries`）。三层是否齐备决定详情能读哪些层。
#[derive(Clone, Debug)]
struct KbEntryRow {
    uri: String,
    name: String,
    is_dir: bool,
    has_abstract: bool,
    has_overview: bool,
    has_detail: bool,
}

/// 知识检索结果（`GET /knowledge/search` 的 `results[]`）。
#[derive(Clone, Debug)]
struct KbSearchRow {
    id: String,
    content: String,
    source: String,
    score: f32,
}

/// 内嵌文件浏览器的一行（`GET /workspace/dirs?include_files=true`）。
#[derive(Clone, Debug)]
struct KbBrowseEntry {
    name: String,
    path: String,
    is_dir: bool,
    size: Option<u64>,
}

/// 设置页的 provider 行。
///
/// **含 api_key**：编辑时要把它回填进输入框，而 `set_value` 需要 `&mut Window`
/// （只有在点击处理器这种同步上下文里才有），所以不能在异步回调里回填——
/// 只能随列表一起取回来。这条缓存**只用于展示与回填**；保存时仍然重新 GET 配置
/// 再改（见 `save_provider`），不拿这份缓存去写。
#[derive(Clone, Debug)]
struct ProviderRow {
    name: String,
    endpoint: String,
    api_key: String,
    model_count: usize,
}

/// 解析 `ask_user` 工具调用参数里的问题列表。
///
/// 形态**唯一**（对齐后端 `AskUserParams`）：`arguments` 里只有 `questions` 数组，
/// 每个问题 `{question, options?}`。`arguments` 在事件里可能是 JSON 字符串或对象，
/// 两种都接（Tauri 版 `parseAskUserQuestions` 同口径）。
fn parse_ask_user(args: &serde_json::Value) -> Vec<(String, Vec<String>)> {
    let parsed: serde_json::Value = match args {
        serde_json::Value::String(s) => serde_json::from_str(s).unwrap_or(serde_json::Value::Null),
        v => v.clone(),
    };
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    if let Some(qs) = parsed.get("questions").and_then(|v| v.as_array()) {
        for q in qs {
            let question = q
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let options: Vec<String> = q
                .get("options")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|o| o.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            if !question.is_empty() {
                out.push((question, options));
            }
        }
    }
    // 兼容「单个问题直接给 question/options」的朴素形态（没有 questions 数组）
    if out.is_empty() {
        if let Some(q) = parsed.get("question").and_then(|v| v.as_str()) {
            let options: Vec<String> = parsed
                .get("options")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|o| o.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            out.push((q.to_string(), options));
        }
    }
    out
}

/// 待审批操作（对齐 `GET /api/v1/approval/status` 的 `pending_approvals[]`）。
#[derive(Clone, Debug)]
struct ApprovalRow {
    request_id: String,
    description: String,
    risk: String,
}

/// 后台任务面板的一行（对齐 Tauri 版 `AgentTasksPanel` 的展示字段）。
#[derive(Clone, Debug)]
struct TaskRow {
    /// 任务 id（`bt_xxx` 委托 / `cmd_xxx` 终端；也是子代理的**子会话 id**）
    id: String,
    kind: String,
    status: String,
    description: String,
    /// 归属会话 id（左栏角标按它聚合；子代理/终端任务才有）
    parent_session_id: String,
    /// 终端输出尾部（后端 `OUTPUT_TAIL_MAX_BYTES` 截断；委托任务为空）
    output_tail: String,
    /// 失败原因（`failed` 时非空）
    error: String,
}

/// 消息内的一个展示块（口径对齐 Tauri 版 `MessageSegments` 的分段渲染）。
///
/// **工具执行结果不在段里**：后端 `MessageSegment` 只有 thinking / text / tool
/// 三种，结果挂在消息级 `tool_calls`（`ToolCallWithResult`），按 `id` 关联。
#[derive(Clone, Debug)]
enum Seg {
    /// 正文（Markdown 渲染）
    Text(String),
    /// 思考过程（折叠卡片；收起时显示最后一行）
    Thinking(String),
    /// 工具调用（卡片：展示意图 + 名称 + 参数；结果按 `id` 从 `tool_calls` 取）
    Tool {
        id: String,
        name: String,
        arguments: String,
        presentation: String,
    },
}

/// 工具调用 + 执行结果（与后端 `ToolCallWithResult` 同构）。
///
/// `result` 是**完整**内容（后端不截断），`duration_ms` / `error` 由
/// `Part::ToolResult` 推导 —— 三者共同决定卡片的状态角标与展开内容。
#[derive(Clone, Debug)]
struct ToolCall {
    id: String,
    name: String,
    arguments: String,
    presentation: String,
    result: Option<String>,
    duration_ms: Option<i64>,
    error: Option<String>,
}

#[derive(Clone, Debug)]
struct MsgRow {
    role: String,
    /// SharedString（Arc 语义）：clone 是 O(1)，避免每帧复制正文
    text: gpui_kit::SharedString,
    /// 结构化分段：历史消息来自 `segments`，流式增量只有正文（segs 为空，用 text 兜底）
    segs: Vec<Seg>,
    /// 本条消息的工具调用（含结果）；段里的 `Tool` 按 `id` 找它取结果与耗时
    tool_calls: Vec<ToolCall>,
    /// 压缩点（`compression_marker`）：显示徽章，**不**渲染成用户气泡
    compression: bool,
}

/// 从消息 JSON 的 segments 里拼接正文（ADR-002：正文单一事实源是 segments，
/// 不是 content——后者只在请求方向携带）
fn segments_text(msg: &serde_json::Value) -> String {
    let Some(arr) = msg.get("segments").and_then(|v| v.as_array()) else {
        return msg
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
    };
    let mut out = String::new();
    for seg in arr {
        let ty = seg.get("type").and_then(|v| v.as_str()).unwrap_or_default();
        match ty {
            "text" => {
                if let Some(t) = seg.get("text").and_then(|v| v.as_str()) {
                    out.push_str(t);
                    out.push('\n');
                }
            }
            "thinking" | "reasoning" => {
                if let Some(t) = seg.get("text").and_then(|v| v.as_str()) {
                    if !t.is_empty() {
                        out.push_str("> 🧠 思考：");
                        out.push_str(&t.replace('\n', "\n> "));
                        out.push('\n');
                    }
                }
            }
            "tool" => {
                // 真实结构：{ type: "tool", tool_call: { name, arguments, .. } }
                let tc = seg.get("tool_call");
                let name = tc
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                out.push_str(&format!("`🔧 {name}`\n"));
            }
            _ => {}
        }
    }
    if out.trim().is_empty() {
        out.push_str("(空消息)");
    }
    out
}

/// 把消息 JSON 的 `segments` 解析成展示块。
///
/// ADR-002：正文的单一事实源是 `segments`（不是 `content`——后者只用于请求方向）。
/// 与 [`segments_text`] 的区别：那个把思考/工具都拼成一段文字（早期纯文本渲染用），
/// 这个保留块类型，交给渲染层分别画成 Markdown / 灰字思考 / 工具卡片。
/// 解析消息的时间线段与工具调用（含结果）。
///
/// 返回 `(段, 工具调用)`：段只负责**顺序**，工具结果挂在消息级 `tool_calls`
/// （后端 `ToolCallWithResult`），渲染时按 `id` 关联 —— 与 Web 版
/// `SegmentBlocks` 的合并口径一致。
fn parse_segments(msg: &serde_json::Value) -> (Vec<Seg>, Vec<ToolCall>) {
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
    (segs, tools)
}

/// 百分号编码（查询参数值用；路径里常有空格、中文、反斜杠）。
fn url_encode(s: &str) -> String {
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
fn strip_hashline(content: &str) -> String {
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
fn format_size(bytes: u64) -> String {
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
fn source_of(source: &str) -> (&'static str, u32) {
    match source {
        "builtin" => ("内置", colors::ACCENT),
        "user" => ("用户配置", colors::SUCCESS),
        "learned" => ("学习演化", 0xa855f7),
        _ => ("未知", colors::TEXT_TERTIARY),
    }
}

/// JSON 值 → 字符串：字符串原样取，对象/数组序列化（工具参数可能两种形态）。
fn json_as_string(v: &serde_json::Value) -> String {
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
fn presentation_of(presentation: &str) -> (IconName, &'static str) {
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
fn format_duration(ms: i64) -> String {
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
fn format_arguments(raw: &str) -> String {
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
fn summarize_tool_target(raw: &str) -> String {
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
fn toggle_expand(
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
fn render_thinking(
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
                .text_color(rgb(colors::TEXT_TERTIARY)),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(13.))
                .text_color(rgb(colors::TEXT_TERTIARY))
                .child("思考过程"),
        );
    if !open && !banner.is_empty() {
        head = head.child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .overflow_hidden()
                .italic()
                .text_size(px(13.))
                .text_color(rgb(colors::TEXT_TERTIARY))
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
        .border_color(rgb(colors::BORDER))
        .bg(rgb(colors::BG_TERTIARY))
        .overflow_hidden()
        .child(head);
    if open {
        card = card.child(
            div()
                .px_3()
                .pb_2()
                .italic()
                .text_size(px(13.))
                .text_color(rgb(colors::TEXT_SECONDARY))
                .child(text),
        );
    }
    card.into_any_element()
}

/// 工具调用卡片（对齐 Web 版 `ToolCallCard`）：头部徽标 + 目标摘要 + 状态，
/// 展开看完整参数；结果单独一层折叠（`tool.result` 是完整内容，不截断）。
#[allow(clippy::too_many_arguments)]
fn render_tool_card(
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
                .text_color(rgb(colors::TEXT_TERTIARY)),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(13.))
                .text_color(rgb(colors::TEXT_SECONDARY))
                .child(label),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(13.))
                .text_color(rgb(colors::TEXT_PRIMARY))
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_size(px(12.))
                .text_color(rgb(colors::TEXT_TERTIARY))
                .child(target),
        );
    // 状态角标：失败红 ✗ / 有结果绿 ✓ / 还没结果时省略号
    if failed {
        head = head.child(
            svg()
                .path(IconName::CircleX.path())
                .size(px(13.))
                .text_color(rgb(colors::ERROR)),
        );
    } else if has_result {
        head = head.child(
            svg()
                .path(IconName::CircleCheck.path())
                .size(px(13.))
                .text_color(rgb(colors::SUCCESS)),
        );
    } else {
        head = head.child(
            div()
                .text_size(px(13.))
                .text_color(rgb(colors::TEXT_TERTIARY))
                .child("…"),
        );
    }
    if let Some(ms) = duration {
        head = head.child(
            div()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_TERTIARY))
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
            colors::ERROR
        } else {
            colors::BORDER
        }))
        .bg(rgb(colors::BG_SECONDARY))
        .overflow_hidden()
        .child(head);
    if let Some(err) = error {
        card = card.child(
            div()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(rgb(colors::ERROR))
                .text_size(px(12.))
                .text_color(rgb(colors::ERROR))
                .child(err),
        );
    }
    if args_open {
        card = card.child(
            div()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(rgb(colors::BORDER))
                .max_h(px(180.))
                .overflow_hidden()
                .text_size(px(12.))
                .text_color(rgb(colors::TEXT_SECONDARY))
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
            .border_color(rgb(colors::BORDER))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .child(
                svg()
                    .path(chevron.path())
                    .size(px(12.))
                    .text_color(rgb(colors::TEXT_TERTIARY)),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgb(colors::TEXT_TERTIARY))
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
                    .text_color(rgb(colors::TEXT_SECONDARY))
                    .child(result),
            );
        }
    }
    card.into_any_element()
}

/// 单条消息的正文（口径对齐 Web 版 `SegmentBlocks`）。
///
/// **相邻同类段合并**（thinking / text 各自连成一块）：模型一次输出常被切成
/// 多个同类增量，逐段渲染会让同一条思考碎成十几个小卡片。
fn render_msg_body(msg: &MsgRow, mi: usize, expand: &ExpandRef, app: &AppRef) -> AnyElement {
    let is_user = msg.role == "user" && !msg.compression;
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
                        .border_color(rgb(colors::BORDER))
                        .bg(rgb(colors::BG_HOVER))
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_SECONDARY))
                        .child("压缩点"),
                )
                .child(div().flex_1().h(px(1.)).bg(rgb(colors::BORDER))),
        );
    }
    if msg.segs.is_empty() {
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
    if is_user {
        div()
            .px_4()
            .py_1()
            .flex()
            .flex_row()
            .justify_end()
            .child(
                div()
                    .max_w(px(640.))
                    .rounded_2xl()
                    .bg(rgb(colors::ACCENT))
                    .text_color(rgb(0xffffff))
                    .px_4()
                    .py_2()
                    .child(body),
            )
            .into_any_element()
    } else {
        div().px_4().py_2().w_full().child(body).into_any_element()
    }
}
/// 消息区里可折叠块的展开状态（思考块 / 工具参数 / 工具结果）。
///
/// 放 `Rc<RefCell>` 而不是放在 `ClientApp` 上：`gpui_list` 的渲染闭包签名是
/// `FnMut(usize, &mut Window, &mut App)`，**拿不到** `&ClientApp`；而点击回调
/// 又必须能把状态写回去并触发重绘 —— 共享句柄 + `AppRef` 是这套框架下唯一
/// 不引入第二份真相的写法。
#[derive(Default)]
struct ExpandState {
    thinking: std::collections::HashSet<String>,
    tool_args: std::collections::HashSet<String>,
    tool_result: std::collections::HashSet<String>,
    /// 子代理活动流里展开的那一行（Web 版同口径：同时只展开一个）
    activity: Option<String>,
}

type ExpandRef = Rc<std::cell::RefCell<ExpandState>>;

/// 反向弱引用（点击回调里 `update` 回 view）。
type AppRef = Arc<Mutex<Option<WeakEntity<ClientApp>>>>;

struct ClientApp {
    /// 左栏派生行（分组 / 会话 / 查看更多）：由 `rebuild_session_rows` 从
    /// `sessions_data` + 折叠态 + 选中态 + 任务角标重新算出
    session_rows: Vec<SessionListRow>,
    /// 会话原始数据（左栏分组的真相源；`sessions` 那份是按折叠/选中态**派生**的行）
    sessions_data: Vec<SessionRow>,
    /// 折叠的工作区分组（key = `working_directory`）
    collapsed_groups: std::collections::HashSet<String>,
    /// 组内额外展开条数（key = `working_directory`；点「查看更多」累加）
    expanded_groups: std::collections::HashMap<String, usize>,
    /// 左栏当前鼠标悬停的会话 id —— 行内的「重命名 / 删除」只在悬停时出现
    /// （对齐 Web 版 `hoveredSession === session.id` 的口径）
    hovered_session: Option<String>,
    /// 全量后台任务（会话角标聚合用；面板展示的是按当前会话过滤后的 `tasks`）
    all_tasks: Vec<TaskRow>,
    /// 任务面板里展开的任务 id（同时只展开一个，与 Tauri 版 `openId` 同口径）
    expanded_task: Option<String>,
    /// 子代理任务的消息缓存（`task_id` = 子会话 id → 结构化消息，供活动流渲染）
    task_messages: std::collections::HashMap<String, Vec<MsgRow>>,
    /// 消息区折叠状态（思考块 / 工具参数 / 工具结果）
    expand: ExpandRef,
    /// 子智能体（角色）列表 / 统计 / 详情
    roles: Vec<RoleRow>,
    roles_stats: RolesStats,
    /// 搜索关键词（输入框实体在 `roles_search`）
    roles_query: String,
    roles_search: Entity<InputState>,
    /// 选中的角色名与其详情
    roles_selected: Option<String>,
    role_detail: Option<RoleDetail>,
    /// 角色页状态行（加载中 / 操作结果）
    roles_status: String,
    /// 角色操作结果提示（回退/退役后的一行反馈）
    roles_action_msg: Option<String>,
    /// 工具页：工具目录（`GET /api/v1/tools`，与 LLM 收到的 tools 列表同源）
    tools: Vec<ToolRow>,
    /// 工具页：搜索关键词（输入框实体在 `tools_search`）
    tools_query: String,
    tools_search: Entity<InputState>,
    /// 工具页：选中的工具名（None = 未选，右侧显示引导）
    tools_selected: Option<String>,
    /// 技能页：学习方法论技能（`GET /api/v1/skills` 中 `category == "custom"` 的那批）
    skills: Vec<SkillRow>,
    /// 技能页：技能使用统计（`GET /api/v1/skills/stats`；失败不阻塞面板）
    skill_stats: Vec<SkillStatRow>,
    /// 技能页：累计调用次数（0 = 不显示统计条与排行）
    skills_total_calls: u64,
    /// 技能页：搜索关键词（输入框实体在 `skills_search`）
    skills_query: String,
    skills_search: Entity<InputState>,
    /// 技能页：选中的技能 id
    skills_selected: Option<String>,
    /// 技能页：选中技能的完整 markdown 内容（详情接口按需拉）
    skill_content: Option<String>,
    /// 技能页状态行（加载/失败提示）
    skills_status: String,
    /// 定时任务页：任务列表（`GET /scheduled-tasks`，ADR-024 间隔制）
    sched_tasks: Vec<SchedTaskRow>,
    /// 定时任务页：新建表单是否展开
    sched_form_open: bool,
    /// 定时任务页：表单选中的间隔预设（秒；`0` = 自定义，读 `sched_custom_secs`）
    sched_preset: u64,
    /// 定时任务页：表单输入框（名称 / 自定义秒 / 工作目录 / 指令）
    sched_name: Entity<InputState>,
    sched_custom_secs: Entity<InputState>,
    sched_workspace: Entity<InputState>,
    sched_prompt: Entity<TextareaState>,
    /// 定时任务页状态行（创建/删除结果）
    sched_status: String,
    /// 记忆页：全部记忆条目（`GET /memory`，已聚合 memory/ + user/ + agent/patterns）
    memories: Vec<MemoryRow>,
    /// 记忆页状态行
    memories_status: String,
    /// 记忆页：选中的记忆 uri
    memory_selected: Option<String>,
    /// 记忆页：L2 全文是否展开（默认收起——L0/L1 才是浏览主视图）
    memory_detail_open: bool,
    /// 洞察页：总览统计（`GET /stats`：技能/工具/文档/搜索四个追踪面）
    insight_stats: Option<InsightStats>,
    /// 洞察页：模型用量总计与分组（`GET /usage/stats?days=N`）
    usage_total: Option<UsageTotal>,
    usage_grouped: Vec<UsageGroup>,
    usage_days: i64,
    /// 洞察页：调度器状态（`GET /scheduler/status`）
    scheduler_running: bool,
    scheduler_executing: Option<String>,
    sched_status_tasks: Vec<SchedStatusRow>,
    /// 洞察页状态行
    insights_status: String,
    /// 审批页：工作流配置摘要（`GET /approval/status` 的 `config`）
    approval_mode: String,
    approval_auto: bool,
    approval_timeout: u64,
    approval_max_pending: u64,
    approval_prompt_cmds: Vec<String>,
    approval_rules: Vec<ApprovalRuleRow>,
    /// 审批页：待用户确认的操作指纹（「询问用户」降级链路，agent loop 追问中）
    pending_confirmations: Vec<String>,
    confirmed_action_count: usize,
    /// 审批页：审计记录（后端倒序 50 条，界面显示前 20）
    approval_history: Vec<ApprovalHistRow>,
    /// 审批页状态行
    approvals_page_status: String,
    /// 知识页：当前 tab（0 = 浏览 / 1 = 导入 / 2 = 搜索）
    kb_tab: u8,
    /// 知识页·浏览：当前目录（相对知识根，空 = 根）
    kb_path: String,
    kb_entries: Vec<KbEntryRow>,
    /// 知识页·浏览：选中的条目 uri 与读到的内容（`(level, content)`）
    kb_selected: Option<String>,
    kb_content: Option<(String, String)>,
    /// 知识页状态行（浏览/导入/搜索共用一行）
    kb_status: String,
    /// 知识页·导入：路径与分类输入框、每文件结果
    kb_ingest_path: Entity<InputState>,
    kb_ingest_category: Entity<InputState>,
    kb_ingest_results: Vec<(String, String)>,
    /// 知识页·导入：**内嵌文件浏览器**的当前目录、条目与状态
    ///
    /// 不用系统文件对话框：那是 OS 级模态框，自动化端口既点不到也看不到
    /// （`/state` 里没有它的位置）。内嵌浏览器由 GPUI 自绘，可点、可断言。
    kb_browse_dir: String,
    kb_browse_entries: Vec<KbBrowseEntry>,
    kb_browse_status: String,
    /// 是否显示点开头的隐藏条目（默认关：项目根目录十几个 `.git`/`.cargo`/`.vscode`
    /// 会把第一屏占满，真正想选的文件要滚几屏 —— 选文件的场景下这是纯噪音）
    kb_show_hidden: bool,
    /// 文件选择弹窗里的过滤词（名字包含即匹配；空 = 不过滤）
    kb_pick_query: String,
    kb_pick_search: Entity<InputState>,
    /// 已勾选待导入的文件（完整路径，保持勾选顺序）
    kb_picked: Vec<String>,
    /// 知识页·搜索：查询输入框与结果
    kb_search: Entity<InputState>,
    kb_results: Vec<KbSearchRow>,
    /// 目录选择器：当前浏览目录（绝对路径；浏览根时为「浏览根」）
    picker_current: String,
    /// 目录选择器：上级目录（None = 已在浏览根）
    picker_parent: Option<String>,
    /// 目录选择器：子目录 `(名称, 完整路径)`
    picker_entries: Vec<(String, String)>,
    /// 目录选择器状态行
    picker_status: String,
    /// 文件页：工作区根（绝对路径，来自 tree 响应）
    ws_root: String,
    /// 文件页：当前目录（相对根；`""` = 根）
    ws_path: String,
    /// 文件页：当前目录的条目
    ws_entries: Vec<WsEntry>,
    /// 文件页：打开的文件（`(相对路径, 内容)`）
    ws_file: Option<(String, String)>,
    /// 文件页状态行（加载中 / 截断提示 / 错误）
    ws_status: String,
    /// 文件页：diff 面板是否展开
    ws_diff_open: bool,
    /// 文件页：改动文件列表
    ws_diff_files: Vec<WsDiffFile>,
    /// 文件页：当前查看的 diff（相对路径 + unified 文本）
    ws_diff: Option<(String, String)>,
    /// 文件页：diff 面板状态行
    ws_diff_status: String,
    /// 反向弱引用：`gpui_list` 的渲染闭包只能拿 `&mut App`，点击折叠块时要
    /// 回写 view 状态 → 与旧 `SessionsDelegate` 同一手法
    app_ref: AppRef,
    /// 当前打开的模态对话框（删除确认 / 重命名）
    dialog: Option<Dialog>,
    /// 新会话待绑定的工作目录（「＋新目录」设置）：随首条消息以
    /// `working_directory` 下发，服务端建会话时固化（目录不存在则不绑定）
    new_session_workspace: Option<String>,
    /// 重命名输入框：打开对话框时**新建**一个实体（`InputState` 没有公开的
    /// `set_value`，复用旧实体无法回填原标题，只能每次新建）
    dialog_input: Option<Entity<InputState>>,
    /// 消息用 Rc 包一层：闭包驱动的虚拟列表需要把数据带进 'static 闭包，
    /// Rc::clone 是 O(1)，比每帧 clone Vec<MsgRow> 便宜得多
    messages: Rc<Vec<MsgRow>>,
    /// gpui 原生变高虚拟列表的状态（只有可见项会被 render）
    msg_list_state: GpuiListState,
    /// 会话消息总数（messages 可能只保留最近 N 条用于渲染）
    message_total: usize,
    status: String,
    port: u16,
    session_count: usize,
    current_title: String,
    /// 当前选中会话 id —— 事件路由键：只渲染本会话的 `chat_stream` 增量
    current_session: Option<String>,
    /// 输入框（gpui-component 的 `Textarea`：IME / 选区 / 撤销都由它管，不自造）
    input: Entity<TextareaState>,
    /// 是否有轮在跑（`turn_state` 驱动，同时闸住重复发送）
    streaming: bool,
    /// 当前视图
    view: View,
    /// 设置页：模型列表（`GET /api/v1/config/models`）
    models: Vec<ModelRow>,
    /// 设置页状态行（切换结果 / 错误）
    settings_status: String,
    /// 设置页状态行是否为错误（决定用 `--color-error` 还是次级文字色）
    settings_error: bool,
    /// 会话级模型选择（`(provider, model)`）：None = 回落到 `preferences.chat`
    session_model: Option<(String, String)>,
    /// 模型下拉是否展开
    model_menu_open: bool,
    /// 最近一次 usage 上报的已用 token（上下文环分子）
    ctx_used: u64,
    /// 最近一次 usage 上报的上下文窗口（分母）
    ctx_window: u64,
    /// 会话 token 汇总（`GET /usage/stats?days=0&session_id=`）：
    /// `(未命中输入, 缓存命中输入, 输出)`；全零时 `None`（不显示统计行）
    session_usage: Option<(u64, u64, u64)>,
    /// 最近一次请求性能（`GET /usage/recent?limit=1`）：
    /// `(ttft_ms, duration_ms, completion_tokens)`；`duration_ms < 0` 时 `None`
    request_perf: Option<(Option<i64>, i64, u64)>,
    /// 本会话的后台任务（`GET /api/v1/tasks` 按 `parent_session_id` 过滤）
    tasks: Vec<TaskRow>,
    /// 任务面板是否折叠（折叠成 36px 竖条，对齐 Tauri 版 dock 的收起态）
    tasks_collapsed: bool,
    /// 会话待办（`(status, title)`，来自 `GET /todos?session_id=`）
    todos: Vec<(String, String)>,
    /// 会话目标（`GET /goals?session_id=`，元素是 `{goal, progress, todo_total, todo_done}`）
    goals: Vec<GoalRow>,
    /// 待办/目标窄条是否折叠
    todo_bar_collapsed: bool,
    /// 本会话的待审批操作（`GET /api/v1/approval/status` 按 `session_id` 过滤）
    pending_approvals: Vec<ApprovalRow>,
    /// 待回答的追问（`ask_user` 同步工具挂起时）：`(问题, 选项)`
    clarification: Option<Vec<(String, Vec<String>)>>,
    /// 已选答案（与 `clarification` 等长；空串 = 未答）
    clarify_answers: Vec<String>,
    /// 设置页：provider 列表（含 endpoint / api_key，供编辑回填）
    providers: Vec<ProviderRow>,
    /// 设置页：正在编辑的 provider（None = 未打开编辑器）
    editing_provider: Option<String>,
    /// 编辑框：endpoint
    edit_endpoint: Entity<TextareaState>,
    /// 编辑框：api_key
    edit_api_key: Entity<TextareaState>,
}

impl ClientApp {
    fn new(window: &mut Window, cx: &mut Context<Self>, port: u16) -> Self {
        let expand: ExpandRef = Rc::new(std::cell::RefCell::new(ExpandState::default()));
        let app_ref: AppRef = Arc::new(Mutex::new(None));
        let messages: Rc<Vec<MsgRow>> = Rc::new(Vec::new());
        let total: usize = 0;

        let input = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("输入消息 · Enter 发送 / Shift+Enter 换行")
        });
        // 子智能体页的搜索框：改动即过滤（订阅 Change 事件）
        let roles_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索子智能体..."));
        cx.subscribe_in(
            &roles_search,
            window,
            |this, _input, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.roles_query = this.roles_search.read(cx).value().to_string();
                    cx.notify();
                }
            },
        )
        .detach();
        // 工具页的搜索框：改动即过滤（同 roles_search 的口径）
        let tools_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索工具..."));
        cx.subscribe_in(
            &tools_search,
            window,
            |this, _input, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.tools_query = this.tools_search.read(cx).value().to_string();
                    cx.notify();
                }
            },
        )
        .detach();
        // 技能页的搜索框（同口径）
        let skills_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索技能..."));
        cx.subscribe_in(
            &skills_search,
            window,
            |this, _input, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.skills_query = this.skills_search.read(cx).value().to_string();
                    cx.notify();
                }
            },
        )
        .detach();
        // 文件选择弹窗的过滤框：改动即过滤（同 roles/tools 搜索框的口径）
        let kb_pick_search = cx.new(|cx| InputState::new(window, cx).placeholder("按名字过滤…"));
        cx.subscribe_in(
            &kb_pick_search,
            window,
            |this, _input, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.kb_pick_query = this.kb_pick_search.read(cx).value().to_string();
                    cx.notify();
                }
            },
        )
        .detach();
        // 知识页的三个输入框（导入路径 / 导入分类 / 搜索词）
        let kb_ingest_path =
            cx.new(|cx| InputState::new(window, cx).placeholder("文件或目录的绝对路径"));
        let kb_ingest_category = cx
            .new(|cx| InputState::new(window, cx).placeholder("分类（可选，如 technical / docs）"));
        let kb_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索知识库..."));
        // 定时任务页的四个表单输入框（名称 / 自定义秒 / 工作目录 / 指令）。
        // 不订阅 Change：值只在「创建」时读一次，没必要每敲一个字就刷视图。
        let sched_name = cx.new(|cx| InputState::new(window, cx).placeholder("任务名称"));
        let sched_custom_secs =
            cx.new(|cx| InputState::new(window, cx).placeholder("自定义间隔（秒，最小 60）"));
        let sched_workspace = cx
            .new(|cx| InputState::new(window, cx).placeholder("工作目录（绝对路径，留空 = 默认）"));
        let sched_prompt = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("给智能体的指令（每次触发让它做的事）")
        });
        // 设置页的两个编辑框：endpoint 与 api_key（打开编辑时从 `GET /config` 回填）
        let endpoint_input =
            cx.new(|cx| TextareaState::new(window, cx).placeholder("API 端点（http(s)://…）"));
        let key_input = cx.new(|cx| TextareaState::new(window, cx).placeholder("API 密钥"));
        // Enter 发送：Textarea 已按多行语义插入换行，发送成功后再清空即无残留
        cx.subscribe_in(
            &input,
            window,
            |this, _input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift, .. } = event {
                    if !shift {
                        this.send_current_input(window, cx);
                    }
                }
            },
        )
        .detach();

        let app = Self {
            session_rows: Vec::new(),
            sessions_data: Vec::new(),
            collapsed_groups: std::collections::HashSet::new(),
            expanded_groups: std::collections::HashMap::new(),
            hovered_session: None,
            all_tasks: Vec::new(),
            expanded_task: None,
            task_messages: std::collections::HashMap::new(),
            expand,
            app_ref: app_ref.clone(),
            roles: Vec::new(),
            roles_stats: RolesStats::default(),
            roles_query: String::new(),
            roles_search,
            roles_selected: None,
            role_detail: None,
            roles_status: "正在加载子智能体…".to_string(),
            roles_action_msg: None,
            tools: Vec::new(),
            tools_query: String::new(),
            tools_search,
            tools_selected: None,
            skills: Vec::new(),
            skill_stats: Vec::new(),
            skills_total_calls: 0,
            skills_query: String::new(),
            skills_search,
            skills_selected: None,
            skill_content: None,
            skills_status: "正在加载技能…".to_string(),
            sched_tasks: Vec::new(),
            sched_form_open: false,
            sched_preset: 21600,
            sched_name,
            sched_custom_secs,
            sched_workspace,
            sched_prompt,
            sched_status: String::new(),
            memories: Vec::new(),
            memories_status: "正在加载记忆…".to_string(),
            memory_selected: None,
            memory_detail_open: false,
            insight_stats: None,
            usage_total: None,
            usage_grouped: Vec::new(),
            usage_days: 7,
            scheduler_running: false,
            scheduler_executing: None,
            sched_status_tasks: Vec::new(),
            insights_status: "正在加载洞察…".to_string(),
            approval_mode: String::new(),
            approval_auto: false,
            approval_timeout: 0,
            approval_max_pending: 0,
            approval_prompt_cmds: Vec::new(),
            approval_rules: Vec::new(),
            pending_confirmations: Vec::new(),
            confirmed_action_count: 0,
            approval_history: Vec::new(),
            approvals_page_status: "正在加载审批状态…".to_string(),
            kb_tab: 0,
            kb_path: String::new(),
            kb_entries: Vec::new(),
            kb_selected: None,
            kb_content: None,
            kb_status: "正在加载知识库…".to_string(),
            kb_ingest_path,
            kb_ingest_category,
            kb_ingest_results: Vec::new(),
            kb_browse_dir: String::new(),
            kb_browse_entries: Vec::new(),
            kb_browse_status: String::new(),
            kb_show_hidden: false,
            kb_pick_query: String::new(),
            kb_pick_search,
            kb_picked: Vec::new(),
            kb_search,
            kb_results: Vec::new(),
            picker_current: String::new(),
            picker_parent: None,
            picker_entries: Vec::new(),
            picker_status: String::new(),
            ws_root: String::new(),
            ws_path: String::new(),
            ws_entries: Vec::new(),
            ws_file: None,
            ws_status: String::new(),
            ws_diff_open: false,
            ws_diff_files: Vec::new(),
            ws_diff: None,
            ws_diff_status: String::new(),
            dialog: None,
            new_session_workspace: None,
            dialog_input: None,
            messages,
            msg_list_state: GpuiListState::new(0, ListAlignment::Bottom, px(120.)),
            message_total: total,
            status: "正在连接后端…".to_string(),
            port,
            session_count: 0,
            current_title: "选择左侧会话查看消息".to_string(),
            current_session: None,
            input,
            streaming: false,
            view: View::Chat,
            models: Vec::new(),
            settings_status: "正在加载模型列表…".to_string(),
            settings_error: false,
            session_model: None,
            model_menu_open: false,
            ctx_used: 0,
            ctx_window: 0,
            session_usage: None,
            request_perf: None,
            tasks: Vec::new(),
            tasks_collapsed: false,
            todos: Vec::new(),
            goals: Vec::new(),
            todo_bar_collapsed: false,
            pending_approvals: Vec::new(),
            clarification: None,
            clarify_answers: Vec::new(),
            providers: Vec::new(),
            editing_provider: None,
            edit_endpoint: endpoint_input,
            edit_api_key: key_input,
        };
        // 回填自身弱引用：消息区折叠块的点击回调要用它 update 回 view
        if let Ok(mut g) = app_ref.lock() {
            *g = Some(cx.entity().downgrade());
        }
        // 统一事件常驻订阅（ADR-028）：订阅在窗口创建时建立，与「发送」解耦
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
        spawn_event_reader(port, tx);
        cx.spawn(async move |this, cx| loop {
            let Some(ev) = rx.recv().await else {
                break;
            };
            if this
                .update(cx, |this, cx| this.handle_event(&ev, cx))
                .is_err()
            {
                break;
            }
        })
        .detach();
        app.load_sessions(cx);
        app.load_models(cx);
        // 子智能体页数据预加载：等到切页时才拉会让用户先看到空白再「跳」出内容
        app.load_roles(cx);
        app.load_roles_stats(cx);
        // 后台任务面板轮询（3s，对齐 Tauri 版 `AgentTasksPanel.POLL_INTERVAL_MS`）。
        // 面板是观测面：只在有选中会话时拉，失败静默（`load_tasks` 内部处理）。
        cx.spawn(async move |this, cx| loop {
            sleep_secs(3).await;
            let alive = this.update(cx, |this, cx| {
                // 任务列表**无条件**轮询：左栏会话角标要按 `parent_session_id`
                // 聚合所有会话，未选中会话时也得更新；面板那三个只在有会话时拉。
                this.load_tasks(cx);
                if this.current_session.is_some() {
                    this.load_todos_goals(cx);
                    this.load_approvals(cx);
                }
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
        app
    }

    fn load_sessions(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            match fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/sessions")).await {
                Ok(body) => {
                    let arr = body
                        .get("sessions")
                        .and_then(|v| v.as_array())
                        .or_else(|| body.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let rows: Vec<SessionRow> = arr
                        .iter()
                        .map(|s| SessionRow {
                            id: s
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            title: s
                                .get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            working_directory: s
                                .get("working_directory")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            updated_at: s
                                .get("updated_at")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                        })
                        .collect();
                    let n = rows.len();
                    println!("[会话] 拉到 {n} 条");
                    cx.update(|app| {
                        let _ = this.update(app, |this, cx| {
                            this.sessions_data = rows;
                            this.session_count = n;
                            this.status = format!("127.0.0.1:{port} · 会话 {n} 条");
                            this.rebuild_session_rows(cx);
                        });
                    });
                }
                Err(e) => {
                    println!("[会话] 拉取失败: {e}");
                    cx.update(|app| {
                        let _ = this.update(app, |this, cx| {
                            this.status = format!("拉取会话失败：{e}");
                            cx.notify();
                        });
                    });
                }
            }
        })
        .detach();
    }

    /// 打开「重命名会话」对话框：新建输入框实体并回填原标题。
    ///
    /// `InputState` 没有公开的 `set_value`，所以每次开弹窗新建一个实体，
    /// 而不是复用一个再回填（复用的写法编译不过）。
    fn open_rename(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let title = self
            .sessions_data
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone())
            .unwrap_or_default();
        self.dialog_input = Some(cx.new(|cx| InputState::new(window, cx).default_value(title)));
        self.dialog = Some(Dialog::RenameSession { id });
        cx.notify();
    }

    /// 打开目录选择器（先拉浏览根 = 盘符/家目录，再逐级进入）。
    ///
    /// 取代此前的「手输绝对路径」：路径打错不会有任何提示，浏览选择才是正常交互。
    fn open_workspace_picker(&mut self, target: PickerTarget, cx: &mut Context<Self>) {
        self.dialog = Some(Dialog::WorkspacePicker { target });
        self.picker_current.clear();
        self.picker_parent = None;
        self.picker_entries.clear();
        self.picker_status = "正在读取目录…".to_string();
        self.load_picker_dirs(String::new(), cx);
        cx.notify();
    }

    /// 拉某一级目录的子目录列表（`GET /workspace/dirs?path=`；空串 = 浏览根）。
    fn load_picker_dirs(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = if path.is_empty() {
                format!("http://{SERVER_HOST}:{port}/api/v1/workspace/dirs")
            } else {
                format!(
                    "http://{SERVER_HOST}:{port}/api/v1/workspace/dirs?path={}",
                    url_encode(&path)
                )
            };
            match fetch_json(url).await {
                Ok(body) => {
                    let current = body
                        .get("current")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let parent = body
                        .get("parent")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let entries: Vec<(String, String)> = body
                        .get("entries")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| {
                                    Some((
                                        e.get("name")?.as_str()?.to_string(),
                                        e.get("path")?.as_str()?.to_string(),
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = entries.len();
                    let _ = this.update(cx, |this, cx| {
                        this.picker_current = current;
                        this.picker_parent = parent;
                        this.picker_entries = entries;
                        this.picker_status = format!("{n} 个子目录");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.picker_status = format!("读取目录失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 目录选择器「选这个目录」：按目标分派（新会话待绑定 / 绑定当前会话）。
    fn confirm_workspace_picker(&mut self, target: PickerTarget, cx: &mut Context<Self>) {
        // 浏览根（未进入任何盘符）时不可选
        if self.picker_current.is_empty() || self.picker_current == "浏览根" {
            self.picker_status = "请先进入一个目录".to_string();
            cx.notify();
            return;
        }
        let path = self.picker_current.clone();
        self.dialog = None;
        match target {
            PickerTarget::NewSession => {
                self.new_session_workspace = Some(path.clone());
                self.start_new_session(cx);
                self.status = format!("新会话将绑定目录：{path}");
                cx.notify();
            }
            PickerTarget::Session => {
                let Some(sid) = self.current_session.clone() else {
                    self.ws_status = "没有活动会话，无法绑定目录".to_string();
                    cx.notify();
                    return;
                };
                let port = self.port;
                cx.spawn(async move |this, cx| {
                    let url =
                        format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{sid}/workspace");
                    match put_json(url, serde_json::json!({ "working_directory": path })).await {
                        Ok(_) => {
                            let _ = this.update(cx, |this, cx| {
                                if let Some(s) = this.sessions_data.iter_mut().find(|s| s.id == sid)
                                {
                                    s.working_directory = path.clone();
                                }
                                // 目录变了：文件页缓存作废并重新拉树
                                this.ws_root.clear();
                                this.ws_entries.clear();
                                this.ws_file = None;
                                this.ws_path.clear();
                                this.load_workspace_tree(String::new(), cx);
                                this.rebuild_session_rows(cx);
                                cx.notify();
                            });
                        }
                        Err(e) => {
                            let _ = this.update(cx, |this, cx| {
                                this.ws_status = format!("绑定目录失败：{e}");
                                cx.notify();
                            });
                        }
                    }
                })
                .detach();
            }
        }
    }

    /// 提交重命名（`POST /sessions/{id}/title`），成功后就地更新左栏标题。
    fn submit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::RenameSession { id }) = self.dialog.clone() else {
            return;
        };
        let title = self
            .dialog_input
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_string())
            .unwrap_or_default();
        self.dialog = None;
        self.dialog_input = None;
        if title.is_empty() {
            cx.notify();
            return;
        }
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{id}/title");
            match post_json(url, serde_json::json!({ "title": title })).await {
                Ok(_) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(s) = this.sessions_data.iter_mut().find(|s| s.id == id) {
                            s.title = title.clone();
                        }
                        this.rebuild_session_rows(cx);
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.status = format!("重命名失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 确认删除会话（`DELETE /sessions/{id}`）。
    ///
    /// 删除的是当前会话时清空主区（消息 / 统计 / 待办 / 目标），回到「选一个会话」空态。
    fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::DeleteSession { id, .. }) = self.dialog.clone() else {
            return;
        };
        self.dialog = None;
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{id}");
            match delete_json(url).await {
                Ok(()) => {
                    let _ = this.update(cx, |this, cx| {
                        this.sessions_data.retain(|s| s.id != id);
                        if this.current_session.as_deref() == Some(id.as_str()) {
                            this.current_session = None;
                            this.messages = Rc::new(Vec::new());
                            this.msg_list_state =
                                GpuiListState::new(0, ListAlignment::Bottom, px(120.));
                            this.message_total = 0;
                            this.current_title = "选择左侧会话查看消息".to_string();
                            this.session_usage = None;
                            this.request_perf = None;
                            this.todos.clear();
                            this.goals.clear();
                        }
                        this.rebuild_session_rows(cx);
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.status = format!("删除失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 模态对话框覆盖层（删除确认 / 重命名）：整窗半透明遮罩 + 居中卡片。
    ///
    /// 自绘而不接组件库 `Dialog`：只有两种形态，自绘更可控（遮罩色、按钮危险色
    /// 都能直接对齐深色主题），也避免为弹窗引入一套焦点/Popover 语义。
    fn render_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.dialog.clone() else {
            return div().into_any_element();
        };
        // 目录选择器是「列表 + 逐级导航」的独立形态，不走下面那条卡片骨架
        if let Dialog::WorkspacePicker { target } = dialog {
            return self.render_workspace_picker_dialog(target, cx);
        }
        // 知识导入的文件选择弹窗同理：列表 + 勾选 + 导航，独立形态
        if let Dialog::KbFilePicker = dialog {
            return self.render_kb_picker_dialog(cx);
        }
        let (heading, message, confirm_label, danger) = match &dialog {
            Dialog::DeleteSession { title, .. } => (
                "删除会话".to_string(),
                Some(format!("确定删除会话「{title}」吗？此操作不可恢复。")),
                "删除".to_string(),
                true,
            ),
            Dialog::RenameSession { .. } => {
                ("重命名会话".to_string(), None, "保存".to_string(), false)
            }
            // 目录选择器是独立形态（列表 + 导航），在 `render_dialog` 开头就被分流，
            // 走不到这里；给个兜底以免新增变体时漏配
            Dialog::WorkspacePicker { .. } => {
                ("选择目录".to_string(), None, "选择".to_string(), false)
            }
            Dialog::KbFilePicker => ("选择文件".to_string(), None, "导入".to_string(), false),
            Dialog::ResetRole { name } => (
                "回退内置种子".to_string(),
                Some(format!(
                    "将 {name} 回退到内置种子定义（学习演化结果会被覆盖）？"
                )),
                "回退".to_string(),
                true,
            ),
            Dialog::DeleteRole { name } => (
                "退役删除角色".to_string(),
                Some(format!("退役并删除角色 {name}（定义与会话一并删除）？")),
                "删除".to_string(),
                true,
            ),
        };
        let mut card = div()
            .w(px(420.))
            .rounded_xl()
            .border_1()
            .border_color(rgb(colors::BORDER))
            .bg(rgb(colors::BG_SECONDARY))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(15.))
                    .text_color(rgb(colors::TEXT_PRIMARY))
                    .child(heading),
            );
        if let Some(msg) = message {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(rgb(colors::TEXT_SECONDARY))
                    .child(msg),
            );
        }
        // 重命名用输入框（目录选择器是独立形态，走别的分支）
        if let (Dialog::RenameSession { .. }, Some(input)) = (&dialog, self.dialog_input.as_ref()) {
            card = card.child(
                div()
                    .w_full()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER))
                    .bg(rgb(colors::BG_PRIMARY))
                    .px_2()
                    .py_1()
                    .child(Input::new(input).appearance(false)),
            );
        }
        card = card.child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(
                    div()
                        .id("dlg-cancel")
                        .px_3()
                        .py_1()
                        .rounded_lg()
                        .cursor_pointer()
                        .text_sm()
                        .text_color(rgb(colors::TEXT_SECONDARY))
                        .child("取消")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.dialog = None;
                            this.dialog_input = None;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("dlg-confirm")
                        .px_3()
                        .py_1()
                        .rounded_lg()
                        .cursor_pointer()
                        .text_sm()
                        .bg(rgb(if danger {
                            colors::ERROR
                        } else {
                            colors::ACCENT
                        }))
                        .text_color(rgb(0xffffff))
                        .child(confirm_label)
                        .on_click(
                            cx.listener(move |this, _, _, cx| match this.dialog.clone() {
                                Some(Dialog::DeleteSession { .. }) => this.confirm_delete(cx),
                                Some(Dialog::RenameSession { .. }) => this.submit_rename(cx),
                                Some(Dialog::WorkspacePicker { .. }) => {}
                                // 文件选择弹窗同样是独立形态（不走这条骨架）
                                Some(Dialog::KbFilePicker) => {}
                                Some(Dialog::ResetRole { name }) => {
                                    this.confirm_reset_role(name, cx)
                                }
                                Some(Dialog::DeleteRole { name }) => {
                                    this.confirm_delete_role(name, cx)
                                }
                                None => {}
                            }),
                        ),
                ),
        );
        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a0))
            .child(card)
            .into_any_element()
    }

    /// 左栏的一行（分组 / 会话 / 查看更多）。
    ///
    /// 行高由内容决定，不再用等高虚拟列表：三类行的高度本来就不一样，而
    /// `List` 的测量缓存会把「查看更多」压到上一行标题上（实测重叠）。
    fn render_session_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.session_rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        match row {
            SessionListRow::Group {
                key,
                label,
                count,
                collapsed,
            } => {
                let chevron = if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                };
                div()
                    .id(("ws-group", ix))
                    .w_full()
                    .px_1()
                    .py_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .rounded_md()
                    .cursor_pointer()
                    .child(
                        svg()
                            .path(chevron.path())
                            .size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY)),
                    )
                    .child(
                        svg()
                            .path(IconName::FolderOpen.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!("{count}")),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_group(key.clone(), cx);
                    }))
                    .into_any_element()
            }
            SessionListRow::Session {
                id,
                title,
                meta,
                tasks,
                selected,
            } => {
                // 行内操作按钮要带走 id / 标题（`title` 随后会被 move 进标题节点）
                let rename_id = id.clone();
                let delete_id = id.clone();
                let hover_id = id.clone();
                let delete_title = title.clone();
                // 悬停才显示行内操作（对齐 Web 版）；容器宽度固定，
                // 避免鼠标进出时标题区反复变宽变窄
                let is_hovered = self.hovered_session.as_deref() == Some(id.as_str());
                let mut head = div().flex().flex_row().items_center().gap_2().child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_sm()
                        .text_color(rgb(if selected {
                            colors::ACCENT
                        } else {
                            colors::TEXT_PRIMARY
                        }))
                        .child(if title.is_empty() {
                            "新对话".to_string()
                        } else {
                            title
                        }),
                );
                if tasks > 0 {
                    head = head.child(
                        div()
                            .flex_shrink_0()
                            .px_1()
                            .rounded_full()
                            .bg(rgb(colors::ERROR))
                            .text_size(px(9.))
                            .text_color(rgb(0xffffff))
                            .child(if tasks > 99 {
                                "99+".to_string()
                            } else {
                                format!("{tasks}")
                            }),
                    );
                }
                // 行内操作：重命名 / 删除。**只在悬停这一行时渲染**（对齐 Web 版
                // `hoveredSession === session.id`）；容器宽度固定，避免鼠标进出时标题区
                // 反复变宽变窄。两个回调都 `stop_propagation()`——整行的 on_click 是
                // 「打开会话」，不拦住会把弹窗和切会话一起触发。
                head = head.child(
                    div()
                        .w(px(50.))
                        .flex_shrink_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_end()
                        .gap_1()
                        .child(if is_hovered {
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .id(("rename", ix))
                                        .size(px(22.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .child(
                                            svg()
                                                .path(IconName::Pencil.path())
                                                .size(px(12.))
                                                .text_color(rgb(colors::TEXT_TERTIARY)),
                                        )
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.open_rename(rename_id.clone(), window, cx);
                                        })),
                                )
                                .child(
                                    div()
                                        .id(("delete", ix))
                                        .size(px(22.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .child(
                                            svg()
                                                .path(IconName::Trash.path())
                                                .size(px(12.))
                                                .text_color(rgb(colors::TEXT_TERTIARY)),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.dialog = Some(Dialog::DeleteSession {
                                                id: delete_id.clone(),
                                                title: delete_title.clone(),
                                            });
                                            cx.notify();
                                        })),
                                )
                                .into_any_element()
                        } else {
                            div().into_any_element()
                        }),
                );
                div()
                    .id(("ws-session", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(if selected {
                        colors::BG_TERTIARY
                    } else {
                        colors::BG_SECONDARY
                    }))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(head)
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(meta),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_session(id.clone(), cx);
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                        // 行内操作按钮只在悬停这一行时渲染（对齐 Web 版口径）
                        let next = if *hovered {
                            Some(hover_id.clone())
                        } else {
                            None
                        };
                        if this.hovered_session != next {
                            this.hovered_session = next;
                            cx.notify();
                        }
                    }))
                    .into_any_element()
            }
            SessionListRow::More { key, hidden } => div()
                .id(("ws-more", ix))
                .w_full()
                .pl_4()
                .pr_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_SECONDARY))
                .child("查看更多")
                .child(
                    div()
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child(format!("({hidden})")),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.expand_group(key.clone(), cx);
                }))
                .into_any_element(),
        }
    }

    /// 拉本会话的 token 汇总与最近一次请求性能（输入卡片底部统计行）。
    ///
    /// 对应 Web 版两个接口：
    /// - `/usage/stats?days=0&session_id=` 按会话聚合 `usage_logs`（全时段，与消息分页无关）
    /// - `/usage/recent?limit=1&session_id=` 最近一条样本（ttft / 时长 / 输出 token）
    ///
    /// 失败**静默保留旧值**：统计条是观测面，不能因一次抖动把已有数字清空。
    fn load_session_usage(&self, cx: &mut Context<Self>) {
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let stats = fetch_json(format!(
                "http://{SERVER_HOST}:{port}/api/v1/usage/stats?days=0&session_id={sid}"
            ))
            .await
            .ok();
            let recent = fetch_json(format!(
                "http://{SERVER_HOST}:{port}/api/v1/usage/recent?limit=1&session_id={sid}"
            ))
            .await
            .ok();
            let usage = stats
                .as_ref()
                .and_then(|b| b.get("total"))
                .map(|t| {
                    (
                        t.get("uncached_input")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        t.get("cached_input").and_then(|v| v.as_u64()).unwrap_or(0),
                        t.get("completion_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                    )
                })
                .filter(|(a, b, c)| a + b + c > 0);
            let perf = recent
                .as_ref()
                .and_then(|b| b.get("samples"))
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|s| {
                    let duration = s.get("duration_ms").and_then(|v| v.as_i64())?;
                    if duration < 0 {
                        return None;
                    }
                    let ttft = s
                        .get("ttft_ms")
                        .and_then(|v| v.as_i64())
                        .filter(|v| *v >= 0);
                    let completion = s
                        .get("completion_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    Some((ttft, duration, completion))
                });
            let _ = this.update(cx, |this, cx| {
                this.session_usage = usage;
                this.request_perf = perf;
                cx.notify();
            });
        })
        .detach();
    }

    /// 加载子智能体列表（`GET /roles`）。
    /// 加载审批页（`GET /approval/status`：工作流配置 + 待处理 + 审计）。
    ///
    /// 同时刷一次待审批列表（`load_approvals`）—— 那个走的是同一份
    /// `pending_approvals`，两个入口（对话横幅 / 审批页）必须看到同一批数据。
    /// 打开文件选择弹窗：默认定位到「当前会话的工作目录」→「用户主目录」。
    ///
    /// 直接落在盘符根是很糟的默认值 —— 一屏全是 `hiberfil.sys` / `pagefile.sys`
    /// 这类系统文件，既找不到东西也说明不了从哪开始找。
    fn open_kb_picker(&mut self, cx: &mut Context<Self>) {
        self.dialog = Some(Dialog::KbFilePicker);
        self.kb_picked.clear();
        let start = self.kb_picker_start_dir();
        self.load_kb_browse(start, cx);
        cx.notify();
    }

    /// 文件选择弹窗的起始目录。
    fn kb_picker_start_dir(&self) -> String {
        if let Some(sid) = &self.current_session {
            if let Some(s) = self.sessions_data.iter().find(|s| &s.id == sid) {
                if !s.working_directory.is_empty() {
                    return s.working_directory.clone();
                }
            }
        }
        std::env::var("USERPROFILE").unwrap_or_default()
    }

    /// 文件选择弹窗的数据源：列某一级（目录 + 文件，`include_files=true`）。
    ///
    /// 走 `/workspace/dirs` 而不是新接口：同一个目录遍历，只是多一个开关
    /// （目录选择器仍只要目录）。空路径 = 浏览根（盘符列表）。
    fn load_kb_browse(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let mut url =
                format!("http://{SERVER_HOST}:{port}/api/v1/workspace/dirs?include_files=true");
            if !path.is_empty() {
                url.push_str(&format!("&path={}", url_encode(&path)));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let current = body
                        .get("current")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let rows: Vec<KbBrowseEntry> = body
                        .get("entries")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| {
                                    Some(KbBrowseEntry {
                                        name: e.get("name")?.as_str()?.to_string(),
                                        path: e.get("path")?.as_str()?.to_string(),
                                        is_dir: e
                                            .get("is_dir")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                        size: e.get("size").and_then(|v| v.as_u64()),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.kb_browse_dir = current;
                        this.kb_browse_entries = rows;
                        this.kb_browse_status = format!("{n} 项");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.kb_browse_status = format!("读取目录失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 勾选 / 取消勾选一个文件（目录不参与勾选，点它只是进入）。
    fn toggle_kb_pick(&mut self, path: String) {
        if let Some(pos) = self.kb_picked.iter().position(|p| p == &path) {
            self.kb_picked.remove(pos);
        } else {
            self.kb_picked.push(path);
        }
    }

    /// 导入已勾选的文件（浏览器路径与「按路径导入」共用 [`ClientApp::kb_ingest_paths`]）。
    fn import_kb_picked(&mut self, cx: &mut Context<Self>) {
        if self.kb_picked.is_empty() {
            self.kb_status = "还没勾选文件".to_string();
            cx.notify();
            return;
        }
        let paths: Vec<std::path::PathBuf> = self
            .kb_picked
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
        let category = self.kb_ingest_category.read(cx).value().trim().to_string();
        self.kb_ingest_paths(paths, category, cx);
    }

    /// 知识页·浏览：拉某一级条目（`GET /knowledge/entries?path=`）。
    fn load_kb_entries(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let mut url = format!("http://{SERVER_HOST}:{port}/api/v1/knowledge/entries");
            if !path.is_empty() {
                url.push_str(&format!("?path={}", url_encode(&path)));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<KbEntryRow> = body
                        .get("entries")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| {
                                    Some(KbEntryRow {
                                        uri: e.get("uri")?.as_str()?.to_string(),
                                        name: e
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        is_dir: e
                                            .get("is_directory")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                        has_abstract: e
                                            .get("has_abstract")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                        has_overview: e
                                            .get("has_overview")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                        has_detail: e
                                            .get("has_detail")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.kb_path = path;
                        this.kb_entries = rows;
                        this.kb_status = format!("{n} 个条目");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.kb_status = format!("加载知识条目失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 知识页·浏览：读条目某一层内容（`GET /knowledge/entries/read`）。
    fn read_kb_entry(&self, uri: String, level: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/knowledge/entries/read?uri={}&level={}",
                url_encode(&uri),
                url_encode(&level)
            );
            match fetch_json(url).await {
                Ok(body) => {
                    let got_level = body
                        .get("level")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&level)
                        .to_string();
                    let content = body
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let _ = this.update(cx, |this, cx| {
                        this.kb_selected = Some(uri);
                        this.kb_content = Some((got_level, content));
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.kb_status = format!("读取条目失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 知识页·浏览：删除条目（`POST /knowledge/entries/delete`；仅 knowledge 命名空间）。
    fn delete_kb_entry(&mut self, uri: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/knowledge/entries/delete");
            let res = post_json(url, serde_json::json!({ "uri": uri })).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.kb_status = "已删除".to_string();
                        this.kb_selected = None;
                        this.kb_content = None;
                        this.load_kb_entries(this.kb_path.clone(), cx);
                    }
                    Err(e) => this.kb_status = format!("删除失败：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 知识页·搜索：语义检索（`GET /knowledge/search?q=`）。
    fn search_kb(&self, q: String, cx: &mut Context<Self>) {
        if q.trim().is_empty() {
            return;
        }
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/knowledge/search?q={}&limit=20",
                url_encode(&q)
            );
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<KbSearchRow> = body
                        .get("results")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|r| {
                                    Some(KbSearchRow {
                                        id: r.get("id")?.as_str()?.to_string(),
                                        content: r
                                            .get("content")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        source: r
                                            .get("source")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        score: r
                                            .get("score")
                                            .and_then(|v| v.as_f64())
                                            .unwrap_or(0.0)
                                            as f32,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let total = body.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
                    let _ = this.update(cx, |this, cx| {
                        this.kb_results = rows;
                        this.kb_status = format!("命中 {total} 条");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.kb_status = format!("检索失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 知识页·导入：读本地路径 → multipart 上传（`POST /knowledge/ingest`）。
    ///
    /// 用**路径输入**而不是系统文件选择器：与 workspace 目录选择器同一手感，
    /// 且能被自动化 API 直接驱动（文件选择器是模态的，脚本没法点）。
    /// 目录会递归收集（上限 200 个文件——防手滑把整个盘导进去）。
    fn ingest_kb(&mut self, cx: &mut Context<Self>) {
        let raw = self.kb_ingest_path.read(cx).value().trim().to_string();
        let category = self.kb_ingest_category.read(cx).value().trim().to_string();
        self.kb_ingest_direct(raw, category, cx);
    }

    /// 路径输入入口：单一路径 → 交给 [`ClientApp::kb_ingest_paths`]。
    ///
    /// 自动化 `kb_ingest` 与手输路径共用这一条；文件选择器直接走 `kb_ingest_paths`。
    fn kb_ingest_direct(&mut self, raw: String, category: String, cx: &mut Context<Self>) {
        if raw.is_empty() {
            self.kb_status = "请选择文件/目录，或填写绝对路径".to_string();
            cx.notify();
            return;
        }
        self.kb_ingest_paths(vec![std::path::PathBuf::from(&raw)], category, cx);
    }

    /// 按路径列表导入（**路径输入与文件选择器共用这一份**）。
    fn kb_ingest_paths(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        category: String,
        cx: &mut Context<Self>,
    ) {
        let files = collect_ingest_files(&paths);
        if files.is_empty() {
            self.kb_status = "选中的路径里没有可导入的文件".to_string();
            cx.notify();
            return;
        }
        let port = self.port;
        self.kb_status = format!("正在导入 {} 个文件…", files.len());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut form = reqwest::multipart::Form::new();
            if !category.is_empty() {
                form = form.text(
                    "metadata",
                    serde_json::json!({ "source_type": category }).to_string(),
                );
            }
            for f in &files {
                // 一次性导入动作，文件不大；此处阻塞读可接受（在 tokio 线程上顺序读）
                let Ok(bytes) = std::fs::read(f) else {
                    continue;
                };
                let name = f
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".to_string());
                form = form.part(
                    "files",
                    reqwest::multipart::Part::bytes(bytes).file_name(name),
                );
            }
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/knowledge/ingest");
            let res = reqwest::Client::new()
                .post(&url)
                .multipart(form)
                .send()
                .await;
            let parsed = match res {
                Ok(resp) => match resp.json::<serde_json::Value>().await {
                    Ok(body) => Ok(body),
                    Err(e) => Err(format!("响应解析失败：{e}")),
                },
                Err(e) => Err(format!("请求失败：{e}")),
            };
            let _ = this.update(cx, |this, cx| {
                match parsed {
                    Ok(body) => {
                        let msg = body
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("已提交")
                            .to_string();
                        this.kb_ingest_results = body
                            .get("files")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|f| {
                                        Some((
                                            f.get("filename")?.as_str()?.to_string(),
                                            f.get("status")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("?")
                                                .to_string(),
                                        ))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        this.kb_status = msg;
                        this.load_kb_entries(String::new(), cx);
                    }
                    Err(e) => this.kb_status = e,
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_approval_page(&self, cx: &mut Context<Self>) {
        self.load_approvals(cx);
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/approval/status");
            match fetch_json(url).await {
                Ok(body) => {
                    let config = body.get("config");
                    let mode = config
                        .and_then(|c| c.get("mode"))
                        .map(|v| {
                            v.as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| v.to_string())
                        })
                        .unwrap_or_default();
                    let auto = config
                        .and_then(|c| c.get("enable_auto_approval"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let timeout = config
                        .and_then(|c| c.get("default_timeout_secs"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let max_pending = config
                        .and_then(|c| c.get("max_pending_approvals"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let prompt_cmds: Vec<String> = config
                        .and_then(|c| c.get("prompt_commands"))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let rules: Vec<ApprovalRuleRow> = config
                        .and_then(|c| c.get("auto_approval_rules"))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|r| {
                                    Some(ApprovalRuleRow {
                                        name: r.get("name")?.as_str()?.to_string(),
                                        decision: r
                                            .get("decision")
                                            .map(|v| {
                                                v.as_str()
                                                    .map(|s| s.to_string())
                                                    .unwrap_or_else(|| v.to_string())
                                            })
                                            .unwrap_or_default(),
                                        enabled: r
                                            .get("enabled")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(true),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let pending_conf: Vec<String> = body
                        .get("pending_confirmations")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let confirmed = body
                        .get("confirmed_action_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    let history: Vec<ApprovalHistRow> = body
                        .get("recent_records")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|r| {
                                    let req = r.get("request")?;
                                    Some(ApprovalHistRow {
                                        description: req
                                            .get("action_description")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        risk: req
                                            .get("risk_level")
                                            .map(|v| {
                                                v.as_str()
                                                    .map(|s| s.to_string())
                                                    .unwrap_or_else(|| v.to_string())
                                            })
                                            .unwrap_or_default(),
                                        decision: r
                                            .get("response")
                                            .and_then(|resp| resp.get("decision"))
                                            .map(|v| {
                                                v.as_str()
                                                    .map(|s| s.to_string())
                                                    .unwrap_or_else(|| v.to_string())
                                            })
                                            .unwrap_or_else(|| "未决".to_string()),
                                        session_id: req
                                            .get("session_id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        result: r.get("execution_result").and_then(|v| v.as_bool()),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = history.len();
                    let _ = this.update(cx, |this, cx| {
                        this.approval_mode = mode;
                        this.approval_auto = auto;
                        this.approval_timeout = timeout;
                        this.approval_max_pending = max_pending;
                        this.approval_prompt_cmds = prompt_cmds;
                        this.approval_rules = rules;
                        this.pending_confirmations = pending_conf;
                        this.confirmed_action_count = confirmed;
                        this.approval_history = history;
                        this.approvals_page_status = format!("审计记录 {n} 条");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.approvals_page_status = format!("加载审批状态失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 加载洞察数据（`/stats` + `/usage/stats?days=N` + `/scheduler/status`）。
    ///
    /// 三个请求都失败也只是少一块，不阻塞整页（每个面独立降级）。
    fn load_insights(&self, cx: &mut Context<Self>) {
        let port = self.port;
        let days = self.usage_days;
        cx.spawn(async move |this, cx| {
            let stats_res = fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/stats")).await;
            let usage_res = fetch_json(format!(
                "http://{SERVER_HOST}:{port}/api/v1/usage/stats?days={days}"
            ))
            .await;
            let sched_res = fetch_json(format!(
                "http://{SERVER_HOST}:{port}/api/v1/scheduler/status"
            ))
            .await;

            let stats = stats_res.ok().map(|b| InsightStats {
                total_skills_tracked: b
                    .get("total_skills_tracked")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_skill_calls: b
                    .get("total_skill_calls")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_tools_tracked: b
                    .get("total_tools_tracked")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_tool_calls: b
                    .get("total_tool_calls")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_docs_tracked: b
                    .get("total_docs_tracked")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_searches: b
                    .get("total_searches")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
            });

            let (usage_total, usage_grouped) = match usage_res {
                Ok(b) => {
                    let total = b.get("total").filter(|v| !v.is_null()).map(|t| UsageTotal {
                        calls: t.get("calls").and_then(|v| v.as_u64()).unwrap_or(0),
                        uncached_input: t
                            .get("uncached_input")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        cached_input: t.get("cached_input").and_then(|v| v.as_u64()).unwrap_or(0),
                        completion_tokens: t
                            .get("completion_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        total_tokens: t.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                        cache_hit_rate: t
                            .get("cache_hit_rate")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0),
                    });
                    let grouped: Vec<UsageGroup> = b
                        .get("grouped")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|g| {
                                    Some(UsageGroup {
                                        group: g.get("group")?.as_str()?.to_string(),
                                        calls: g.get("calls").and_then(|v| v.as_u64()).unwrap_or(0),
                                        total_tokens: g
                                            .get("total_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        cache_hit_rate: g
                                            .get("cache_hit_rate")
                                            .and_then(|v| v.as_f64())
                                            .unwrap_or(0.0),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (total, grouped)
                }
                Err(_) => (None, Vec::new()),
            };

            let (running, executing, sched_tasks) = match sched_res {
                Ok(b) => {
                    let running = b.get("running").and_then(|v| v.as_bool()).unwrap_or(false);
                    let executing = b
                        .get("executing_task_id")
                        .and_then(|v| v.as_str())
                        .map(|v| v.to_string());
                    let tasks: Vec<SchedStatusRow> = b
                        .get("tasks")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|t| {
                                    Some(SchedStatusRow {
                                        id: t.get("id")?.as_str()?.to_string(),
                                        name: t
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        interval_secs: t
                                            .get("interval_secs")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        run_count: t
                                            .get("run_count")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        last_run_ago_secs: t
                                            .get("last_run_ago_secs")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        next_due_in_secs: t
                                            .get("next_due_in_secs")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (running, executing, tasks)
                }
                Err(_) => (false, None, Vec::new()),
            };

            let n = sched_tasks.len();
            let _ = this.update(cx, |this, cx| {
                this.insight_stats = stats;
                this.usage_total = usage_total;
                this.usage_grouped = usage_grouped;
                this.scheduler_running = running;
                this.scheduler_executing = executing;
                this.sched_status_tasks = sched_tasks;
                this.insights_status = format!(
                    "调度器{} · {n} 个任务",
                    if running { "运行中" } else { "未装配" }
                );
                cx.notify();
            });
        })
        .detach();
    }

    /// 加载记忆条目（`GET /memory`）。
    ///
    /// 后端已聚合三类来源（`memory/` 事实与决策、`user/` 画像、`agent/patterns` 模式）
    /// 并且只返回叶子：目录只是导航中间节点，内容为空、importance 是默认值，
    /// 直接展示只会误导（这是后端注释里的原话，界面不再二次筛选）。
    fn load_memories(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/memory");
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<MemoryRow> = body
                        .get("memories")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|m| {
                            let meta = m.get("metadata");
                            let text_of = |key: &str| {
                                m.get(key)
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                                    .filter(|s| !s.trim().is_empty())
                            };
                            Some(MemoryRow {
                                uri: m.get("uri")?.as_str()?.to_string(),
                                name: m
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                relative_path: m
                                    .get("relative_path")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                importance: meta
                                    .and_then(|v| v.get("importance"))
                                    .and_then(|v| v.as_f64())
                                    .unwrap_or(0.5)
                                    as f32,
                                updated_at: meta
                                    .and_then(|v| v.get("updated_at"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                tags: meta
                                    .and_then(|v| v.get("tags"))
                                    .and_then(|v| v.as_array())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|t| t.as_str().map(|s| s.to_string()))
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                                abstract_text: text_of("abstract"),
                                overview_text: text_of("overview"),
                                detail_text: text_of("detail"),
                            })
                        })
                        .collect();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.memories = rows;
                        this.memories_status = format!("{n} 条记忆（L0/L1/L2 三层）");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.memories_status = format!("加载记忆失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 加载定时任务列表（`GET /scheduled-tasks`）。
    fn load_sched_tasks(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/scheduled-tasks");
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<SchedTaskRow> = body
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|t| {
                            Some(SchedTaskRow {
                                id: t.get("id")?.as_str()?.to_string(),
                                name: t
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                interval_secs: t
                                    .get("interval_secs")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0),
                                workspace: t
                                    .get("workspace")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                prompt: t
                                    .get("prompt")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                enabled: t.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                                // RFC3339 字符串（磁盘上的旧 epoch 数字由后端兼容读入）
                                last_run_at: t
                                    .get("last_run_at")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string()),
                                next_run_at: t
                                    .get("next_run_at")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string()),
                                last_result: t
                                    .get("last_result")
                                    .and_then(|v| v.as_str())
                                    .map(|v| v.to_string()),
                            })
                        })
                        .collect();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.sched_tasks = rows;
                        this.sched_status = format!("{n} 个定时任务");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.sched_status = format!("加载定时任务失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 创建定时任务（`POST /scheduled-tasks`）。
    ///
    /// 间隔取预设；预设为 `0`（自定义）时读秒数输入框，最小 60 秒
    /// （与 Web 版 `min={60}` 同口径 —— 更短的间隔没有意义，只会刷屏）。
    fn create_sched_task(&mut self, cx: &mut Context<Self>) {
        let name = self.sched_name.read(cx).value().trim().to_string();
        let workspace = self.sched_workspace.read(cx).value().trim().to_string();
        let prompt = self.sched_prompt.read(cx).value().trim().to_string();
        let secs = if self.sched_preset == 0 {
            self.sched_custom_secs
                .read(cx)
                .value()
                .trim()
                .parse::<u64>()
                .unwrap_or(0)
        } else {
            self.sched_preset
        };
        if name.is_empty() || prompt.is_empty() || secs < 60 {
            self.sched_status = "名称与指令不能为空，间隔至少 60 秒".to_string();
            cx.notify();
            return;
        }
        self.create_sched_task_direct(name, secs, workspace, prompt, cx);
    }

    /// 真正发创建请求（UI 表单与自动化 `sched_create` **共用这一份**）。
    ///
    /// 分开的理由同 ADR-052 D8：自动化若另写一份请求构造，两条路径就分叉了。
    /// 校验也放在调用方之前各做一次，这里只信任入参。
    fn create_sched_task_direct(
        &mut self,
        name: String,
        secs: u64,
        workspace: String,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/scheduled-tasks");
            let body = serde_json::json!({
                "name": name,
                "interval_secs": secs,
                "workspace": workspace,
                "prompt": prompt,
            });
            let res = post_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.sched_status = "定时任务已创建".to_string();
                        this.sched_form_open = false;
                        this.load_sched_tasks(cx);
                    }
                    Err(e) => this.sched_status = format!("创建失败：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 删除定时任务（`DELETE /scheduled-tasks/{id}`）。
    fn delete_sched_task(&mut self, id: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/scheduled-tasks/{}",
                url_encode(&id)
            );
            let res = delete_json(url).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.sched_status = "定时任务已删除".to_string();
                        this.load_sched_tasks(cx);
                    }
                    Err(e) => this.sched_status = format!("删除失败：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 加载学习方法论技能（`GET /skills` + `GET /skills/stats`）。
    ///
    /// 只保留 `category == "custom"`：其余类是内置能力（文件/命令/网络），
    /// 那些由工具面板覆盖（与 Web 版 `SkillsPanel` 同口径）。
    /// 统计失败不阻塞面板（只影响右上统计条与列表里的调用次数）。
    fn load_skills(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let list_url = format!("http://{SERVER_HOST}:{port}/api/v1/skills");
            let stats_url = format!("http://{SERVER_HOST}:{port}/api/v1/skills/stats");
            let list_res = fetch_json(list_url).await;
            let stats_res = fetch_json(stats_url).await;

            let rows: Vec<SkillRow> = list_res
                .ok()
                .filter(|body| body.get("skills").is_some())
                .map(|body| {
                    body.get("skills")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                })
                .unwrap_or_default()
                .iter()
                .filter(|s| s.get("category").and_then(|v| v.as_str()) == Some("custom"))
                .filter_map(|s| {
                    Some(SkillRow {
                        id: s.get("id")?.as_str()?.to_string(),
                        name: s
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        description: s
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        version: s
                            .get("version")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        updated_at: s
                            .get("updated_at")
                            .and_then(|v| v.as_str())
                            .map(|v| v.to_string()),
                    })
                })
                .collect();

            let (stats, total_calls) = match stats_res {
                Ok(body) => {
                    let total = body
                        .get("total_calls")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let stats: Vec<SkillStatRow> = body
                        .get("skills")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| {
                                    Some(SkillStatRow {
                                        skill_id: s.get("skill_id")?.as_str()?.to_string(),
                                        total_calls: s
                                            .get("total_calls")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        success_calls: s
                                            .get("success_calls")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        avg_time_ms: s
                                            .get("avg_time_ms")
                                            .and_then(|v| v.as_f64())
                                            .unwrap_or(0.0),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (stats, total)
                }
                Err(_) => (Vec::new(), 0),
            };

            let n = rows.len();
            let _ = this.update(cx, |this, cx| {
                this.skills = rows;
                this.skill_stats = stats;
                this.skills_total_calls = total_calls;
                this.skills_status = format!("{n} 个方法论技能");
                cx.notify();
            });
        })
        .detach();
    }

    /// 拉选中技能的完整 markdown 内容（`GET /skills/{id}`）。
    fn load_skill_detail(&self, id: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/skills/{}",
                url_encode(&id)
            );
            let res = fetch_json(url).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(body) => {
                        this.skill_content = body
                            .get("content")
                            .and_then(|v| v.as_str())
                            .map(|v| v.to_string());
                        this.skills_status = "已加载技能内容".to_string();
                    }
                    Err(e) => {
                        this.skill_content = None;
                        this.skills_status = format!("加载技能详情失败：{e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 加载工具目录（`GET /api/v1/tools`：与 LLM 收到的 tools 列表同源）。
    fn load_tools(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/tools");
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<ToolRow> = body
                        .get("tools")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|t| {
                                    Some(ToolRow {
                                        name: t.get("name")?.as_str()?.to_string(),
                                        description: t
                                            .get("description")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        parameters: t
                                            .get("parameters")
                                            .cloned()
                                            .unwrap_or(serde_json::Value::Null),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let _ = this.update(cx, |this, cx| {
                        this.tools = rows;
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.status = format!("加载工具失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn load_roles(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            match fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/roles")).await {
                Ok(body) => {
                    let rows: Vec<RoleRow> = body
                        .get("roles")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|r| RoleRow {
                            name: r
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            source: r
                                .get("source")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            status: r
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            version: r.get("version").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                            purpose: r
                                .get("purpose")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            tool_count: r.get("tool_count").and_then(|v| v.as_u64()),
                            model: r
                                .get("model")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            usage: r.get("usage").map(|u| {
                                (
                                    u.get("calls").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                                    u.get("success_rate")
                                        .and_then(|v| v.as_f64())
                                        .unwrap_or(0.0) as f32,
                                )
                            }),
                        })
                        .collect();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.roles = rows;
                        this.roles_status = format!("{n} 个子智能体");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_status = format!("加载子智能体失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 加载子智能体统计（`GET /roles/stats`）：页面顶部那条汇总。
    fn load_roles_stats(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            if let Ok(body) =
                fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/roles/stats")).await
            {
                let stats = RolesStats {
                    total_calls: body
                        .get("total_calls")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    success_rate: body
                        .get("success_rate")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0) as f32,
                    by_task_type: body
                        .get("by_task_type")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|pair| {
                                    let p = pair.as_array()?;
                                    Some((
                                        p.first()?.as_str()?.to_string(),
                                        p.get(1)?.as_u64()? as usize,
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    by_role: body
                        .get("by_role")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|r| {
                                    Some((
                                        r.get("name")?.as_str()?.to_string(),
                                        r.get("calls")?.as_u64()? as u32,
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                let _ = this.update(cx, |this, cx| {
                    this.roles_stats = stats;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 回退内置种子（`POST /roles/{name}/reset`）：成功后刷新列表 / 统计 / 详情。
    ///
    /// 服务端语义是「下次会话边界生效」——已在跑的会话仍用旧定义，所以提示里
    /// 明说这一点，避免用户以为立刻变了。
    fn confirm_reset_role(&mut self, name: String, cx: &mut Context<Self>) {
        self.dialog = None;
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/roles/{name}/reset");
            match post_json(url, serde_json::json!({})).await {
                Ok(_) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_action_msg =
                            Some("已回退内置种子（下次会话边界生效）".to_string());
                        this.load_roles(cx);
                        this.load_roles_stats(cx);
                        this.load_role_detail(name.clone(), cx);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_action_msg = Some(format!("操作失败：{e}"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 退役删除角色（`DELETE /roles/{name}`）：成功后清空选中并刷新列表。
    fn confirm_delete_role(&mut self, name: String, cx: &mut Context<Self>) {
        self.dialog = None;
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/roles/{name}");
            match delete_json(url).await {
                Ok(()) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_action_msg = Some("角色已退役删除".to_string());
                        if this.roles_selected.as_deref() == Some(name.as_str()) {
                            this.roles_selected = None;
                            this.role_detail = None;
                        }
                        this.load_roles(cx);
                        this.load_roles_stats(cx);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_action_msg = Some(format!("操作失败：{e}"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 加载工作区目录树（`GET /workspace/tree?depth=1&session_id=`）。
    ///
    /// 单层浏览（点目录进、点「上级」出）：树的可展开形态要维护每个节点的
    /// 独立状态与嵌套请求，先按单层落，够用且不易错。
    fn load_workspace_tree(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let mut url = format!("http://{SERVER_HOST}:{port}/api/v1/workspace/tree?depth=1");
            if !path.is_empty() {
                url.push_str(&format!("&path={}", url_encode(&path)));
            }
            if let Some(sid) = &session {
                url.push_str(&format!("&session_id={sid}"));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let root = body
                        .get("root")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let mut entries: Vec<WsEntry> = body
                        .get("entries")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| {
                                    Some(WsEntry {
                                        name: e.get("name")?.as_str()?.to_string(),
                                        is_dir: e.get("type").and_then(|v| v.as_str())
                                            == Some("dir"),
                                        path: e
                                            .get("path")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        size: e.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    // 目录在前、文件在后，各自按名字排序（与常见文件浏览器一致）
                    entries.sort_by(|a, b| {
                        b.is_dir
                            .cmp(&a.is_dir)
                            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                    });
                    let n = entries.len();
                    let _ = this.update(cx, |this, cx| {
                        this.ws_root = root;
                        this.ws_path = path;
                        this.ws_entries = entries;
                        this.ws_status = format!("{n} 项");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.ws_status = format!("读取目录失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 读取工作区文件（`GET /workspace/read?limit=2000&session_id=`）。
    fn load_workspace_file(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let mut url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/workspace/read?limit=2000&path={}",
                url_encode(&path)
            );
            if let Some(sid) = &session {
                url.push_str(&format!("&session_id={sid}"));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let truncated = body
                        .get("truncated")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let total = body.get("total_lines").and_then(|v| v.as_u64());
                    let raw = body
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    let text = strip_hashline(raw);
                    let status = if raw.is_empty() {
                        "（空文件或二进制，无法以文本显示）".to_string()
                    } else if truncated {
                        format!(
                            "已截断：仅显示前 2000 行（共 {} 行）",
                            total
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "?".to_string())
                        )
                    } else {
                        format!(
                            "{} 行",
                            total
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| text.lines().count().to_string())
                        )
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.ws_file = Some((path, text));
                        this.ws_status = status;
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.ws_status = format!("读取文件失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 拉工作区改动文件列表（`GET /workspace/diff?base=snapshot`，不带 path）。
    ///
    /// 这是「AI 到底改了什么」的入口。**快照是按消息存的**（后端要求
    /// `message_id` 或 `index`，缺了直接 400）—— `index=0` = 会话起点那一刻的
    /// 工作区快照，也就是「本次会话从头到现在改了什么」。
    fn load_workspace_diff_list(&self, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let mut url =
                format!("http://{SERVER_HOST}:{port}/api/v1/workspace/diff?base=snapshot&index=0");
            if let Some(sid) = &session {
                url.push_str(&format!("&session_id={sid}"));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let files: Vec<WsDiffFile> = body
                        .get("files")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|f| {
                                    Some(WsDiffFile {
                                        path: f.get("path")?.as_str()?.to_string(),
                                        status: f
                                            .get("status")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("modified")
                                            .to_string(),
                                        old_lines: f
                                            .get("old_lines")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        new_lines: f
                                            .get("new_lines")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = files.len();
                    let _ = this.update(cx, |this, cx| {
                        this.ws_diff_files = files;
                        this.ws_diff_status = if n == 0 {
                            "工作区无改动（相对本次会话快照）".to_string()
                        } else {
                            format!("{n} 个文件有改动")
                        };
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.ws_diff_status = format!("读取改动列表失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 拉单个文件的 unified diff（`GET /workspace/diff?base=snapshot&index=0&path=`）。
    fn load_workspace_diff(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let mut url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/workspace/diff?base=snapshot&index=0&path={}",
                url_encode(&path)
            );
            if let Some(sid) = &session {
                url.push_str(&format!("&session_id={sid}"));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let unified = body
                        .get("unified")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let status = body
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("modified")
                        .to_string();
                    let text = if unified.is_empty() {
                        match status.as_str() {
                            "binary" => "（二进制文件，无文本 diff）".to_string(),
                            "unchanged" => "（无改动）".to_string(),
                            _ => "（服务端未返回 unified diff）".to_string(),
                        }
                    } else {
                        unified
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.ws_diff = Some((path, text));
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.ws_diff_status = format!("读取 diff 失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 加载角色详情（`GET /roles/{name}`）。
    fn load_role_detail(&self, name: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/roles/{name}");
            if let Ok(body) = fetch_json(url).await {
                let detail = RoleDetail {
                    system_prompt: body
                        .get("system_prompt")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    tools: body
                        .get("tools")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                let _ = this.update(cx, |this, cx| {
                    this.role_detail = Some(detail);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 该会话正在跑的后台任务数（`pending` / `running` 计）。
    fn running_tasks_of(&self, session_id: &str) -> usize {
        self.all_tasks
            .iter()
            .filter(|t| {
                t.parent_session_id == session_id
                    && (t.status == "pending" || t.status == "running")
            })
            .count()
    }

    /// 折叠 / 展开一个工作区分组。
    fn toggle_group(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.collapsed_groups.remove(&key) {
            self.collapsed_groups.insert(key);
        }
        self.rebuild_session_rows(cx);
    }

    /// 组内「查看更多」：再展开 `SESSIONS_LOAD_STEP` 条。
    fn expand_group(&mut self, key: String, cx: &mut Context<Self>) {
        *self.expanded_groups.entry(key).or_insert(0) += SESSIONS_LOAD_STEP;
        self.rebuild_session_rows(cx);
    }

    /// 新建会话：清空选中与消息。真正的会话记录由服务端在首条消息时按本地
    /// 生成的 id 创建（事件按 `session_id` 路由，id 必须发起前就确定）。
    fn start_new_session(&mut self, cx: &mut Context<Self>) {
        self.current_session = None;
        self.streaming = false;
        self.session_model = None;
        self.model_menu_open = false;
        self.ctx_used = 0;
        self.ctx_window = 0;
        self.messages = Rc::new(Vec::new());
        self.msg_list_state = GpuiListState::new(0, ListAlignment::Bottom, px(120.));
        self.message_total = 0;
        self.current_title = "新会话（发送首条消息后创建）".to_string();
        self.rebuild_session_rows(cx);
    }

    /// 重建左栏行（分组 / 会话 / 查看更多）。
    ///
    /// 排序对齐 `gui-vite/src/components/session/SessionList.tsx` 的 `sortGroups`：
    /// 1.「最近活跃」段：24h 内有活动的工作区，按组内最近时间倒序，最多 3 个；
    /// 2.「其余」段：按路径字母序（稳定段，位置不随使用漂移）；
    /// 3. 默认组（`""`）始终垫底（即使没有未绑定会话，也要能在这儿新建）。
    ///
    /// 组内折叠：初始 3 条 + 「查看更多」累加；**当前会话始终可见**（从别处切进
    /// 深处会话时不会被折叠藏住，同 Tauri 版）。
    fn rebuild_session_rows(&mut self, cx: &mut Context<Self>) {
        use std::collections::{BTreeMap, HashSet};
        let mut groups: BTreeMap<String, Vec<&SessionRow>> = BTreeMap::new();
        groups.entry(String::new()).or_default();
        for s in &self.sessions_data {
            groups
                .entry(s.working_directory.clone())
                .or_default()
                .push(s);
        }
        for list in groups.values_mut() {
            list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        }
        let now = now_unix_secs();
        let last_activity = |list: &[&SessionRow]| -> i64 {
            list.iter()
                .filter_map(|s| parse_rfc3339_secs(&s.updated_at))
                .max()
                .unwrap_or(0)
        };
        let mut active: Vec<(String, Vec<&SessionRow>)> = groups
            .iter()
            .filter(|(k, l)| !k.is_empty() && last_activity(l) >= now - ACTIVE_GROUP_WINDOW_SECS)
            .map(|(k, l)| (k.clone(), l.clone()))
            .collect();
        active.sort_by(|a, b| last_activity(&b.1).cmp(&last_activity(&a.1)));
        active.truncate(ACTIVE_GROUP_LIMIT);
        let active_keys: HashSet<&str> = active.iter().map(|(k, _)| k.as_str()).collect();
        let mut rest: Vec<(String, Vec<&SessionRow>)> = groups
            .iter()
            .filter(|(k, _)| !k.is_empty() && !active_keys.contains(k.as_str()))
            .map(|(k, l)| (k.clone(), l.clone()))
            .collect();
        rest.sort_by(|a, b| a.0.cmp(&b.0));
        let mut ordered = active;
        ordered.extend(rest);
        ordered.extend(
            groups
                .iter()
                .filter(|(k, _)| k.is_empty())
                .map(|(k, l)| (k.clone(), l.clone())),
        );

        let mut rows: Vec<SessionListRow> = Vec::new();
        for (key, list) in ordered {
            let collapsed = self.collapsed_groups.contains(&key);
            let count = list.len();
            rows.push(SessionListRow::Group {
                label: group_label(&key),
                key: key.clone(),
                count,
                collapsed,
            });
            if collapsed {
                continue;
            }
            let base =
                SESSIONS_INITIAL_VISIBLE + self.expanded_groups.get(&key).copied().unwrap_or(0);
            let visible = match self
                .current_session
                .as_deref()
                .and_then(|id| list.iter().position(|s| s.id == id))
            {
                Some(pos) if pos >= base => pos + 1,
                _ => base,
            }
            .min(count);
            for s in list.iter().take(visible) {
                rows.push(SessionListRow::Session {
                    id: s.id.clone(),
                    title: s.title.clone(),
                    meta: format_relative(&s.updated_at),
                    tasks: self.running_tasks_of(&s.id),
                    selected: self.current_session.as_deref() == Some(s.id.as_str()),
                });
            }
            let hidden = count.saturating_sub(visible);
            if hidden > 0 {
                rows.push(SessionListRow::More { key, hidden });
            }
        }
        self.session_rows = rows;
        cx.notify();
    }

    /// 加载模型服务列表（`GET /api/v1/config/models`）。
    ///
    /// 只取展示与切换需要的字段：provider / 模型名 / 能力标签 / 上下文窗口 /
    /// 是否为当前 chat 首选。`preferences.chat` 是 `{model, provider}`，provider
    /// 可能缺省（同名模型只注册在一个 provider 上时）。
    fn load_models(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/models");
            match fetch_json(url).await {
                Ok(body) => {
                    let pref = body.get("preferences").and_then(|p| p.get("chat"));
                    let chat_model = pref
                        .and_then(|c| c.get("model"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let chat_provider = pref
                        .and_then(|c| c.get("provider"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let mut rows: Vec<ModelRow> = Vec::new();
                    if let Some(models) = body.get("models").and_then(|v| v.as_array()) {
                        for m in models {
                            let name = m
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let provider = m
                                .get("provider")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let capabilities = m
                                .get("capabilities")
                                .and_then(|v| v.as_array())
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|x| x.as_str())
                                        .collect::<Vec<_>>()
                                        .join("/")
                                })
                                .unwrap_or_default();
                            let context = m.get("context_length").and_then(|v| v.as_u64());
                            let is_chat = name == chat_model
                                && (chat_provider.is_empty() || provider == chat_provider);
                            rows.push(ModelRow {
                                provider,
                                name,
                                capabilities,
                                context,
                                is_chat,
                            });
                        }
                    }
                    let n = rows.len();
                    // 再拉一次整份配置：providers 的 endpoint / api_key 只在 `/config`
                    // 里（`/config/models` 只给 name/endpoint/enabled/model_count）。
                    // 编辑回填需要它们，而回填只能在同步点击处理器里做（要 `&mut Window`）。
                    let cfg_res =
                        fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/config")).await;
                    let mut providers: Vec<ProviderRow> = Vec::new();
                    if let Ok(body) = cfg_res {
                        if let Some(arr) = body
                            .get("config")
                            .and_then(|c| c.get("models"))
                            .and_then(|m| m.get("providers"))
                            .and_then(|p| p.as_array())
                        {
                            for p in arr {
                                providers.push(ProviderRow {
                                    name: p
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default()
                                        .to_string(),
                                    endpoint: p
                                        .get("endpoint")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default()
                                        .to_string(),
                                    api_key: p
                                        .get("api_key")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default()
                                        .to_string(),
                                    model_count: p
                                        .get("models")
                                        .and_then(|v| v.as_array())
                                        .map(|a| a.len())
                                        .unwrap_or(0),
                                });
                            }
                        }
                    }
                    let _ = this.update(cx, |this, cx| {
                        this.models = rows;
                        this.providers = providers;
                        this.settings_status = format!("模型 {n} 个");
                        this.settings_error = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings_status = format!("加载模型列表失败：{e}");
                        this.settings_error = true;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 切换默认聊天模型（`POST /api/v1/config/models/switch`，持久化 preferences.chat）。
    fn switch_chat_model(&mut self, provider: String, model: String, cx: &mut Context<Self>) {
        let port = self.port;
        self.settings_status = format!("正在切换到 {model}…");
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/models/switch");
            // capability 用 kebab-case（core 的 ModelCapability 序列化口径）
            let body = serde_json::json!({
                "model": model,
                "capability": "chat",
                "provider": provider,
            });
            match post_json(url, body).await {
                Ok(_) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings_status = format!("已把 {model} 设为聊天模型");
                        this.settings_error = false;
                        this.load_models(cx);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings_status = format!("切换失败：{e}");
                        this.settings_error = true;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 全局一级导航栏（64px 图标 rail）。
    ///
    /// 与 Tauri 版 `Sidebar.tsx` 同构：**固定宽度、不做展开/收起**——会话列表在会话页
    /// 左栏，不在 rail 里；rail 只放 11 个一级入口。未实现的入口点进去是占位页。
    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rail = div()
            .w(px(colors::RAIL_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .py_3()
            .bg(rgb(colors::BG_SECONDARY))
            .border_r_1()
            .border_color(rgb(colors::BORDER))
            // 品牌块（Tauri 版是「天」字方块）
            .child(
                div()
                    .w(px(36.))
                    .h(px(36.))
                    .mb_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .bg(rgb(colors::BG_TERTIARY))
                    .text_color(rgb(colors::ACCENT))
                    .child("天"),
            );
        for (ix, entry) in NAV_ITEMS.iter().enumerate() {
            let (icon, _label, view) = *entry;
            let tint = if self.view == view {
                colors::ACCENT
            } else {
                colors::TEXT_SECONDARY
            };
            let mut item = div()
                .id(("nav", ix))
                .w(px(40.))
                .h(px(36.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .text_sm();
            if self.view == view {
                item = item.bg(rgb(colors::BG_TERTIARY)).text_color(rgb(tint));
            } else {
                item = item.text_color(rgb(tint));
            }
            // Lucide SVG（stroke 用 currentColor → 由 text_color 上色，可随选中态换色）
            rail = rail.child(
                item.child(svg().path(icon.path()).size(px(18.)).text_color(rgb(tint)))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.view = view;
                        if view == View::Settings {
                            this.load_models(cx);
                        }
                        // 数据已在启动时预加载：只在空列表（首次失败/未拉到）时补拉，
                        // 避免每次切页都发请求、页面先空一帧再填
                        if view == View::Roles && this.roles.is_empty() {
                            this.load_roles(cx);
                            this.load_roles_stats(cx);
                        }
                        // 文件页首次进入才拉目录树（工作区根 = 会话绑定目录）
                        if view == View::Workspace && this.ws_root.is_empty() {
                            this.load_workspace_tree(String::new(), cx);
                        }
                        cx.notify();
                    })),
            );
        }
        rail
    }

    /// 切页：**侧栏点击与自动化 `switch_tab` 共用这一份**。
    ///
    /// 「首次进入才补拉」的逻辑必须住在这里。早先它内联在侧栏的 `on_click` 闭包
    /// 里，于是点侧栏会拉数据、而自动化 `switch_tab` 只改 `view` 不拉 —— 两条
    /// 路径分叉，验证结论随即失真（ADR-052 D8 的实例，实测踩过：文件页树一直空）。
    fn switch_view(&mut self, view: View, cx: &mut Context<Self>) {
        self.view = view;
        if view == View::Settings {
            self.load_models(cx);
        }
        // 数据已在启动时预加载：只在空列表（首次失败/未拉到）时补拉，
        // 避免每次切页都发请求、页面先空一帧再填
        if view == View::Roles && self.roles.is_empty() {
            self.load_roles(cx);
            self.load_roles_stats(cx);
        }
        // 文件页首次进入才拉目录树（工作区根 = 会话绑定目录）
        if view == View::Workspace && self.ws_root.is_empty() {
            self.load_workspace_tree(String::new(), cx);
        }
        // 工具页首次进入才拉工具目录（与 roles 同一懒加载口径）
        if view == View::Tools && self.tools.is_empty() {
            self.load_tools(cx);
        }
        // 技能页首次进入才拉（列表 + 使用统计）
        if view == View::Skills && self.skills.is_empty() {
            self.load_skills(cx);
        }
        // 定时任务页每次进入都刷新：agent 可能刚用 schedule_task 建过任务
        if view == View::Scheduled {
            self.load_sched_tasks(cx);
        }
        // 记忆页每次进入都刷新：演化任务会在会话结束后写入
        if view == View::Memory {
            self.load_memories(cx);
        }
        // 洞察页每次进入都刷新（用量/调度状态随时在变）
        if view == View::Insights {
            self.load_insights(cx);
        }
        // 审批页每次进入都刷新（挂起请求随时可能出现）
        if view == View::Approval {
            self.load_approval_page(cx);
        }
        // 知识页首次进入才拉根目录（后续导航自己发请求）
        if view == View::Knowledge && self.kb_entries.is_empty() {
            self.load_kb_entries(String::new(), cx);
        }
        cx.notify();
    }

    /// 输入卡片底部显示的当前 chat 模型（`模型 · provider`）。
    ///
    /// 口径与 Tauri 版 `ModelSelector` 一致：未选中时回退到 `preferences.chat`
    /// （`load_models` 已把它标成 `is_chat`），再没有就显示占位。
    fn current_model_label(&self) -> String {
        if let Some((provider, model)) = &self.session_model {
            return format!("{model} · {provider}");
        }
        match self.models.iter().find(|m| m.is_chat) {
            Some(m) => format!("{} · {}", m.name, m.provider),
            None => "未选择模型".to_string(),
        }
    }

    /// 拉取本会话的后台任务（`GET /api/v1/tasks`，按 `parent_session_id` 归属过滤）。
    ///
    /// Tauri 版靠「3s 轮询 + `task_status` 事件唤醒」双保险；这里先做轮询
    /// （事件唤醒在 `handle_event` 里补，见 `task_status` 分支）。
    /// 失败**静默保留旧值**——面板是观测面，不能因为一次网络抖动把已有列表清空。
    fn load_tasks(&self, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/tasks");
            if let Ok(body) = fetch_json(url).await {
                let arr = body.as_array().cloned().unwrap_or_default();
                // 全量任务一次拉回来：面板只显示当前会话的，左栏会话角标要按
                // `parent_session_id` 聚合**所有**会话（同一份轮询供两处用，
                // 与 Tauri 版把 `backgroundTasks` 放进 store 同口径）。
                let all: Vec<TaskRow> = arr
                    .iter()
                    .map(|t| TaskRow {
                        id: t
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        kind: t
                            .get("kind")
                            .and_then(|v| v.as_str())
                            .unwrap_or("-")
                            .to_string(),
                        status: t
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("-")
                            .to_string(),
                        description: t
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        parent_session_id: t
                            .get("parent_session_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        output_tail: t
                            .get("output_tail")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        error: t
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    })
                    .collect();
                let rows: Vec<TaskRow> = all
                    .iter()
                    .filter(|t| {
                        session
                            .as_deref()
                            .map(|s| s == t.parent_session_id)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect();
                let _ = this.update(cx, |this, cx| {
                    this.tasks = rows;
                    this.all_tasks = all;
                    this.rebuild_session_rows(cx);
                });
            }
        })
        .detach();
    }

    /// 会话后台任务停靠条（右侧 dock）。
    ///
    /// 对齐 Tauri 版 `AgentTasksPanel`：无任务时可折叠成竖条；活跃任务在上
    /// （顺序由后端返回决定）、终态保留展示。**取消按钮暂未接**（`POST /tasks/{id}/cancel`）。
    fn render_tasks_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.tasks_collapsed {
            return div()
                .id("tasks-expand")
                .w(px(36.))
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .py_2()
                .border_l_1()
                .border_color(rgb(colors::BORDER))
                .bg(rgb(colors::BG_SECONDARY))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.tasks_collapsed = false;
                    cx.notify();
                }))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_SECONDARY))
                        .child("任务"),
                );
        }
        let mut panel = div()
            .w(px(300.))
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(colors::BORDER))
            .bg(rgb(colors::BG_SECONDARY))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child(format!("后台任务 {}", self.tasks.len())),
                    )
                    .child(
                        div()
                            .id("tasks-collapse")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child("⟩")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.tasks_collapsed = true;
                                cx.notify();
                            })),
                    ),
            );
        if self.tasks.is_empty() {
            panel = panel.child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child("本会话暂无后台任务"),
            );
        }
        // 任务卡片（对齐 Tauri 版 `AgentTasksPanel`）：头 = 箭头 + 状态 + 类型徽章 +
        // 描述 + 状态文案 + 取消；展开体 = 终端输出尾部 / 子代理消息流。
        let mut cards: Vec<AnyElement> = Vec::new();
        for (ix, t) in self.tasks.iter().enumerate() {
            let open = self.expanded_task.as_deref() == Some(t.id.as_str());
            let terminal = matches!(
                t.status.as_str(),
                "completed" | "success" | "failed" | "cancelled" | "canceled"
            );
            let (glyph, color) = match t.status.as_str() {
                "running" | "pending" => ("●", colors::ACCENT),
                "completed" | "success" => ("✓", colors::SUCCESS),
                "failed" | "cancelled" | "canceled" => ("✗", colors::ERROR),
                _ => ("○", colors::TEXT_TERTIARY),
            };
            // 失败要带原因（截断），与 Tauri 版 `statusLabel` 同口径
            let status_label: String = match t.status.as_str() {
                "pending" => "等待中".to_string(),
                "running" => "运行中".to_string(),
                "completed" | "success" => "已完成".to_string(),
                "cancelled" | "canceled" => "已取消".to_string(),
                "failed" if !t.error.is_empty() => {
                    format!("失败（{}）", ellipsis(&t.error, 20))
                }
                "failed" => "失败".to_string(),
                other => other.to_string(),
            };
            let is_command = t.kind == "command";
            let kind_label = if is_command { "终端" } else { "委托" };
            let kind_color = if is_command {
                colors::TEXT_TERTIARY
            } else {
                colors::ACCENT
            };
            let chevron = if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            };
            let mut head = div()
                .id(("task-head", ix))
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
                        .text_color(rgb(colors::TEXT_TERTIARY)),
                )
                .child(div().text_xs().text_color(rgb(color)).child(glyph))
                .child(
                    div()
                        .px_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(kind_color))
                        .text_size(px(10.))
                        .text_color(rgb(kind_color))
                        .child(kind_label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_PRIMARY))
                        .child(ellipsis(&t.description, 26)),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(if t.status == "failed" {
                            colors::ERROR
                        } else {
                            colors::TEXT_TERTIARY
                        }))
                        .child(status_label),
                );
            // 非终态才给「取消」；点它不能顺带把卡片展开（stop_propagation）
            if !terminal {
                let cancel_id = t.id.clone();
                head = head.child(
                    div()
                        .id(("task-cancel", ix))
                        .px_1()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::BORDER))
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .cursor_pointer()
                        .child("取消")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.cancel_task(cancel_id.clone(), cx);
                        })),
                );
            }
            let open_id = t.id.clone();
            let head = head.on_click(cx.listener(move |this, _, _, cx| {
                let id = open_id.clone();
                if this.expanded_task.as_deref() == Some(id.as_str()) {
                    this.expanded_task = None;
                } else {
                    this.expanded_task = Some(id.clone());
                    // 委托任务的子代理消息按需拉一次并缓存（task_id 即子会话 id）
                    if this.task_messages.get(&id).is_none() {
                        this.load_task_messages(id.clone(), cx);
                    }
                }
                cx.notify();
            }));
            let mut card = div()
                .flex()
                .flex_col()
                .rounded_lg()
                .border_1()
                .border_color(rgb(if t.status == "failed" {
                    colors::ERROR
                } else {
                    colors::BORDER
                }))
                .bg(rgb(colors::BG_PRIMARY))
                .overflow_hidden()
                .child(head);
            if open {
                let mut body = div()
                    .border_t_1()
                    .border_color(rgb(colors::BORDER))
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1();
                // 失败任务：展开体顶部给完整原因（头部只放截断版）
                if t.status == "failed" && !t.error.is_empty() {
                    body = body.child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::ERROR))
                            .child(t.error.clone()),
                    );
                }
                if is_command {
                    let text = if t.output_tail.is_empty() {
                        "（暂无输出）".to_string()
                    } else {
                        t.output_tail.clone()
                    };
                    body = body.child(
                        div()
                            .max_h(px(200.))
                            .overflow_hidden()
                            .rounded_md()
                            .bg(rgb(0x0d1117))
                            .p_2()
                            .text_size(px(11.))
                            .text_color(rgb(0xc9d1d9))
                            .child(text),
                    );
                } else {
                    // 子代理活动流：紧凑单行投影（思考 / 输出 / 工具），
                    // 不是消息气泡 —— 与 Web 版 `DelegateActivity` 同构
                    body = body.child(self.render_delegate_activity(&t.id, !terminal, cx));
                }
                card = card.child(body);
            }
            cards.push(card.into_any_element());
        }
        panel
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .children(cards)
                    .overflow_y_scrollbar(),
            )
            .id("tasks-panel")
    }

    /// 取消一个后台任务（`POST /tasks/{id}/cancel`），随后刷新任务列表。
    ///
    /// 失败静默：任务可能已自行结束，下一次轮询会把真实状态带回来
    /// （与 Web 版「取消失败不弹错、等轮询」同口径）。
    fn cancel_task(&mut self, task_id: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/tasks/{task_id}/cancel");
            let _ = post_json(url, serde_json::json!({})).await;
            let _ = this.update(cx, |this, cx| this.load_tasks(cx));
        })
        .detach();
    }

    /// 拉子代理任务的消息（展开委托任务时用）。
    ///
    /// 子代理是**带父会话引用的独立会话**（ADR-030：`task_id` 即 `session_id`），
    /// 所以直接读它的消息接口即可，不需要另开一条流。
    fn load_task_messages(&self, task_id: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url =
                format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{task_id}/messages?limit=20");
            if let Ok(body) = fetch_json(url).await {
                let arr = body
                    .get("messages")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                // 与主对话流同一套结构化解析：子代理的活动流要靠 segments 区分
                // 思考 / 输出 / 工具，纯文本摊平后这些结构就没了（此前正是如此）
                let rows: Vec<MsgRow> = arr
                    .iter()
                    .map(|m| {
                        let (segs, tool_calls) = parse_segments(m);
                        MsgRow {
                            role: m
                                .get("role")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?")
                                .to_string(),
                            text: segments_text(m).into(),
                            segs,
                            tool_calls,
                            compression: false,
                        }
                    })
                    .collect();
                let _ = this.update(cx, |this, cx| {
                    this.task_messages.insert(task_id, rows);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 提交追问回答（`POST /api/v1/chat/answer`）。
    ///
    /// `answers` 是 `ask_user` 工具的返回值（直接回给模型）：
    /// `{ answers: [{question, answer}], extra }`——与 Tauri 版 `ClarificationBubble`
    /// 的序列化口径一致，模型侧看到的形态不因客户端不同而变。
    fn submit_clarification(&mut self, cx: &mut Context<Self>) {
        let Some(questions) = self.clarification.clone() else {
            return;
        };
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        let answers: Vec<serde_json::Value> = questions
            .iter()
            .enumerate()
            .map(|(i, (q, _))| {
                serde_json::json!({
                    "question": q,
                    "answer": self.clarify_answers.get(i).cloned().unwrap_or_default(),
                })
            })
            .collect();
        let port = self.port;
        self.status = "正在提交回答…".to_string();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/chat/answer");
            let body = serde_json::json!({
                "session_id": sid,
                "answers": { "answers": answers, "extra": "" },
            });
            let res = post_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.status = "回答已提交".to_string();
                        this.clarification = None;
                        this.clarify_answers.clear();
                    }
                    Err(e) => this.status = format!("提交回答失败：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 追问卡片（`ask_user` 挂起时显示在输入区上方）。
    ///
    /// 对齐 Tauri 版 `ClarificationBubble` 的**语义**（不是像素）：问题 + 选项，
    /// 全部答完才可提交。第一版只做「点选项作答」；无选项的问题给出提示让用户
    /// 用输入框里的原话补（避免为极少数场景先造一个多行富文本表单）。
    fn render_clarification(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(questions) = &self.clarification else {
            return div().into_any_element();
        };
        let all_answered = !questions.is_empty()
            && self
                .clarify_answers
                .iter()
                .take(questions.len())
                .all(|a| !a.trim().is_empty());
        let mut card = div().px_4().pb_2().child(
            div()
                .w_full()
                .max_w(px(896.))
                .mx_auto()
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors::ACCENT))
                .bg(rgb(colors::BG_SECONDARY))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(colors::ACCENT))
                        .child("模型在向你确认（回答后它继续执行）"),
                ),
        );
        for (qi, (question, options)) in questions.iter().enumerate() {
            let answered = self
                .clarify_answers
                .get(qi)
                .map(|a| !a.trim().is_empty())
                .unwrap_or(false);
            let q_title = if answered {
                format!("✓ {}. {}", qi + 1, question)
            } else {
                format!("{}. {}", qi + 1, question)
            };
            let mut q_block = div().flex().flex_col().gap_1().child(
                div()
                    .text_xs()
                    .text_color(if answered {
                        rgb(colors::SUCCESS)
                    } else {
                        rgb(colors::TEXT_PRIMARY)
                    })
                    .child(q_title),
            );
            if options.is_empty() {
                q_block = q_block.child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child("（该问题没有选项：把你输入框里写的原话作为回答）"),
                );
            }
            for (oi, option) in options.iter().enumerate() {
                let selected = self
                    .clarify_answers
                    .get(qi)
                    .map(|a| a == option)
                    .unwrap_or(false);
                let mut chip = div()
                    .id(("clarify", qi * 100 + oi))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER))
                    .cursor_pointer()
                    .text_xs();
                if selected {
                    chip = chip
                        .bg(rgb(colors::ACCENT))
                        .border_color(rgb(colors::ACCENT))
                        .text_color(rgb(0xffffff));
                } else {
                    chip = chip
                        .bg(rgb(colors::BG_HOVER))
                        .text_color(rgb(colors::TEXT_PRIMARY));
                }
                let answer = option.clone();
                q_block = q_block.child(chip.child(option.clone()).on_click(cx.listener(
                    move |this, _, _w, cx| {
                        if let Some(slot) = this.clarify_answers.get_mut(qi) {
                            *slot = answer.clone();
                        }
                        cx.notify();
                    },
                )));
            }
            card = card.child(q_block);
        }
        let mut submit = div()
            .id("clarify-submit")
            .px_3()
            .py_1()
            .rounded_md()
            .text_xs()
            .text_color(rgb(0xffffff));
        submit = if all_answered {
            submit.bg(rgb(colors::ACCENT)).cursor_pointer()
        } else {
            submit.bg(rgb(colors::BG_TERTIARY))
        };
        card.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child(if all_answered {
                            "已全部作答".to_string()
                        } else {
                            format!("还有 {} 个问题未回答", questions.len())
                        }),
                )
                .child(div().flex_1())
                .child(
                    submit
                        .child("提交回答")
                        .on_click(cx.listener(move |this, _, _w, cx| {
                            if this.clarify_answers.iter().all(|a| !a.trim().is_empty()) {
                                this.submit_clarification(cx);
                            }
                        })),
                ),
        )
        .into_any_element()
    }

    /// 拉取本会话的待审批操作（`GET /api/v1/approval/status`）。
    ///
    /// 只保留 `session_id` 匹配当前会话的项——审批是**会话内**的挂起操作，
    /// 别的会话的审批不该在本文出现（那会诱导用户给不相干的命令授权）。
    fn load_approvals(&self, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/approval/status");
            if let Ok(body) = fetch_json(url).await {
                let mut rows: Vec<ApprovalRow> = Vec::new();
                if let Some(arr) = body.get("pending_approvals").and_then(|v| v.as_array()) {
                    for a in arr {
                        let sid = a
                            .get("session_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        if session.as_deref() != Some(sid.as_str()) {
                            continue;
                        }
                        rows.push(ApprovalRow {
                            request_id: a
                                .get("request_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            description: a
                                .get("action_description")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            risk: a
                                .get("risk_level")
                                .and_then(|v| v.as_str())
                                .unwrap_or("-")
                                .to_string(),
                        });
                    }
                }
                let _ = this.update(cx, |this, cx| {
                    this.pending_approvals = rows;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 提交审批决策（`POST /api/v1/approval/respond`，decision = approve / deny）。
    fn respond_approval(
        &mut self,
        request_id: String,
        decision: &'static str,
        cx: &mut Context<Self>,
    ) {
        let port = self.port;
        self.status = format!("正在提交审批：{decision}");
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/approval/respond");
            let body = serde_json::json!({ "request_id": request_id, "decision": decision });
            let res = post_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                this.status = match res {
                    Ok(_) => format!("审批已提交（{decision}）"),
                    Err(e) => format!("提交审批失败：{e}"),
                };
                this.load_approvals(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 审批横幅（消息区顶部）：对齐 Tauri 版 `ApprovalBanner`（琥珀色告警 + 允许/拒绝）。
    ///
    /// 只显示**第一条**待审批：审批是一个个来的，堆一屏横幅既看不出关系也不知道点哪个；
    /// 处理完一条，下一条自然顶上（轮询刷新）。
    fn render_approval_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(a) = self.pending_approvals.first() else {
            return div().into_any_element();
        };
        let rid_approve = a.request_id.clone();
        let rid_deny = a.request_id.clone();
        div()
            .px_4()
            .pb_2()
            .child(
                div()
                    .w_full()
                    .max_w(px(896.))
                    .mx_auto()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::WARNING))
                    .bg(rgb(colors::BG_SECONDARY))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(colors::WARNING))
                                    .child("⚠ 操作等待授权"),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(colors::TEXT_TERTIARY))
                                    .child(format!("风险：{}", a.risk)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_PRIMARY))
                            .child(ellipsis(&a.description, 300)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(
                                div()
                                    .id("approve")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(colors::SUCCESS))
                                    .text_color(rgb(0xffffff))
                                    .cursor_pointer()
                                    .text_xs()
                                    .child("允许")
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.respond_approval(rid_approve.clone(), "approve", cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("deny")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(colors::ERROR))
                                    .text_color(rgb(0xffffff))
                                    .cursor_pointer()
                                    .text_xs()
                                    .child("拒绝")
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.respond_approval(rid_deny.clone(), "deny", cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 拉取会话待办与目标（`GET /todos` + `/goals`，均按 `session_id` 过滤）。
    ///
    /// 两者都是**会话绑定**的推理辅助（ADR-022：无全局计划页），所以跟着当前会话走、
    /// 切会话即重拉。失败静默（与任务面板同口径：观测面不因抖动清空）。
    fn load_todos_goals(&self, cx: &mut Context<Self>) {
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let base = format!("http://{SERVER_HOST}:{port}/api/v1");
            let todos_res = fetch_json(format!("{base}/todos?session_id={sid}")).await;
            let goals_res = fetch_json(format!("{base}/goals?session_id={sid}")).await;
            let mut todos: Vec<(String, String)> = Vec::new();
            if let Ok(body) = todos_res {
                if let Some(arr) = body.get("todos").and_then(|v| v.as_array()) {
                    for t in arr {
                        todos.push((
                            t.get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("pending")
                                .to_string(),
                            t.get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                        ));
                    }
                }
            }
            let mut goals: Vec<GoalRow> = Vec::new();
            if let Ok(body) = goals_res {
                if let Some(arr) = body.get("goals").and_then(|v| v.as_array()) {
                    for g in arr {
                        // 元素是 `{ goal: {...}, progress, todo_total, todo_done }`
                        let goal = g.get("goal").unwrap_or(g);
                        goals.push(GoalRow {
                            title: goal
                                .get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            status: goal
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("active")
                                .to_string(),
                            progress: g.get("progress").and_then(|v| v.as_u64()).unwrap_or(0),
                            todo_total: g.get("todo_total").and_then(|v| v.as_u64()).unwrap_or(0),
                            todo_done: g.get("todo_done").and_then(|v| v.as_u64()).unwrap_or(0),
                        });
                    }
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.todos = todos;
                this.goals = goals;
                cx.notify();
            });
        })
        .detach();
    }

    /// 会话待办 / 目标窄条（在输入卡片**上方**，限宽 896px 居中）。
    ///
    /// 对齐 Tauri 版 `SessionTodoPanel`：两者都空时**整条不渲染**（不占位）；
    /// 有内容时可折叠。待办状态用 `[ ]` / `[•]` / `[x]` 图标，已完成置灰。
    fn render_todo_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.todos.is_empty() && self.goals.is_empty() {
            return div().into_any_element();
        }
        let collapsed = self.todo_bar_collapsed;
        let mut bar = div().px_4().pb(px(6.)).child(
            div()
                .w_full()
                .max_w(px(896.))
                .mx_auto()
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors::BORDER))
                .bg(rgb(colors::BG_SECONDARY))
                .px_3()
                .py_2()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors::TEXT_SECONDARY))
                                .child(format!(
                                    "待办 {} · 目标 {}",
                                    self.todos.len(),
                                    self.goals.len()
                                )),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .id("todo-bar-toggle")
                                .px_1()
                                .cursor_pointer()
                                .text_xs()
                                .text_color(rgb(colors::TEXT_TERTIARY))
                                .child(if collapsed { "▾" } else { "▴" })
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.todo_bar_collapsed = !this.todo_bar_collapsed;
                                    cx.notify();
                                })),
                        ),
                ),
        );
        if collapsed {
            return bar.into_any_element();
        }
        for g in &self.goals {
            let (glyph, color) = match g.status.as_str() {
                "completed" => ("✓", colors::SUCCESS),
                "archived" => ("·", colors::TEXT_TERTIARY),
                _ => ("🎯", colors::ACCENT),
            };
            // 进度文案与 Tauri 版一致：有子待办才显示百分比，否则「进行中」
            //（`todo_total == 0` 时后端给的 progress 是 0，直接显示 0% 会误导）
            let progress_text = if g.todo_total > 0 {
                format!("{}%（{}/{}）", g.progress, g.todo_done, g.todo_total)
            } else {
                "进行中".to_string()
            };
            bar = bar.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .child(div().text_color(rgb(color)).child(glyph))
                    .child(
                        div()
                            .text_color(rgb(colors::TEXT_PRIMARY))
                            .child(ellipsis(&g.title, 40)),
                    )
                    .child(
                        div()
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(progress_text),
                    ),
            );
        }
        for (status, title) in &self.todos {
            let (glyph, color) = match status.as_str() {
                "completed" => ("[x]", colors::SUCCESS),
                "in_progress" => ("[•]", colors::ACCENT),
                _ => ("[ ]", colors::TEXT_TERTIARY),
            };
            let text_color = if status == "completed" {
                colors::TEXT_TERTIARY
            } else {
                colors::TEXT_PRIMARY
            };
            bar = bar.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .child(div().text_color(rgb(color)).child(glyph))
                    .child(div().text_color(rgb(text_color)).child(ellipsis(title, 48))),
            );
        }
        bar.into_any_element()
    }

    /// 上下文占用百分比（0 = 还没收到 usage 上报）
    fn ctx_pct(&self) -> u64 {
        if self.ctx_window == 0 {
            0
        } else {
            self.ctx_used.saturating_mul(100) / self.ctx_window
        }
    }

    /// 上下文占用文案。
    ///
    /// Tauri 版这块是一个环形进度（`ContextRing` + 库里的 `ProgressCircle`），
    /// 先落成同色的文本百分比；环状绘制随后替换（数值口径已就绪）。
    fn ctx_label(&self) -> String {
        if self.ctx_window == 0 {
            "上下文 --".to_string()
        } else {
            format!("上下文 {}%", self.ctx_pct())
        }
    }

    /// 选择模型：写**当前会话**的模型（`PUT /sessions/{id}/model`）。
    ///
    /// 与 Tauri 版 `ModelSelector.selectModel` 同语义——**只改会话，不改全局默认**
    /// （全局默认由设置页的 `POST /config/models/switch` 负责）。
    /// 未创建会话时先记在本地：首条消息的 `ChatRequest` 会带上 `model`/`provider`，
    /// 服务端建会话时即固化，不必等会话 id 生成后再补一次 PUT。
    fn select_chat_model(&mut self, provider: String, model: String, cx: &mut Context<Self>) {
        self.model_menu_open = false;
        self.session_model = Some((provider.clone(), model.clone()));
        let Some(sid) = self.current_session.clone() else {
            self.status = format!("新会话将使用 {model} · {provider}");
            cx.notify();
            return;
        };
        let port = self.port;
        self.status = format!("正在为会话切换模型到 {model}…");
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{sid}/model");
            let body = serde_json::json!({ "provider": provider, "model": model });
            let res = put_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                this.status = match res {
                    Ok(_) => format!("会话模型已切换：{model}"),
                    Err(e) => format!("设置会话模型失败：{e}"),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// 打开 provider 编辑器：把该 provider 的 endpoint / api_key 回填进输入框。
    ///
    /// 必须**同步**做（`set_value` 要 `&mut Window`），所以值取自随列表缓存的那份
    /// （见 [`ProviderRow`] 的注释：缓存只用于回填，不用于写）。
    fn open_provider_editor(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.providers.iter().find(|p| p.name == name).cloned() else {
            return;
        };
        self.edit_endpoint
            .update(cx, |s, cx| s.set_value(row.endpoint, window, cx));
        self.edit_api_key
            .update(cx, |s, cx| s.set_value(row.api_key, window, cx));
        self.editing_provider = Some(name);
        self.settings_status = "编辑 provider：改完点「保存」（会先重读配置再写）".to_string();
        self.settings_error = false;
        cx.notify();
    }

    /// 测试 provider 连通性（`POST /api/v1/config/test-connection`）。
    ///
    /// 用**当前编辑框里的** endpoint / api_key 测——这样用户可以在保存前先验证，
    /// 避免把错误的 key 写进配置再回滚。model 取该 provider 的第一个模型。
    fn test_provider_connection(&mut self, cx: &mut Context<Self>) {
        let endpoint = self.edit_endpoint.read(cx).value().to_string();
        let api_key = self.edit_api_key.read(cx).value().to_string();
        let model = self
            .editing_provider
            .as_ref()
            .and_then(|name| {
                self.models
                    .iter()
                    .find(|m| &m.provider == name && m.capabilities.contains("chat"))
                    .or_else(|| self.models.iter().find(|m| &m.provider == name))
                    .map(|m| m.name.clone())
            })
            .unwrap_or_default();
        if endpoint.trim().is_empty() || api_key.trim().is_empty() || model.is_empty() {
            self.settings_status = "测试连接需要 endpoint / api_key / 模型名都不为空".to_string();
            self.settings_error = true;
            cx.notify();
            return;
        }
        let port = self.port;
        self.settings_status = format!("正在测试 {model}…");
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/test-connection");
            let body = serde_json::json!({
                "endpoint": endpoint,
                "api_key": api_key,
                "model": model,
            });
            let res = post_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(v) => {
                        let ok = v.get("success").and_then(|b| b.as_bool()).unwrap_or(false);
                        let msg = v
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("")
                            .to_string();
                        this.settings_error = !ok;
                        this.settings_status = if ok {
                            format!("连接成功：{msg}")
                        } else {
                            format!("连接失败：{msg}")
                        };
                    }
                    Err(e) => {
                        this.settings_error = true;
                        this.settings_status = format!("测试失败：{e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 保存 provider 的 endpoint / api_key。
    ///
    /// ⚠️ `PUT /config` 是**整份替换**（`persist_and_reload`），不是按段合并
    /// （段保全只保「程序未建模的键」）。所以这里**先重新 GET 一份最新配置**，
    /// 只改目标 provider 的两个字段，再整体提交——把「读-改-写」的覆盖窗口压到
    /// 最短（一次往返），不要改成「用启动时缓存的配置提交」，那会冲掉期间别人
    /// 改过的建模字段（例如 agent 刚切的模型、MCP 预热注入的东西）。
    fn save_provider(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.editing_provider.clone() else {
            return;
        };
        let endpoint = self.edit_endpoint.read(cx).value().to_string();
        let api_key = self.edit_api_key.read(cx).value().to_string();
        let port = self.port;
        self.settings_status = format!("正在保存 {name}…");
        cx.spawn(async move |this, cx| {
            let base = format!("http://{SERVER_HOST}:{port}/api/v1");
            let mut cfg = match fetch_json(format!("{base}/config")).await {
                Ok(body) => body
                    .get("config")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings_status = format!("保存失败（读取配置）：{e}");
                        this.settings_error = true;
                        cx.notify();
                    });
                    return;
                }
            };
            let mut found = false;
            if let Some(arr) = cfg
                .get_mut("models")
                .and_then(|m| m.get_mut("providers"))
                .and_then(|p| p.as_array_mut())
            {
                for p in arr.iter_mut() {
                    if p.get("name").and_then(|v| v.as_str()) == Some(name.as_str()) {
                        p["endpoint"] = serde_json::Value::String(endpoint.clone());
                        p["api_key"] = serde_json::Value::String(api_key.clone());
                        found = true;
                    }
                }
            }
            if !found {
                let _ = this.update(cx, |this, cx| {
                    this.settings_status = format!("保存失败：配置里没有 provider {name}");
                    this.settings_error = true;
                    cx.notify();
                });
                return;
            }
            let res = put_json(
                format!("{base}/config"),
                serde_json::json!({ "config": cfg }),
            )
            .await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.settings_status = format!("{name} 已保存");
                        this.settings_error = false;
                        this.editing_provider = None;
                        this.load_models(cx);
                    }
                    Err(e) => {
                        this.settings_status = format!("保存失败：{e}");
                        this.settings_error = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 设置页（第一版）：模型列表 + 切换聊天模型。
    ///
    /// 只做「能日常用」的最小集合——没有它，换模型只能回 Tauri 版改配置。
    /// 存储 / 安全 / MCP / soul 等分组按后续批次补（端点都已存在，见
    /// `server/src/api/config/routes.rs`）。
    /// 子智能体页（对齐 `gui-vite/src/components/roles/RolesPanel.tsx`）：
    /// 左列表（搜索 + 角色项）+ 右详情（统计条 + 系统提示 / 工具 / 模型）。
    /// 文件页（对齐 Web 版 `WorkspacePanel` 的**只读部分**）：左侧单层目录树 +
    /// 右侧文件内容。
    ///
    /// 编辑保存（apply-patch）、diff 面板、目录绑定留阶段 2 —— 那三块各自要一套
    /// 状态机（草稿 / 冲突 / 快照索引），先把"能看"落地。
    fn render_workspace(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let crumb = if self.ws_path.is_empty() {
            if self.ws_root.is_empty() {
                "未选择会话（无可浏览目录）".to_string()
            } else {
                self.ws_root.clone()
            }
        } else {
            format!(
                "{}/{}",
                self.ws_root.trim_end_matches(['\\', '/']),
                self.ws_path
            )
        };

        let mut list = div().flex_1().min_h_0().w_full().flex().flex_col().gap_1();
        if !self.ws_path.is_empty() {
            let parent = {
                let p = self.ws_path.replace('\\', "/");
                match p.rfind('/') {
                    Some(pos) => p[..pos].to_string(),
                    None => String::new(),
                }
            };
            list = list.child(
                div()
                    .id("ws-up")
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_SECONDARY))
                    .child(
                        svg()
                            .path(IconName::CornerLeftUp.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY)),
                    )
                    .child("上级目录")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_workspace_tree(parent.clone(), cx);
                    })),
            );
        }
        for (ix, entry) in self.ws_entries.iter().enumerate() {
            let is_dir = entry.is_dir;
            let path = entry.path.clone();
            let name = entry.name.clone();
            let size = entry.size;
            list = list.child(
                div()
                    .id(("ws-entry", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .child(
                        svg()
                            .path(if is_dir {
                                IconName::Folder.path()
                            } else {
                                IconName::FileText.path()
                            })
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(rgb(if is_dir {
                                colors::TEXT_PRIMARY
                            } else {
                                colors::TEXT_SECONDARY
                            }))
                            .child(name),
                    )
                    .children(if is_dir {
                        None
                    } else {
                        Some(
                            div()
                                .flex_shrink_0()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY))
                                .child(format_size(size)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if is_dir {
                            this.load_workspace_tree(path.clone(), cx);
                        } else {
                            this.load_workspace_file(path.clone(), cx);
                        }
                    })),
            );
        }

        let content = match &self.ws_file {
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    svg()
                        .path(IconName::FileText.path())
                        .size(px(36.))
                        .text_color(rgb(colors::TEXT_TERTIARY)),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child("从左侧选择一个文件查看内容"),
                )
                .into_any_element(),
            Some((path, text)) => div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(colors::BORDER))
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(path.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .p_4()
                        .text_size(px(12.))
                        .text_color(rgb(colors::TEXT_SECONDARY))
                        .child(text.clone())
                        .overflow_y_scrollbar(),
                )
                .into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_6()
                    .py_4()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        svg()
                            .path(IconName::FolderTree.path())
                            .size(px(16.))
                            .text_color(rgb(colors::TEXT_SECONDARY)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(15.))
                            .text_color(rgb(colors::TEXT_PRIMARY))
                            .child("文件"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(crumb),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(self.ws_status.clone()),
                    )
                    // 选择工作目录：逐级浏览后绑定到当前会话
                    .child(
                        div()
                            .id("ws-pick-dir")
                            .flex_shrink_0()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .cursor_pointer()
                            .child(
                                svg()
                                    .path(IconName::FolderOpen.path())
                                    .size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY)),
                            )
                            .child("选择目录")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_workspace_picker(PickerTarget::Session, cx);
                            })),
                    )
                    // Diff 开关：右侧展开「工作区改动」面板（对照会话快照基线）
                    .child(
                        div()
                            .id("ws-diff-toggle")
                            .flex_shrink_0()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(if self.ws_diff_open {
                                colors::ACCENT
                            } else {
                                colors::BORDER
                            }))
                            .text_size(px(11.))
                            .text_color(rgb(if self.ws_diff_open {
                                colors::ACCENT
                            } else {
                                colors::TEXT_SECONDARY
                            }))
                            .cursor_pointer()
                            .child("Diff")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.ws_diff_open = !this.ws_diff_open;
                                if this.ws_diff_open {
                                    this.load_workspace_diff_list(cx);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .w(px(280.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(rgb(colors::BORDER))
                            .bg(rgb(colors::BG_SECONDARY))
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .border_b_1()
                                    .border_color(rgb(colors::BORDER))
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_SECONDARY))
                                    .child(if self.ws_path.is_empty() {
                                        "工作区根目录".to_string()
                                    } else {
                                        self.ws_path.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .p_2()
                                    .child(list.overflow_y_scrollbar()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .bg(rgb(colors::BG_PRIMARY))
                            .child(content),
                    )
                    // diff 面板（右侧，可开关）：AI 改了什么，一眼看到
                    .children(if self.ws_diff_open {
                        Some(self.render_diff_panel(cx))
                    } else {
                        None
                    }),
            )
    }
    /// diff 面板（右侧）：改动文件列表 + 选中文件的 unified diff（+/- 染色）。
    fn render_diff_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut files = div().flex().flex_col().gap_1().w_full();
        if self.ws_diff_files.is_empty() {
            files = files.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child(self.ws_diff_status.clone()),
            );
        }
        for (ix, f) in self.ws_diff_files.iter().enumerate() {
            let selected = self
                .ws_diff
                .as_ref()
                .map(|(p, _)| p == &f.path)
                .unwrap_or(false);
            let (color, label) = match f.status.as_str() {
                "added" => (colors::SUCCESS, "A"),
                "removed" => (colors::ERROR, "D"),
                "binary" => (colors::WARNING, "B"),
                _ => (colors::ACCENT, "M"),
            };
            let path = f.path.clone();
            files = files.child(
                div()
                    .id(("ws-diff-file", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .bg(rgb(if selected {
                        colors::BG_TERTIARY
                    } else {
                        colors::BG_SECONDARY
                    }))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(color))
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child(f.path.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!("+{} −{}", f.new_lines, f.old_lines)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_workspace_diff(path.clone(), cx);
                    })),
            );
        }

        let detail = match &self.ws_diff {
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child("选一个文件查看 diff"),
                )
                .into_any_element(),
            Some((_, unified)) => {
                let mut lines = div().flex().flex_col().w_full();
                for line in unified.lines().take(3000) {
                    let color = if line.starts_with("+++") || line.starts_with("---") {
                        colors::TEXT_TERTIARY
                    } else if line.starts_with('+') {
                        colors::SUCCESS
                    } else if line.starts_with('-') {
                        colors::ERROR
                    } else if line.starts_with("@@") {
                        colors::ACCENT
                    } else {
                        colors::TEXT_SECONDARY
                    };
                    lines = lines.child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(color))
                            .child(line.to_string()),
                    );
                }
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .p_2()
                    .child(lines.overflow_y_scrollbar())
                    .into_any_element()
            }
        };

        div()
            .w(px(380.))
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(colors::BORDER))
            .bg(rgb(colors::BG_SECONDARY))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER))
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_SECONDARY))
                    .child("工作区改动（相对会话起点快照）"),
            )
            // 列表必须自己可滚动：改动文件可能几十个，`max_h` 只限高不裁剪，
            // 内容会溢出盖到下面的 diff 区（实测二十多个文件时整片文字重叠）
            .child(
                div()
                    // 同 picker：滚动容器必须落在确定高度里（`max_h` 会让它塌成 0）
                    .h(px(240.))
                    .min_h_0()
                    .p_2()
                    .child(files.overflow_y_scrollbar()),
            )
            .child(detail)
            .into_any_element()
    }

    /// 记忆页：左列表（按目录分组）+ 右详情（L0/L1/L2 三层，L2 默认收起）。
    ///
    /// 不建完整树（Web 版 `buildMemoryTree`）：记忆条目本就按目录扁平分布，
    /// 一层分组标题 + 平铺足够定位，且省掉树节点的展开态管理。
    /// 知识页：三 tab（浏览 / 导入 / 搜索）。
    fn render_knowledge(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = div().flex().flex_row().items_center().gap_1();
        for (i, label) in ["浏览", "导入", "搜索"].iter().enumerate() {
            let idx = i as u8;
            let is_cur = self.kb_tab == idx;
            tabs = tabs.child(
                div()
                    .id(("kb-tab", i))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(if is_cur {
                        colors::ACCENT
                    } else {
                        colors::BORDER
                    }))
                    .bg(rgb(if is_cur {
                        colors::BG_TERTIARY
                    } else {
                        colors::BG_SECONDARY
                    }))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(rgb(if is_cur {
                        colors::ACCENT
                    } else {
                        colors::TEXT_SECONDARY
                    }))
                    .child(*label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.kb_tab = idx;
                        // 首次进导入 tab 才拉一次浏览目录（避免每次切都发请求）
                        if idx == 1 && this.kb_browse_entries.is_empty() {
                            this.load_kb_browse(String::new(), cx);
                        }
                        cx.notify();
                    })),
            );
        }

        let content = match self.kb_tab {
            1 => self.render_kb_ingest(cx).into_any_element(),
            2 => self.render_kb_search_tab(cx).into_any_element(),
            _ => self.render_kb_browse(cx).into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        svg()
                                            .path(IconName::BookOpen.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY))
                                            .child("知识库"),
                                    ),
                            )
                            .child(tabs),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(self.kb_status.clone()),
                    ),
            )
            .child(content)
    }

    /// 知识页·浏览：面包屑 + 条目列表 + 右内容（L0/L1/L2 切换）。
    fn render_kb_browse(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut crumbs = div().flex().flex_row().items_center().gap_1().child(
            div()
                .id("kb-root")
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_size(px(10.))
                .text_color(rgb(colors::TEXT_SECONDARY))
                .child("知识库根")
                .on_click(cx.listener(|this, _, _, cx| this.load_kb_entries(String::new(), cx))),
        );
        if !self.kb_path.is_empty() {
            let parent = self
                .kb_path
                .rsplit_once('/')
                .map(|(p, _)| p.to_string())
                .unwrap_or_default();
            crumbs = crumbs
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child(format!("/ {}", self.kb_path)),
                )
                .child(
                    div()
                        .id("kb-up")
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .text_size(px(10.))
                        .text_color(rgb(colors::ACCENT))
                        .child("↑ 上级")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.load_kb_entries(parent.clone(), cx)
                        })),
                );
        }

        let mut list = div().flex_1().min_h_0().p_2().flex().flex_col().gap_1();
        if self.kb_entries.is_empty() {
            list = list.child(
                div()
                    .py_6()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child("这个目录是空的（或尚未导入任何文档）"),
            );
        }
        for (ix, e) in self.kb_entries.iter().enumerate() {
            let is_sel = self.kb_selected.as_deref() == Some(e.uri.as_str());
            let uri = e.uri.clone();
            let is_dir = e.is_dir;
            let layers = format!(
                "{}{}{}",
                if e.has_abstract { "L0 " } else { "" },
                if e.has_overview { "L1 " } else { "" },
                if e.has_detail { "L2" } else { "" }
            );
            list = list.child(
                div()
                    .id(("kb", ix))
                    .w_full()
                    .px_2()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(if is_sel {
                        colors::BG_TERTIARY
                    } else {
                        colors::BG_SECONDARY
                    }))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .text_color(rgb(if is_sel {
                                        colors::ACCENT
                                    } else {
                                        colors::TEXT_PRIMARY
                                    }))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(format!(
                                        "{}{}",
                                        if is_dir { "[D] " } else { "" },
                                        e.name
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(9.))
                                    .text_color(rgb(colors::TEXT_TERTIARY))
                                    .child(layers),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if is_dir {
                            // 目录：uri → 相对知识根的路径（`tianyan://knowledge/a/b` → `a/b`）
                            let rel = uri
                                .split("knowledge/")
                                .nth(1)
                                .unwrap_or_default()
                                .to_string();
                            this.load_kb_entries(rel, cx);
                        } else {
                            this.kb_selected = Some(uri.clone());
                            this.read_kb_entry(uri.clone(), "detail".to_string(), cx);
                        }
                    })),
            );
        }

        let detail = match (&self.kb_selected, &self.kb_content) {
            (Some(uri), Some((level, text))) => {
                let uri_owned = uri.clone();
                let uri_del = uri.clone();
                let mut levels = div().flex().flex_row().items_center().gap_1();
                for (li, lv) in ["abstract", "overview", "detail"].iter().enumerate() {
                    let is_cur = level == lv;
                    let u = uri_owned.clone();
                    levels = levels.child(
                        div()
                            .id(("kb-lv", li))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(if is_cur {
                                colors::ACCENT
                            } else {
                                colors::BORDER
                            }))
                            .cursor_pointer()
                            .text_size(px(10.))
                            .text_color(rgb(if is_cur {
                                colors::ACCENT
                            } else {
                                colors::TEXT_SECONDARY
                            }))
                            .child(lv.to_string())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.read_kb_entry(u.clone(), lv.to_string(), cx)
                            })),
                    );
                }
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_SECONDARY))
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .child(uri_owned.clone()),
                                    )
                                    .child(
                                        div()
                                            .id("kb-del")
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .bg(rgb(colors::ERROR))
                                            .cursor_pointer()
                                            .text_size(px(10.))
                                            .text_color(rgb(colors::TEXT_PRIMARY))
                                            .child("删除")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.delete_kb_entry(uri_del.clone(), cx)
                                            })),
                                    ),
                            )
                            .child(levels),
                    )
                    .child(
                        div().flex_1().min_h_0().p_4().child(
                            div()
                                .w_full()
                                .child(TextView::markdown(
                                    ("kb-md", 0usize),
                                    SharedString::from(text.clone()),
                                ))
                                .overflow_y_scrollbar(),
                        ),
                    )
                    .into_any_element()
            }
            _ => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    svg()
                        .path(IconName::BookOpen.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY)),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child("选一个条目查看内容（目录点进去）"),
                )
                .into_any_element(),
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().px_4().py_2().child(crumbs))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .w(px(320.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(rgb(colors::BORDER))
                            .bg(rgb(colors::BG_SECONDARY))
                            .child(list.overflow_y_scrollbar()),
                    )
                    .child(div().flex_1().min_w_0().h_full().child(detail)),
            )
    }
    /// 知识页·导入：选文件（弹窗）或按路径批量，读本地文件 → multipart 上传。
    ///
    /// 文件选择走**弹窗**（`Dialog::KbFilePicker`）而不是常驻列表：导入页的主任务只有
    /// 「选文件 + 导入」，把一张文件列表铺在页面上会把主次关系拉平、也挤掉结果区。
    fn render_kb_ingest(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 已选摘要：弹窗关掉后仍要看得到「将要导入什么」
        let picked_line = if self.kb_picked.is_empty() {
            "还没有选中文件".to_string()
        } else {
            format!(
                "已选 {} 个：{}",
                self.kb_picked.len(),
                self.kb_picked
                    .iter()
                    .take(4)
                    .map(|p| std::path::Path::new(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.clone()))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };

        let mut results = div().flex().flex_col().gap_1().w_full();
        for (i, (name, status)) in self.kb_ingest_results.iter().enumerate() {
            results = results.child(
                div()
                    .id(("kb-ir", i))
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(status.clone()),
                    ),
            );
        }

        div()
            .flex_1()
            .min_h_0()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            // ① 主路径：选文件（弹窗）
            .child(
                div()
                    .p_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(colors::BORDER))
                    .bg(rgb(colors::BG_SECONDARY))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(
                                svg()
                                    .path(IconName::Upload.path())
                                    .size(px(20.))
                                    .text_color(rgb(colors::ACCENT)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(rgb(colors::TEXT_PRIMARY))
                                            .child("选择文件导入"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_TERTIARY))
                                            .child(
                                                "可多选；导入是异步任务，L0/L1 摘要生成前浏览里可能还看不到",
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .id("kb-open-picker")
                                    .px_4()
                                    .py_2()
                                    .rounded_md()
                                    .bg(rgb(colors::ACCENT))
                                    .cursor_pointer()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_PRIMARY))
                                    .child("选择文件…")
                                    .on_click(cx.listener(|this, _, _, cx| this.open_kb_picker(cx))),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(picked_line),
                    )
                    .child(field_wrap(&self.kb_ingest_category)),
            )
            // ② 次路径：按路径批量（目录递归）
            .child(
                div()
                    .p_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(colors::BORDER))
                    .bg(rgb(colors::BG_SECONDARY))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child("或按路径批量导入（文件或目录，目录递归，上限 200）"),
                    )
                    .child(field_wrap(&self.kb_ingest_path))
                    .child(
                        div()
                            .id("kb-ingest")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child("按路径导入")
                            .on_click(cx.listener(|this, _, _, cx| this.ingest_kb(cx))),
                    ),
            )
            // ③ 结果
            .child(results)
            .overflow_y_scrollbar()
    }

    fn render_kb_search_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut results = div().flex().flex_col().gap_2().w_full();
        if self.kb_results.is_empty() {
            results = results.child(
                div()
                    .py_6()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child("没有结果——输入词后点搜索"),
            );
        }
        for (i, r) in self.kb_results.iter().enumerate() {
            results = results.child(
                div()
                    .id(("kb-r", i))
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER))
                    .bg(rgb(colors::BG_SECONDARY))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(r.source.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::ACCENT))
                                    .child(format!("{:.3}", r.score)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child(ellipsis(&r.content.replace('\n', " "), 300)),
                    ),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(field_wrap(&self.kb_search)))
                    .child(
                        div()
                            .id("kb-do-search")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(colors::ACCENT))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_PRIMARY))
                            .child("搜索")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let q = this.kb_search.read(cx).value().to_string();
                                this.search_kb(q, cx);
                            })),
                    ),
            )
            .child(results.overflow_y_scrollbar())
    }

    /// 审批页：工作流配置 + 待审批（可操作）+ 待确认指纹 + 审计记录。
    ///
    /// 审批链路口径（ADR-033）：工具执行门控 → 风险分级 → 自动审批规则
    /// （Deny 优先）→ `ApprovalMode` 三态短路 → 人工审批挂起通道。
    /// 本页是这个链路的**观测面**：配置长什么样、当前挂起什么、过往怎么判的。
    fn render_approval(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().flex_1().min_h_0().p_4().flex().flex_col().gap_3();

        // 1. 配置卡
        let mut cfg = div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER))
            .bg(rgb(colors::BG_SECONDARY))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_PRIMARY))
                            .child("工作流配置"),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER))
                            .bg(rgb(colors::BG_TERTIARY))
                            .text_size(px(10.))
                            .text_color(rgb(colors::ACCENT))
                            .child(if self.approval_mode.is_empty() {
                                "未装配".to_string()
                            } else {
                                format!("模式 {}", self.approval_mode)
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(if self.approval_auto {
                                colors::SUCCESS
                            } else {
                                colors::TEXT_TERTIARY
                            }))
                            .child(if self.approval_auto {
                                "自动审批已启用"
                            } else {
                                "自动审批已关闭"
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!(
                                "超时 {}s · 最大挂起 {} · 已确认指纹 {}",
                                self.approval_timeout,
                                self.approval_max_pending,
                                self.confirmed_action_count
                            )),
                    ),
            );
        if !self.approval_prompt_cmds.is_empty() {
            let mut chips = div().flex().flex_row().items_center().gap_1().flex_wrap();
            for c in &self.approval_prompt_cmds {
                chips = chips.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::WARNING))
                        .text_size(px(10.))
                        .text_color(rgb(colors::WARNING))
                        .child(c.clone()),
                );
            }
            cfg = cfg.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child("总是询问的命令（不被自动规则与任何模式放行）"),
                    )
                    .child(chips),
            );
        }
        if !self.approval_rules.is_empty() {
            let mut rules = div().flex().flex_col().gap_1().w_full();
            for r in self.approval_rules.iter().take(12) {
                rules = rules.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_SECONDARY))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .child(r.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(if r.decision.eq_ignore_ascii_case("deny") {
                                    colors::ERROR
                                } else {
                                    colors::SUCCESS
                                }))
                                .child(r.decision.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY))
                                .child(if r.enabled { "启用" } else { "停用" }),
                        ),
                );
            }
            cfg = cfg.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!(
                                "自动审批规则（{} 条，Deny 优先）",
                                self.approval_rules.len()
                            )),
                    )
                    .child(rules),
            );
        }
        body = body.child(cfg);

        // 2. 待审批（可操作）
        let mut pending = div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(if self.pending_approvals.is_empty() {
                colors::BORDER
            } else {
                colors::WARNING
            }))
            .bg(rgb(colors::BG_SECONDARY))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_PRIMARY))
                    .child(format!("待审批（{}）", self.pending_approvals.len())),
            );
        if self.pending_approvals.is_empty() {
            pending = pending.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child("没有挂起的审批请求"),
            );
        }
        for (ix, a) in self.pending_approvals.iter().enumerate() {
            let rid_a = a.request_id.clone();
            let rid_d = a.request_id.clone();
            pending = pending.child(
                div()
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER))
                    .bg(rgb(colors::BG_PRIMARY))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_PRIMARY))
                                    .child(a.description.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(risk_color(&a.risk)))
                                    .child(format!("风险 {}", a.risk)),
                            ),
                    )
                    .child(
                        div()
                            .id(("ap-ok", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(colors::SUCCESS))
                            .cursor_pointer()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_PRIMARY))
                            .child("允许")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.respond_approval(rid_a.clone(), "approve", cx)
                            })),
                    )
                    .child(
                        div()
                            .id(("ap-no", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(colors::ERROR))
                            .cursor_pointer()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_PRIMARY))
                            .child("拒绝")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.respond_approval(rid_d.clone(), "deny", cx)
                            })),
                    ),
            );
        }
        body = body.child(pending);

        // 3. 待确认指纹（「询问用户」降级链路）
        if !self.pending_confirmations.is_empty() {
            let mut list = div().flex().flex_col().gap_1().w_full();
            for (ix, fp) in self.pending_confirmations.iter().enumerate() {
                list = list.child(
                    div()
                        .id(("ap-fp", ix))
                        .w_full()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_SECONDARY))
                        .child(fp.clone()),
                );
            }
            body = body.child(
                div()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER))
                    .bg(rgb(colors::BG_SECONDARY))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_PRIMARY))
                            .child(format!(
                                "待确认操作指纹（{}）",
                                self.pending_confirmations.len()
                            )),
                    )
                    .child(list),
            );
        }

        // 4. 审计记录
        let mut hist = div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER))
            .bg(rgb(colors::BG_SECONDARY))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_PRIMARY))
                    .child(format!("审计记录（{}）", self.approval_history.len())),
            );
        if self.approval_history.is_empty() {
            hist = hist.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child("暂无审批记录"),
            );
        }
        for h in self.approval_history.iter().take(20) {
            hist = hist.child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(risk_color(&h.risk)))
                            .child(h.risk.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(h.description.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(if h.decision.eq_ignore_ascii_case("deny") {
                                colors::ERROR
                            } else if h.decision.eq_ignore_ascii_case("approve") {
                                colors::SUCCESS
                            } else {
                                colors::TEXT_TERTIARY
                            }))
                            .child(h.decision.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(match h.result {
                                Some(true) => colors::SUCCESS,
                                Some(false) => colors::ERROR,
                                None => colors::TEXT_TERTIARY,
                            }))
                            .child(match h.result {
                                Some(true) => "执行成功".to_string(),
                                Some(false) => "执行失败".to_string(),
                                None => "未执行".to_string(),
                            }),
                    ),
            );
        }
        body = body.child(hist);

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        svg()
                                            .path(IconName::Shield.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY))
                                            .child("审批"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY))
                                    .child(self.approvals_page_status.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("ap-reload")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child("刷新")
                            .on_click(cx.listener(|this, _, _, cx| this.load_approval_page(cx))),
                    ),
            )
            .child(body.overflow_y_scrollbar())
    }

    /// 洞察页：总览四卡 + 模型用量（含分组）+ 调度器任务表。
    fn render_insights(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.insight_stats.clone().unwrap_or_default();
        let mut body = div().flex_1().min_h_0().p_4().flex().flex_col().gap_3();

        body = body.child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .flex_wrap()
                .child(stat_card(
                    "技能",
                    format!("{}", s.total_skills_tracked),
                    format!("累计调用 {} 次", group_thousands(s.total_skill_calls)),
                ))
                .child(stat_card(
                    "工具",
                    format!("{}", s.total_tools_tracked),
                    format!("累计调用 {} 次", group_thousands(s.total_tool_calls)),
                ))
                .child(stat_card(
                    "文档",
                    format!("{}", s.total_docs_tracked),
                    "VFS 访问过的文档".to_string(),
                ))
                .child(stat_card(
                    "搜索",
                    format!("{}", s.total_searches),
                    "检索次数".to_string(),
                )),
        );

        if let Some(t) = &self.usage_total {
            let mut usage = div()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors::BORDER))
                .bg(rgb(colors::BG_SECONDARY))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .flex_wrap()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(colors::TEXT_PRIMARY))
                                .child(format!("近 {} 天模型用量", self.usage_days)),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY))
                                .child(format!(
                                    "{} 次调用 · 总 token {} · 缓存命中 {:.1}%",
                                    group_thousands(t.calls),
                                    group_thousands(t.total_tokens),
                                    t.cache_hit_rate * 100.0
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .flex_wrap()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child(format!("未命中输入 {}", group_thousands(t.uncached_input)))
                        .child(format!("缓存命中输入 {}", group_thousands(t.cached_input)))
                        .child(format!("输出 {}", group_thousands(t.completion_tokens))),
                );
            if !self.usage_grouped.is_empty() {
                let mut rows = div().flex().flex_col().gap_1().w_full();
                for g in self.usage_grouped.iter().take(10) {
                    rows = rows.child(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_SECONDARY))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(g.group.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY))
                                    .child(format!(
                                        "{} 次 · {} tok · 命中 {:.0}%",
                                        group_thousands(g.calls),
                                        group_thousands(g.total_tokens),
                                        g.cache_hit_rate * 100.0
                                    )),
                            ),
                    );
                }
                usage = usage.child(rows);
            }
            body = body.child(usage);
        }

        let mut sched = div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER))
            .bg(rgb(colors::BG_SECONDARY))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_PRIMARY))
                            .child("调度器"),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(if self.scheduler_running {
                                colors::SUCCESS
                            } else {
                                colors::TEXT_TERTIARY
                            }))
                            .child(if self.scheduler_running {
                                "运行中"
                            } else {
                                "未装配"
                            }),
                    ),
            );
        if let Some(id) = &self.scheduler_executing {
            sched = sched.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(colors::ACCENT))
                    .child(format!("正在执行 {id}")),
            );
        }
        if self.sched_status_tasks.is_empty() {
            sched = sched.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child("没有已注册的定时任务"),
            );
        }
        for t in &self.sched_status_tasks {
            sched = sched.child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(t.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!(
                                "{} · 跑 {} 次 · 上次 {}前",
                                format_interval(t.interval_secs),
                                t.run_count,
                                format_secs_short(t.last_run_ago_secs)
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!("下次 {}后", format_secs_short(t.next_due_in_secs))),
                    ),
            );
        }
        body = body.child(sched);

        // 天数切换 + 刷新
        let mut days_bar = div().flex().flex_row().items_center().gap_1();
        for d in [1i64, 3, 7, 30] {
            let is_cur = self.usage_days == d;
            days_bar = days_bar.child(
                div()
                    .id(("ins-days", d as usize))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(if is_cur {
                        colors::ACCENT
                    } else {
                        colors::BORDER
                    }))
                    .bg(rgb(if is_cur {
                        colors::BG_TERTIARY
                    } else {
                        colors::BG_SECONDARY
                    }))
                    .cursor_pointer()
                    .text_size(px(10.))
                    .text_color(rgb(if is_cur {
                        colors::ACCENT
                    } else {
                        colors::TEXT_SECONDARY
                    }))
                    .child(format!("{d} 天"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.usage_days = d;
                        this.load_insights(cx);
                    })),
            );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        svg()
                                            .path(IconName::ChartLine.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY))
                                            .child("洞察"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY))
                                    .child(self.insights_status.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(days_bar)
                            .child(
                                div()
                                    .id("ins-reload")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER))
                                    .cursor_pointer()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_SECONDARY))
                                    .child("刷新")
                                    .on_click(cx.listener(|this, _, _, cx| this.load_insights(cx))),
                            ),
                    ),
            )
            .child(body.overflow_y_scrollbar())
    }

    fn render_memory(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 按目录分组（relative_path 的父段；根级归到「(根)」）
        let mut groups: Vec<(String, Vec<&MemoryRow>)> = Vec::new();
        for m in &self.memories {
            let key = m
                .relative_path
                .rsplit_once('/')
                .map(|(d, _)| d.to_string())
                .unwrap_or_else(|| "(根)".to_string());
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => v.push(m),
                None => groups.push((key, vec![m])),
            }
        }
        let selected = self
            .memories
            .iter()
            .find(|m| self.memory_selected.as_deref() == Some(m.uri.as_str()))
            .cloned();

        // 左：分组列表
        let mut list = div().flex_1().min_h_0().p_2().flex().flex_col().gap_2();
        if self.memories.is_empty() {
            list = list.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path(IconName::Brain.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY)),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child("暂无记忆——自演化综述任务会在会话结束后写入"),
                    ),
            );
        }
        for (gi, (dir, items)) in groups.iter().enumerate() {
            let mut section = div().flex().flex_col().gap_1().child(
                div()
                    .px_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(dir.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!("· {}", items.len())),
                    ),
            );
            for (ix, m) in items.iter().enumerate() {
                let is_sel = self.memory_selected.as_deref() == Some(m.uri.as_str());
                let uri = m.uri.clone();
                // 重要度三档配色：≥0.7 绿（稳固）/ ≥0.4 次级 / 其余红（待巩固）
                let imp_color = if m.importance >= 0.7 {
                    colors::SUCCESS
                } else if m.importance >= 0.4 {
                    colors::TEXT_SECONDARY
                } else {
                    colors::ERROR
                };
                section = section.child(
                    div()
                        .id(("mem", gi * 1000 + ix))
                        .w_full()
                        .px_2()
                        .py_2()
                        .rounded_lg()
                        .bg(rgb(if is_sel {
                            colors::BG_TERTIARY
                        } else {
                            colors::BG_SECONDARY
                        }))
                        .cursor_pointer()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(px(12.))
                                        .text_color(rgb(if is_sel {
                                            colors::ACCENT
                                        } else {
                                            colors::TEXT_PRIMARY
                                        }))
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .child(m.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(rgb(imp_color))
                                        .child(format!("{:.2}", m.importance)),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .child(
                                    m.abstract_text
                                        .as_ref()
                                        .map(|t| ellipsis(t, 56))
                                        .unwrap_or_else(|| "（无摘要）".to_string()),
                                ),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.memory_selected = Some(uri.clone());
                            this.memory_detail_open = false;
                            cx.notify();
                        })),
                );
            }
            list = list.child(section);
        }

        // 右：详情（L0 → L1 → L2，L2 折叠）
        let detail = match &selected {
            None => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    svg()
                        .path(IconName::Brain.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY)),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child("选择一条记忆查看三层内容"),
                )
                .into_any_element(),
            Some(m) => {
                let mut head = div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgb(colors::TEXT_PRIMARY))
                            .child(m.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(m.relative_path.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .flex_wrap()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!("重要度 {:.2}", m.importance))
                            .child(if m.updated_at.is_empty() {
                                String::new()
                            } else {
                                format!("更新于 {}", short_stamp(&m.updated_at))
                            })
                            .child(if m.tags.is_empty() {
                                String::new()
                            } else {
                                format!("#{}", m.tags.join(" #"))
                            }),
                    );
                let mut body = div().flex_1().min_h_0().p_4().flex().flex_col().gap_3();
                if let Some(t) = &m.abstract_text {
                    body = body.child(memory_block("L0 摘要", t.clone()));
                }
                if let Some(t) = &m.overview_text {
                    body = body.child(memory_block("L1 概览", t.clone()));
                }
                if let Some(t) = &m.detail_text {
                    let open = self.memory_detail_open;
                    let label = if open {
                        "▾ L2 全文（收起）".to_string()
                    } else {
                        format!("▸ L2 全文（{} 字，点击展开）", t.chars().count())
                    };
                    body = body.child(
                        div()
                            .id("mem-detail-toggle")
                            .cursor_pointer()
                            .text_size(px(10.))
                            .text_color(rgb(colors::ACCENT))
                            .child(label)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.memory_detail_open = !this.memory_detail_open;
                                cx.notify();
                            })),
                    );
                    if open {
                        body = body.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_SECONDARY))
                                .child(t.clone()),
                        );
                    }
                }
                head = head.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child(m.uri.clone()),
                );
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(head)
                    .child(body.overflow_y_scrollbar())
                    .into_any_element()
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        svg()
                                            .path(IconName::Brain.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY))
                                            .child("记忆"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY))
                                    .child(self.memories_status.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("mem-reload")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child("刷新")
                            .on_click(cx.listener(|this, _, _, cx| this.load_memories(cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .w(px(320.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(rgb(colors::BORDER))
                            .bg(rgb(colors::BG_SECONDARY))
                            .child(list.overflow_y_scrollbar()),
                    )
                    .child(div().flex_1().min_w_0().h_full().child(detail)),
            )
    }

    /// 定时任务页：列表卡片 + 新建表单（间隔预设 / 自定义秒 / 工作区 / 指令）。
    ///
    /// 与 Web 版 `ScheduledTasksSection` 同口径：间隔预设按钮组而非下拉
    /// （GPUI 没有原生 select；按钮行反而更好点），自定义走秒数输入。
    fn render_scheduled(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let head = div()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(colors::BORDER))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                svg()
                                    .path(IconName::CalendarClock.path())
                                    .size(px(16.))
                                    .text_color(rgb(colors::TEXT_SECONDARY)),
                            )
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .text_color(rgb(colors::TEXT_PRIMARY))
                                    .child("定时任务"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(
                                "按执行间隔调用智能体在指定工作区工作；\
                                 服务停机期间超期的任务重启后补跑一次",
                            ),
                    ),
            )
            .child(
                div()
                    .id("sched-toggle")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_SECONDARY))
                    .child(if self.sched_form_open {
                        "取消"
                    } else {
                        "＋ 新建定时任务"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sched_form_open = !this.sched_form_open;
                        cx.notify();
                    })),
            );

        let mut body = div().flex_1().min_h_0().p_4().flex().flex_col().gap_3();

        // 状态行（创建/删除结果；空串不占位）
        if !self.sched_status.is_empty() {
            body = body.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child(self.sched_status.clone()),
            );
        }

        // 新建表单
        if self.sched_form_open {
            let mut presets = div().flex().flex_row().items_center().gap_1().flex_wrap();
            for (secs, label) in SCHED_PRESETS {
                let is_cur = self.sched_preset == secs;
                presets = presets.child(
                    div()
                        .id(("sched-preset", secs))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(if is_cur {
                            colors::ACCENT
                        } else {
                            colors::BORDER
                        }))
                        .bg(rgb(if is_cur {
                            colors::BG_TERTIARY
                        } else {
                            colors::BG_SECONDARY
                        }))
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(rgb(if is_cur {
                            colors::ACCENT
                        } else {
                            colors::TEXT_SECONDARY
                        }))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sched_preset = secs;
                            cx.notify();
                        })),
                );
            }
            let mut form = div()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors::BORDER))
                .bg(rgb(colors::BG_SECONDARY))
                .flex()
                .flex_col()
                .gap_2()
                .child(field_wrap(&self.sched_name))
                .child(presets);
            if self.sched_preset == 0 {
                form = form.child(field_wrap(&self.sched_custom_secs));
            }
            form = form
                .child(field_wrap(&self.sched_workspace))
                .child(field_wrap_area(&self.sched_prompt))
                .child(
                    div()
                        .id("sched-create")
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(colors::ACCENT))
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_PRIMARY))
                        .child("创建")
                        .on_click(cx.listener(|this, _, _, cx| this.create_sched_task(cx))),
                );
            body = body.child(form);
        }

        // 列表
        if self.sched_tasks.is_empty() {
            body = body.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path(IconName::CalendarClock.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY)),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(
                                "暂无定时任务——可让智能体用 schedule_task 工具创建，或在此手动创建",
                            ),
                    ),
            );
        } else {
            let mut list = div().flex().flex_col().gap_2().w_full();
            for (ix, t) in self.sched_tasks.iter().enumerate() {
                let id = t.id.clone();
                let mut card = div()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER))
                    .bg(rgb(colors::BG_SECONDARY))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(rgb(colors::TEXT_PRIMARY))
                                            .child(t.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(rgb(colors::BORDER))
                                            .bg(rgb(colors::BG_TERTIARY))
                                            .text_size(px(10.))
                                            .text_color(rgb(colors::ACCENT))
                                            .child(format_interval(t.interval_secs)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY))
                                    .child(format!(
                                        "工作区: {}",
                                        if t.workspace.is_empty() {
                                            "(默认)"
                                        } else {
                                            &t.workspace
                                        }
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_SECONDARY))
                                    .child(ellipsis(&t.prompt, 140)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY))
                                    .child(format!(
                                        "下次(约): {}",
                                        t.next_run_at
                                            .as_deref()
                                            .and_then(parse_rfc3339_secs)
                                            .map(relative_from_now)
                                            .unwrap_or_else(|| "待定".to_string())
                                    ))
                                    .child(
                                        t.last_run_at
                                            .as_deref()
                                            .and_then(parse_rfc3339_secs)
                                            .map(|ts| format!("上次: {}", relative_from_now(ts)))
                                            .unwrap_or_default(),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id(("sched-del", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child("删除")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_sched_task(id.clone(), cx)
                            })),
                    );
                if let Some(res) = t.last_result.as_ref().filter(|r| !r.is_empty()) {
                    card = card.child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!("结果: {}", ellipsis(res, 120))),
                    );
                }
                list = list.child(card);
            }
            body = body.child(list);
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(body.overflow_y_scrollbar())
    }

    /// 技能页：顶部采纳统计 + 左列表（搜索）+ 右详情（markdown 正文）。
    ///
    /// 与 Web 版 `SkillsPanel` 同口径：技能是方法论文档，只读；不做「运行」按钮
    /// （执行语义归工具面板与对话里的工具卡）。
    fn render_skills(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.skills_query.trim().to_lowercase();
        let visible: Vec<SkillRow> = self
            .skills
            .iter()
            .filter(|s| {
                query.is_empty()
                    || s.name.to_lowercase().contains(&query)
                    || s.description.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        let selected = self
            .skills
            .iter()
            .find(|s| self.skills_selected.as_deref() == Some(s.id.as_str()))
            .cloned();

        // 顶部：累计调用 + 采纳排行（方法论技能无「执行成败」语义，只看频率）
        let mut stats_bar = div()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(colors::BORDER))
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .flex_wrap();
        if self.skills_total_calls > 0 {
            stats_bar = stats_bar.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_SECONDARY))
                    .child(format!(
                        "技能累计调用 {} 次",
                        group_thousands(self.skills_total_calls)
                    )),
            );
            let mut chips = div().flex().flex_row().items_center().gap_1().flex_wrap();
            for st in self.skill_stats.iter().take(8) {
                chips = chips.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::BORDER))
                        .bg(rgb(colors::BG_SECONDARY))
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_SECONDARY))
                        .child(format!("{} {} 次", st.skill_id, st.total_calls)),
                );
            }
            stats_bar = stats_bar.child(chips);
        } else {
            stats_bar = stats_bar.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child("暂无技能调用统计——智能体调用技能后，这里会展示采纳排行与平均耗时"),
            );
        }

        // 左：技能列表
        let mut list = div().flex_1().min_h_0().p_2().flex().flex_col().gap_1();
        if visible.is_empty() {
            list = list.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path(IconName::Lightbulb.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY)),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(if self.skills.is_empty() {
                                "暂无学习方法论技能"
                            } else {
                                "未找到匹配技能"
                            }),
                    ),
            );
        }
        for (ix, skill) in visible.iter().enumerate() {
            let is_sel = self.skills_selected.as_deref() == Some(skill.id.as_str());
            let id = skill.id.clone();
            let stat = self.skill_stats.iter().find(|s| s.skill_id == skill.id);
            let mut row = div()
                .id(("skill", ix))
                .w_full()
                .px_2()
                .py_2()
                .rounded_lg()
                .bg(rgb(if is_sel {
                    colors::BG_TERTIARY
                } else {
                    colors::BG_SECONDARY
                }))
                .cursor_pointer()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(if is_sel {
                            colors::ACCENT
                        } else {
                            colors::TEXT_PRIMARY
                        }))
                        .child(skill.name.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(ellipsis(&skill.description, 52)),
                );
            if let Some(st) = stat.filter(|st| st.total_calls > 0) {
                let mut line = format!(
                    "调用 {} 次 · 均值 {}ms",
                    st.total_calls,
                    st.avg_time_ms.round() as u64
                );
                if st.total_calls > st.success_calls {
                    line.push_str(&format!(
                        " · 执行异常 {} 次",
                        st.total_calls - st.success_calls
                    ));
                }
                row = row.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child(line),
                );
            }
            if let Some(t) = &skill.updated_at {
                row = row.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child(format!("更新于 {}", short_stamp(t))),
                );
            }
            list = list.child(row.on_click(cx.listener(move |this, _, _, cx| {
                this.skills_selected = Some(id.clone());
                this.skill_content = None;
                this.skills_status = "正在加载技能内容…".to_string();
                this.load_skill_detail(id.clone(), cx);
                cx.notify();
            })));
        }

        // 右：详情
        let detail = match &selected {
            None => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    svg()
                        .path(IconName::Lightbulb.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY)),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child("选择一个技能查看详情"),
                )
                .into_any_element(),
            Some(skill) => {
                let content = self.skill_content.clone().unwrap_or_default();
                let mut chips = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER))
                            .bg(rgb(colors::BG_SECONDARY))
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!("v{}", skill.version)),
                    );
                if let Some(t) = &skill.updated_at {
                    chips = chips.child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!("更新于 {}", short_stamp(t))),
                    );
                }
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .text_color(rgb(colors::TEXT_PRIMARY))
                                    .child(skill.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY))
                                    .child(skill.description.clone()),
                            )
                            .child(chips),
                    )
                    .child(div().flex_1().min_h_0().p_4().child(if content.is_empty() {
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(
                                if self.skills_status.starts_with("加载技能详情失败")
                                    || self.skills_status.starts_with("正在加载")
                                {
                                    self.skills_status.clone()
                                } else {
                                    "该技能没有可查看的内容".to_string()
                                },
                            )
                            .into_any_element()
                    } else {
                        div()
                            .w_full()
                            .child(TextView::markdown(
                                ("skill-md", 0usize),
                                SharedString::from(content),
                            ))
                            .overflow_y_scrollbar()
                            .into_any_element()
                    }))
                    .into_any_element()
            }
        };

        div().size_full().flex().flex_col().child(stats_bar).child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_row()
                .child(
                    div()
                        .w(px(colors::LIST_WIDTH))
                        .h_full()
                        .flex()
                        .flex_col()
                        .border_r_1()
                        .border_color(rgb(colors::BORDER))
                        .bg(rgb(colors::BG_SECONDARY))
                        .child(
                            div()
                                .px_4()
                                .py_3()
                                .border_b_1()
                                .border_color(rgb(colors::BORDER))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            svg()
                                                .path(IconName::Lightbulb.path())
                                                .size(px(16.))
                                                .text_color(rgb(colors::TEXT_SECONDARY)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(15.))
                                                .text_color(rgb(colors::TEXT_PRIMARY))
                                                .child("技能"),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(rgb(colors::TEXT_TERTIARY))
                                        .child("自动学习方法论（GEPA 总结），只读查看"),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(rgb(colors::BORDER))
                                        .bg(rgb(colors::BG_PRIMARY))
                                        .px_2()
                                        .py_1()
                                        .child(Input::new(&self.skills_search).appearance(false)),
                                ),
                        )
                        .child(list.overflow_y_scrollbar()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(detail),
                ),
        )
    }

    /// 工具页：左列表（搜索 + 名称/描述）+ 右详情（参数 JSON Schema）。
    ///
    /// 与 Web 版 `ToolsPanel` 同口径：工具是**原子操作**，与「技能」（方法论）两套概念。
    /// 参数不做表单（工具是给 LLM 调用的，人只读 schema）。
    fn render_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.tools_query.trim().to_lowercase();
        let visible: Vec<ToolRow> = self
            .tools
            .iter()
            .filter(|t| {
                query.is_empty()
                    || t.name.to_lowercase().contains(&query)
                    || t.description.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        let selected = self
            .tools
            .iter()
            .find(|t| self.tools_selected.as_deref() == Some(t.name.as_str()))
            .cloned();

        let mut list = div().flex_1().min_h_0().p_2().flex().flex_col().gap_1();
        if visible.is_empty() {
            list = list.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path(IconName::Wrench.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY)),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(if self.tools.is_empty() {
                                "暂无可用工具"
                            } else {
                                "未找到匹配工具"
                            }),
                    ),
            );
        }
        for (ix, tool) in visible.iter().enumerate() {
            let is_sel = self.tools_selected.as_deref() == Some(tool.name.as_str());
            let name = tool.name.clone();
            list = list.child(
                div()
                    .id(("tool", ix))
                    .w_full()
                    .px_2()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(if is_sel {
                        colors::BG_TERTIARY
                    } else {
                        colors::BG_SECONDARY
                    }))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(if is_sel {
                                colors::ACCENT
                            } else {
                                colors::TEXT_PRIMARY
                            }))
                            .child(tool.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(ellipsis(&tool.description, 56)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tools_selected = Some(name.clone());
                        cx.notify();
                    })),
            );
        }

        let detail = match &selected {
            None => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    svg()
                        .path(IconName::Wrench.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY)),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child("选择一个工具查看详情"),
                )
                .into_any_element(),
            Some(tool) => {
                let schema = serde_json::to_string_pretty(&tool.parameters)
                    .unwrap_or_else(|_| "（参数无法序列化）".to_string());
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .text_color(rgb(colors::TEXT_PRIMARY))
                                    .child(tool.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY))
                                    .child(tool.description.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY))
                                    .child("参数 Schema"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER))
                                    .bg(rgb(colors::BG_SECONDARY))
                                    .p_3()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_SECONDARY))
                                            .child(schema)
                                            .overflow_y_scrollbar(),
                                    ),
                            ),
                    )
                    .into_any_element()
            }
        };

        div()
            .size_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .w(px(colors::LIST_WIDTH))
                    .h_full()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(rgb(colors::BORDER))
                    .bg(rgb(colors::BG_SECONDARY))
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        svg()
                                            .path(IconName::Wrench.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY))
                                            .child("工具"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY))
                                    .child(format!(
                                        "系统内置原子操作（{} 个），LLM 通过 tool call 调用",
                                        self.tools.len()
                                    )),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER))
                                    .bg(rgb(colors::BG_PRIMARY))
                                    .px_2()
                                    .py_1()
                                    .child(Input::new(&self.tools_search).appearance(false)),
                            ),
                    )
                    .child(list.overflow_y_scrollbar()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(detail),
            )
    }

    fn render_roles(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.roles_query.trim().to_lowercase();
        let visible: Vec<RoleRow> = self
            .roles
            .iter()
            .filter(|r| {
                query.is_empty()
                    || r.name.to_lowercase().contains(&query)
                    || r.purpose.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();

        // 左：角色列表
        let mut list = div().flex_1().min_h_0().p_2().flex().flex_col().gap_1();
        if visible.is_empty() {
            list = list.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path(IconName::Bot.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY)),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(if self.roles.is_empty() {
                                "暂无角色"
                            } else {
                                "未找到匹配子智能体"
                            }),
                    ),
            );
        } else {
            for (ix, role) in visible.iter().enumerate() {
                list = list.child(self.render_role_item(ix, role, cx));
            }
        }

        // 右：统计条 + 详情
        let mut stats_bar = div()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(colors::BORDER))
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .flex_wrap()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .child(
                        div()
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child("累计委托"),
                    )
                    .child(
                        div()
                            .text_color(rgb(colors::TEXT_PRIMARY))
                            .child(format!("{}", self.roles_stats.total_calls)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .child(
                        div()
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child("成功率"),
                    )
                    .child(
                        div()
                            .text_color(rgb(if self.roles_stats.success_rate < 0.5 {
                                colors::ERROR
                            } else {
                                colors::SUCCESS
                            }))
                            .child(format!(
                                "{}%",
                                (self.roles_stats.success_rate * 100.0).round() as u32
                            )),
                    ),
            );
        if !self.roles_stats.by_task_type.is_empty() {
            let mut chips = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_wrap()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child("任务类型："),
                );
            for (kind, count) in &self.roles_stats.by_task_type {
                chips = chips.child(
                    div()
                        .px_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::BORDER))
                        .bg(rgb(colors::BG_SECONDARY))
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_SECONDARY))
                        .child(format!("{kind} {count}")),
                );
            }
            stats_bar = stats_bar.child(chips);
        }
        if !self.roles_stats.by_role.is_empty() {
            let mut chips = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_wrap()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child("按角色："),
                );
            for (name, calls) in &self.roles_stats.by_role {
                chips = chips.child(
                    div()
                        .px_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::BORDER))
                        .bg(rgb(colors::BG_SECONDARY))
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_SECONDARY))
                        .child(format!("{name} {calls}")),
                );
            }
            stats_bar = stats_bar.child(chips);
        }

        let detail = match self
            .roles_selected
            .as_ref()
            .and_then(|name| self.roles.iter().find(|r| &r.name == name))
        {
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    svg()
                        .path(IconName::Bot.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY)),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child("选择一个子智能体查看详情"),
                )
                .into_any_element(),
            Some(role) => {
                let (source_label, source_color) = source_of(&role.source);
                let card = div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_6()
                            .py_4()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .text_color(rgb(colors::TEXT_PRIMARY))
                                            .child(role.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_full()
                                            .border_1()
                                            .border_color(rgb(source_color))
                                            .text_size(px(11.))
                                            .text_color(rgb(source_color))
                                            .child(source_label),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_TERTIARY))
                                            .child(format!("v{}", role.version)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_TERTIARY))
                                            .child(match role.tool_count {
                                                Some(n) => format!("{n} 工具"),
                                                None => "工具不限".to_string(),
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_TERTIARY))
                                            .child(format!(
                                                "模型：{}",
                                                role.model
                                                    .clone()
                                                    .unwrap_or_else(|| "回落主 Agent".to_string())
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY))
                                    .child(role.purpose.clone()),
                            ),
                    )
                    // 操作栏（对齐 Web 版 RolesPanel）：回退内置种子 / 退役删除
                    .child(self.render_role_actions(role, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .p_6()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_TERTIARY))
                                    .child("系统提示"),
                            )
                            .child(
                                div()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER))
                                    .bg(rgb(colors::BG_SECONDARY))
                                    .p_3()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY))
                                    .child(match &self.role_detail {
                                        Some(d) if !d.system_prompt.is_empty() => {
                                            d.system_prompt.clone()
                                        }
                                        Some(_) => "（空）".to_string(),
                                        None => "加载中…".to_string(),
                                    }),
                            )
                            .children(self.role_detail.as_ref().map(|d| {
                                if d.tools.is_empty() {
                                    div().into_any_element()
                                } else {
                                    let mut chips = div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_1()
                                        .flex_wrap()
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(rgb(colors::TEXT_TERTIARY))
                                                .child("工具白名单"),
                                        );
                                    for tool in &d.tools {
                                        chips = chips.child(
                                            div()
                                                .px_1()
                                                .rounded_md()
                                                .border_1()
                                                .border_color(rgb(colors::BORDER))
                                                .text_size(px(11.))
                                                .text_color(rgb(colors::TEXT_SECONDARY))
                                                .child(tool.clone()),
                                        );
                                    }
                                    chips.into_any_element()
                                }
                            }))
                            .overflow_y_scrollbar(),
                    );
                card.into_any_element()
            }
        };

        div()
            .size_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .w(px(320.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(rgb(colors::BORDER))
                    .bg(rgb(colors::BG_SECONDARY))
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        svg()
                                            .path(IconName::Users.path())
                                            .size(px(18.))
                                            .text_color(rgb(colors::TEXT_SECONDARY)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY))
                                            .child("子智能体"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY))
                                    .child("角色化分工（内置 / 配置 / 学习三源平级），自演化更新"),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER))
                                    .bg(rgb(colors::BG_PRIMARY))
                                    .px_2()
                                    .py_1()
                                    .child(Input::new(&self.roles_search).appearance(false)),
                            ),
                    )
                    .child(list.overflow_y_scrollbar()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(stats_bar)
                    .child(detail),
            )
    }

    /// 角色操作栏：回退内置种子 / 退役删除（都走确认弹窗）。
    ///
    /// 「回退」在 `builtin && version == 1` 时禁用 —— 内置种子本来就是 v1，
    /// 没有可回退的内容（Web 版同口径）。
    /// 目录选择器弹窗：当前路径 + 逐级浏览 + 「选择此目录」。
    ///
    /// 浏览根（盘符列表）不允许直接选中 —— 后端要求的是具体目录。
    /// 文件选择弹窗（**GPUI 自绘**，不是 OS 对话框）：点目录进入、点文件勾选。
    ///
    /// 形态与 workspace 目录选择器一致（遮罩 + 居中卡片 + 列表），因为它们是同一类
    /// 任务（在文件系统里定位）。弹窗而不是平铺：导入 tab 的主任务只有「选文件 +
    /// 导入」，把一张常驻的文件列表铺在页面上会把主次关系拉平。
    fn render_kb_picker_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        // 列表：上级目录 → 子目录 → 文件
        let mut list = div().flex().flex_col().gap_1().w_full();
        if !self.kb_browse_dir.is_empty() && self.kb_browse_dir != "浏览根" {
            let parent = std::path::Path::new(&self.kb_browse_dir)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            list = list.child(
                div()
                    .id("kbp-up")
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_SECONDARY))
                    .child(
                        svg()
                            .path(IconName::CornerLeftUp.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY)),
                    )
                    .child("上级目录")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_kb_browse(parent.clone(), cx);
                    })),
            );
        }
        if self.kb_browse_entries.is_empty() {
            list = list.child(
                div()
                    .py_6()
                    .w_full()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child(if self.kb_browse_status.is_empty() {
                        "（空目录）"
                    } else {
                        "（空目录或读取中）"
                    }),
            );
        }
        // 遇到第一个文件时插一个「文件」分隔标题：目录总在前，没有分隔时
        // 用户看不出「下面还有文件」（截图里第一屏全是目录就是这个感受）。
        let q = self.kb_pick_query.trim().to_lowercase();
        let mut files_label_drawn = false;
        for (ix, e) in self.kb_browse_entries.iter().enumerate() {
            // 默认隐藏点开头的条目：项目根目录动辄十几个 `.git`/`.cargo`/`.vscode`，
            // 第一屏全被它们占满，真正要选的文件得滚好几屏（截图实测过）。
            if !self.kb_show_hidden && e.name.starts_with('.') {
                continue;
            }
            // 过滤词：只在「按名字」上匹配（选文件的场景里，路径信息没用）
            if !q.is_empty() && !e.name.to_lowercase().contains(&q) {
                continue;
            }
            if !e.is_dir && !files_label_drawn {
                files_label_drawn = true;
                list = list.child(
                    div()
                        .px_2()
                        .pt_2()
                        .pb_1()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY))
                        .child("文件"),
                );
            }
            let is_picked = self.kb_picked.iter().any(|p| p == &e.path);
            let dir = e.path.clone();
            let is_dir = e.is_dir;
            let picked_path = e.path.clone();
            let icon = if is_dir {
                IconName::Folder
            } else {
                IconName::File
            };
            let size_text = e.size.map(format_size).unwrap_or_default();
            list =
                list.child(
                    div()
                        .id(("kbp-item", ix))
                        .w_full()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(if is_picked {
                            colors::BG_TERTIARY
                        } else {
                            colors::BG_SECONDARY
                        }))
                        .cursor_pointer()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(svg().path(icon.path()).size(px(12.)).text_color(rgb(
                            if is_picked {
                                colors::ACCENT
                            } else {
                                colors::TEXT_TERTIARY
                            },
                        )))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_size(px(12.))
                                .text_color(rgb(if is_picked {
                                    colors::ACCENT
                                } else {
                                    colors::TEXT_SECONDARY
                                }))
                                .child(e.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY))
                                .child(size_text),
                        )
                        .child(
                            // 目录不给勾选位（点它就是进入），文件给一个明确的勾选标记
                            div()
                                .w(px(16.))
                                .flex()
                                .justify_center()
                                .text_size(px(11.))
                                .text_color(rgb(if is_picked {
                                    colors::SUCCESS
                                } else {
                                    colors::TEXT_TERTIARY
                                }))
                                .child(if is_dir {
                                    ""
                                } else if is_picked {
                                    "✓"
                                } else {
                                    "○"
                                }),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if is_dir {
                                this.load_kb_browse(dir.clone(), cx);
                            } else {
                                this.toggle_kb_pick(picked_path.clone());
                                cx.notify();
                            }
                        })),
                );
        }

        let card =
            div()
                .w(px(680.))
                .max_h(px(620.))
                .rounded_xl()
                .border_1()
                .border_color(rgb(colors::BORDER))
                .bg(rgb(colors::BG_SECONDARY))
                .flex()
                .flex_col()
                // 标题行
                .child(
                    div()
                        .px_4()
                        .py_3()
                        .border_b_1()
                        .border_color(rgb(colors::BORDER))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            svg()
                                .path(IconName::FolderOpen.path())
                                .size(px(16.))
                                .text_color(rgb(colors::TEXT_SECONDARY)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(14.))
                                .text_color(rgb(colors::TEXT_PRIMARY))
                                .child("选择要导入知识库的文件"),
                        )
                        .child(
                            div()
                                .id("kbp-close")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_TERTIARY))
                                .child("✕")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.dialog = None;
                                    cx.notify();
                                })),
                        ),
                )
                // 路径行
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(colors::BORDER))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_TERTIARY))
                                .child(if self.kb_browse_dir.is_empty() {
                                    "浏览根".to_string()
                                } else {
                                    self.kb_browse_dir.clone()
                                }),
                        )
                        .child(
                            div()
                                .id("kbp-root")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(rgb(colors::ACCENT))
                                .child("回到根")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.load_kb_browse(String::new(), cx)
                                })),
                        ),
                )
                // 隐藏项开关（默认隐藏，见列表循环里的过滤）
                .child(
                    div()
                        .px_4()
                        .pb_2()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(colors::BORDER))
                                .bg(rgb(colors::BG_PRIMARY))
                                .px_2()
                                .py_1()
                                .child(Input::new(&self.kb_pick_search).appearance(false)),
                        )
                        .child(
                            div()
                                .id("kbp-hidden")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(if self.kb_show_hidden {
                                    colors::ACCENT
                                } else {
                                    colors::BORDER
                                }))
                                .cursor_pointer()
                                .text_size(px(10.))
                                .text_color(rgb(if self.kb_show_hidden {
                                    colors::ACCENT
                                } else {
                                    colors::TEXT_TERTIARY
                                }))
                                .child(if self.kb_show_hidden {
                                    "隐藏项：显示"
                                } else {
                                    "隐藏项：隐藏"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.kb_show_hidden = !this.kb_show_hidden;
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY))
                                .child(if self.kb_browse_status.is_empty() {
                                    String::new()
                                } else {
                                    self.kb_browse_status.clone()
                                }),
                        ),
                )
                // 列表（确定高度 + 自己滚动）
                .child(
                    div()
                        .h(px(400.))
                        .min_h_0()
                        .px_3()
                        .py_2()
                        .child(list.overflow_y_scrollbar()),
                )
                // 底部：已选 + 清空 / 取消 / 导入
                .child(
                    div()
                        .px_4()
                        .py_3()
                        .border_t_1()
                        .border_color(rgb(colors::BORDER))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_TERTIARY))
                                .child(if self.kb_picked.is_empty() {
                                    "点目录进入，点文件勾选".to_string()
                                } else {
                                    format!("已选 {} 个文件", self.kb_picked.len())
                                }),
                        )
                        .child(
                            div()
                                .id("kbp-clear")
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(colors::BORDER))
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_SECONDARY))
                                .child("清空")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.kb_picked.clear();
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id("kbp-cancel")
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(colors::BORDER))
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_SECONDARY))
                                .child("取消")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.dialog = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id("kbp-confirm")
                                .px_4()
                                .py_2()
                                .rounded_md()
                                .bg(rgb(colors::ACCENT))
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_PRIMARY))
                                .child("导入选中")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.import_kb_picked(cx);
                                    this.dialog = None;
                                    cx.notify();
                                })),
                        ),
                );

        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a0))
            .child(card)
            .into_any_element()
    }

    fn render_workspace_picker_dialog(
        &self,
        target: PickerTarget,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut list = div().flex().flex_col().gap_1().w_full();
        if let Some(parent) = self.picker_parent.clone() {
            list = list.child(
                div()
                    .id("picker-up")
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_SECONDARY))
                    .child(
                        svg()
                            .path(IconName::CornerLeftUp.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY)),
                    )
                    .child("上级目录")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_picker_dirs(parent.clone(), cx);
                    })),
            );
        }
        for (ix, (name, path)) in self.picker_entries.iter().enumerate() {
            let path = path.clone();
            list = list.child(
                div()
                    .id(("picker-dir", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_SECONDARY))
                    .child(
                        svg()
                            .path(IconName::Folder.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(name.clone()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_picker_dirs(path.clone(), cx);
                    })),
            );
        }
        if self.picker_entries.is_empty() && self.picker_parent.is_none() {
            list = list.child(
                div()
                    .px_2()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child(self.picker_status.clone()),
            );
        }

        let can_pick = !self.picker_current.is_empty() && self.picker_current != "浏览根";
        let pick_label = match target {
            PickerTarget::NewSession => "用它新建会话",
            PickerTarget::Session => "绑定到当前会话",
        };

        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a0))
            .child(
                div()
                    .w(px(560.))
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(colors::BORDER))
                    .bg(rgb(colors::BG_SECONDARY))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(15.))
                            .text_color(rgb(colors::TEXT_PRIMARY))
                            .child("选择工作目录"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_SECONDARY))
                                    .child(self.picker_current.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY))
                                    .child(self.picker_status.clone()),
                            ),
                    )
                    .child(
                        div()
                            // 必须给**确定高度**：`overflow_y_scrollbar()` 内部按父高布局，
                            // 父容器只有 `max_h` 时高度会塌成 0（实测列表区只留一条空白）。
                            .h(px(300.))
                            .min_h_0()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(colors::BORDER))
                            .bg(rgb(colors::BG_PRIMARY))
                            .p_2()
                            .child(list.overflow_y_scrollbar()),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .id("picker-cancel")
                                    .px_3()
                                    .py_1()
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .text_sm()
                                    .text_color(rgb(colors::TEXT_SECONDARY))
                                    .child("取消")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.dialog = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("picker-confirm")
                                    .px_3()
                                    .py_1()
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .text_sm()
                                    .bg(rgb(if can_pick {
                                        colors::ACCENT
                                    } else {
                                        colors::BG_TERTIARY
                                    }))
                                    .text_color(rgb(if can_pick {
                                        0xffffff
                                    } else {
                                        colors::TEXT_TERTIARY
                                    }))
                                    .child(pick_label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if can_pick {
                                            this.confirm_workspace_picker(target, cx);
                                        }
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_role_actions(&self, role: &RoleRow, cx: &mut Context<Self>) -> AnyElement {
        let reset_name = role.name.clone();
        let delete_name = role.name.clone();
        let reset_disabled = role.source == "builtin" && role.version <= 1;
        let reset_color = if reset_disabled {
            colors::TEXT_TERTIARY
        } else {
            colors::TEXT_SECONDARY
        };
        div()
            .px_6()
            .py_2()
            .border_b_1()
            .border_color(rgb(colors::BORDER))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("role-reset")
                    .px_3()
                    .py_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(rgb(reset_color))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path(IconName::RotateCcw.path())
                            .size(px(12.))
                            .text_color(rgb(reset_color)),
                    )
                    .child("回退内置种子")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if reset_disabled {
                            return;
                        }
                        this.dialog = Some(Dialog::ResetRole {
                            name: reset_name.clone(),
                        });
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("role-delete")
                    .px_3()
                    .py_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::ERROR))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(rgb(colors::ERROR))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path(IconName::Trash.path())
                            .size(px(12.))
                            .text_color(rgb(colors::ERROR)),
                    )
                    .child("退役删除")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.dialog = Some(Dialog::DeleteRole {
                            name: delete_name.clone(),
                        });
                        cx.notify();
                    })),
            )
            .children(self.roles_action_msg.clone().map(|m| {
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child(m)
            }))
            .into_any_element()
    }

    /// 角色列表项：名称 + 来源徽章 + 职责 + 版本 / 工具数 / 成功率。
    fn render_role_item(&self, ix: usize, role: &RoleRow, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.roles_selected.as_deref() == Some(role.name.as_str());
        let (source_label, source_color) = source_of(&role.source);
        let name = role.name.clone();
        let usage_text = role
            .usage
            .filter(|(calls, _)| *calls > 0)
            .map(|(calls, rate)| {
                (
                    format!("成功率 {}%", (rate * 100.0).round() as u32),
                    rate < 0.5,
                    calls,
                )
            });
        div()
            .id(("role", ix))
            .w_full()
            .px_2()
            .py_2()
            .rounded_lg()
            .bg(rgb(if selected {
                colors::BG_TERTIARY
            } else {
                colors::BG_SECONDARY
            }))
            .cursor_pointer()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_size(px(13.))
                            .text_color(rgb(if selected {
                                colors::ACCENT
                            } else {
                                colors::TEXT_PRIMARY
                            }))
                            .child(role.name.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px_1()
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(source_color))
                            .text_size(px(10.))
                            .text_color(rgb(source_color))
                            .child(source_label),
                    )
                    .child(if role.status == "experimental" {
                        div()
                            .flex_shrink_0()
                            .px_1()
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(colors::WARNING))
                            .text_size(px(10.))
                            .text_color(rgb(colors::WARNING))
                            .child("试验性")
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    }),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(role.purpose.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(10.))
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child(format!("v{}", role.version))
                    .child(match role.tool_count {
                        Some(n) => format!("{n} 工具"),
                        None => "工具不限".to_string(),
                    })
                    .children(usage_text.map(|(text, low, calls)| {
                        div()
                            .text_color(rgb(if low {
                                colors::ERROR
                            } else {
                                colors::TEXT_TERTIARY
                            }))
                            .child(format!("{text}（{calls} 次）"))
                    })),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.roles_selected = Some(name.clone());
                this.role_detail = None;
                this.load_role_detail(name.clone(), cx);
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 提供商区：endpoint / 密钥可编辑（保存时先重读配置再写，见 `save_provider`）
        let mut providers_block = div().flex().flex_col().gap_1().px_4().pt_3().child(
            div()
                .text_xs()
                .text_color(rgb(colors::TEXT_SECONDARY))
                .child("提供商（端点 / 密钥）"),
        );
        for (ix, p) in self.providers.iter().enumerate() {
            let name = p.name.clone();
            providers_block = providers_block.child(
                div()
                    .id(("prov", ix))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .child(div().w(px(110.)).text_xs().child(p.name.clone()))
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(ellipsis(&p.endpoint, 44)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!("{} 模型", p.model_count)),
                    )
                    .child(
                        div()
                            .id(("prov-edit", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(colors::BG_TERTIARY))
                            .cursor_pointer()
                            .text_xs()
                            .child("编辑")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_provider_editor(name.clone(), window, cx);
                            })),
                    ),
            );
        }
        if let Some(name) = self.editing_provider.clone() {
            providers_block = providers_block.child(
                div()
                    .mt_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER))
                    .bg(rgb(colors::BG_SECONDARY))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child(format!("编辑 {name}")),
                    )
                    .child(Textarea::new(&self.edit_endpoint).h(px(40.)))
                    .child(Textarea::new(&self.edit_api_key).h(px(40.)))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(
                                div()
                                    .id("prov-test")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(colors::BG_TERTIARY))
                                    .text_color(rgb(colors::TEXT_PRIMARY))
                                    .cursor_pointer()
                                    .text_xs()
                                    .child("测试连接")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.test_provider_connection(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("prov-save")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(colors::ACCENT))
                                    .text_color(rgb(0xffffff))
                                    .cursor_pointer()
                                    .text_xs()
                                    .child("保存")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.save_provider(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("prov-cancel")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(rgb(colors::TEXT_TERTIARY))
                                    .child("取消")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.editing_provider = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }
        let mut list = div().flex().flex_col().gap_1().p_4();
        if self.models.is_empty() {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child("暂无模型：请先在配置里添加 provider 与模型"),
            );
        }
        for (ix, m) in self.models.iter().enumerate() {
            let provider = m.provider.clone();
            let name = m.name.clone();
            let meta = {
                let mut parts: Vec<String> = Vec::new();
                if !m.capabilities.is_empty() {
                    parts.push(m.capabilities.clone());
                }
                if let Some(ctx_len) = m.context {
                    parts.push(format!("ctx {ctx_len}"));
                }
                parts.join(" · ")
            };
            let mut row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded_md();
            if m.is_chat {
                row = row.bg(rgb(colors::BG_HOVER));
            }
            let action = if m.is_chat {
                div()
                    .text_xs()
                    .text_color(rgb(colors::SUCCESS))
                    .child("当前聊天模型")
                    .into_any_element()
            } else {
                div()
                    .id(("set-chat", ix))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(colors::BG_TERTIARY))
                    .text_color(rgb(colors::TEXT_PRIMARY))
                    .cursor_pointer()
                    .child("设为聊天模型")
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.switch_chat_model(provider.clone(), name.clone(), cx);
                    }))
                    .into_any_element()
            };
            list = list.child(
                row.child(div().w(px(110.)).text_xs().child(m.provider.clone()))
                    .child(div().flex_1().child(m.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(meta),
                    )
                    .child(action),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER))
                    .child(div().text_sm().child("设置 · 模型"))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(if self.settings_error {
                                rgb(colors::ERROR)
                            } else {
                                rgb(colors::TEXT_SECONDARY)
                            })
                            .child(self.settings_status.clone()),
                    ),
            )
            .child(providers_block)
            .child(div().flex_1().min_h_0().child(list))
    }

    /// 显式停止当前轮：`POST /chat/streams/{id}/cancel`（ADR-036 的结构性取消
    /// 在服务端置位，客户端断线**不**取消——本地助手跑完再取）。
    fn stop_current_turn(&mut self, cx: &mut Context<Self>) {
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        let port = self.port;
        self.status = "正在停止…".to_string();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/chat/streams/{sid}/cancel");
            let res = post_json(url, serde_json::json!({})).await;
            let _ = this.update(cx, |this, cx| {
                this.status = match res {
                    Ok(_) => format!("已请求停止 · {sid}"),
                    Err(e) => format!("停止失败：{e}"),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// 选中会话：切换路由键并拉消息
    fn open_session(&mut self, id: String, cx: &mut Context<Self>) {
        self.current_session = Some(id.clone());
        // 切会话即放弃上一会话的流式状态（本客户端只渲染当前会话的增量）
        self.streaming = false;
        // 会话级模型 / 上下文进度都是**按会话**的：切走即回落（模型回到 preferences.chat）
        self.session_model = None;
        self.model_menu_open = false;
        self.ctx_used = 0;
        self.ctx_window = 0;
        // 统计行是按会话的：切走即清空，由新会话的两次 GET 重新填
        self.session_usage = None;
        self.request_perf = None;
        // 会话语义的面板数据先清空（避免看着新会话却显示上一条的待办/审批），
        // 再**立即**拉一次 —— 否则要等 3s 轮询窗口，体感就是"切过去慢一拍"
        self.todos.clear();
        self.goals.clear();
        self.pending_approvals.clear();
        // 文件页目录树按会话绑定目录解析：切会话必须作废，否则会继续显示
        // 上一条会话（或全局默认）的目录
        self.ws_root.clear();
        self.ws_entries.clear();
        self.ws_file = None;
        self.ws_path.clear();
        self.load_messages(id, cx);
        self.load_session_usage(cx);
        self.load_todos_goals(cx);
        self.load_approvals(cx);
        // 选中态与「当前会话始终可见」都体现在左栏行上
        self.rebuild_session_rows(cx);
    }

    /// 处理一条统一事件。
    ///
    /// 只认 `type=chat_stream` 且只认当前会话（子代理事件的 `session_id` 是
    /// `task_id`，属于任务面板，后续接入）。`chunk_type` 语义见
    /// `docs/architecture/event-protocol.md` §4。
    fn handle_event(&mut self, ev: &serde_json::Value, cx: &mut Context<Self>) {
        if ev.get("type").and_then(|v| v.as_str()) != Some("chat_stream") {
            return;
        }
        let sid = ev
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if Some(sid) != self.current_session.as_deref() {
            return;
        }
        match ev
            .get("chunk_type")
            .and_then(|v| v.as_str())
            .unwrap_or("answer")
        {
            "answer" => {
                let delta = ev.get("delta").and_then(|v| v.as_str()).unwrap_or_default();
                self.append_delta(delta);
                // 上下文占用（Tauri 版 `ContextRing` 的数据源）：usage 随 answer 块下发
                if let Some(u) = ev.get("usage") {
                    if let Some(used) = u.get("total_tokens").and_then(|v| v.as_u64()) {
                        self.ctx_used = used;
                    }
                    if let Some(win) = u.get("context_window").and_then(|v| v.as_u64()) {
                        self.ctx_window = win;
                    }
                }
            }
            // 思考增量暂不并入正文（会污染 Markdown 结构），待独立渲染区
            "thought" => {}
            "tool_call" => {
                let name = ev
                    .get("tool_call")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("工具");
                self.append_delta(&format!("\n`🔧 {name}`\n"));
                // ask_user 是**同步工具**：工具执行挂起等回答，输入区要被追问接管
                if name == "ask_user" {
                    if let Some(args) = ev.get("tool_call").and_then(|v| v.get("arguments")) {
                        let questions = parse_ask_user(args);
                        if !questions.is_empty() {
                            self.clarify_answers = vec![String::new(); questions.len()];
                            self.clarification = Some(questions);
                        }
                    }
                }
            }
            "observation" => {}
            // 消息边界：流末尾的 assistant 完整消息（含工具段）为权威全文，替换占位
            "message" => {
                if let Some(msg) = ev.get("message") {
                    if msg.get("role").and_then(|v| v.as_str()) == Some("assistant") {
                        let full = segments_text(msg);
                        self.replace_last(&full);
                    }
                }
            }
            "error" => {
                let msg = ev
                    .get("delta")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未知错误");
                self.streaming = false;
                self.status = format!("出错：{msg}");
                self.append_delta(&format!("\n\n> ⚠️ {msg}\n"));
            }
            "turn_state" => {
                let state = ev
                    .get("turn_state")
                    .and_then(|v| v.get("state"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                self.streaming = state == "running";
                if !self.streaming {
                    self.status = format!("空闲 · {sid}");
                    // 本轮结束：刷新底部统计行（token 汇总 + 最近一次请求性能）
                    self.load_session_usage(cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// 往最后一条 assistant 消息追加增量（虚拟列表 count 不变，只重测末项）
    fn append_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let Some(last) = self.messages.last() else {
            return;
        };
        if last.role != "assistant" {
            return;
        }
        let mut next: Vec<MsgRow> = (*self.messages).clone();
        let idx = next.len() - 1;
        let merged = format!("{}{}", next[idx].text, delta);
        next[idx].text = merged.into();
        self.messages = Rc::new(next);
        self.msg_list_state.remeasure_items(idx..idx + 1);
        self.msg_list_state.scroll_to_end();
    }

    /// 用权威全文替换最后一条 assistant 消息
    fn replace_last(&mut self, full: &str) {
        if self.messages.last().map(|m| m.role.as_str()) != Some("assistant") {
            return;
        }
        let mut next: Vec<MsgRow> = (*self.messages).clone();
        let idx = next.len() - 1;
        next[idx].text = full.to_string().into();
        self.messages = Rc::new(next);
        self.msg_list_state.remeasure_items(idx..idx + 1);
        self.msg_list_state.scroll_to_end();
    }

    /// 发送输入框内容（Enter 或「发送」按钮）
    fn send_current_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.streaming {
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.send_text(text, cx);
    }

    /// 发送一段文本：`POST /chat/stream` 只是开关（ADR-028），输出走常驻 SSE。
    fn send_text(&mut self, text: String, cx: &mut Context<Self>) {
        if self.streaming {
            return;
        }
        // 新会话：id 必须在发起前确定（事件按 session_id 路由）
        let session_id = match self.current_session.clone() {
            Some(sid) => sid,
            None => {
                let sid = new_session_id();
                self.current_session = Some(sid.clone());
                sid
            }
        };

        let mut next: Vec<MsgRow> = (*self.messages).clone();
        next.push(MsgRow {
            role: "user".to_string(),
            text: text.clone().into(),
            segs: Vec::new(),
            tool_calls: Vec::new(),
            compression: false,
        });
        next.push(MsgRow {
            role: "assistant".to_string(),
            text: "".into(),
            segs: Vec::new(),
            tool_calls: Vec::new(),
            compression: false,
        });
        if next.len() > RENDER_LIMIT {
            let drop_n = next.len() - RENDER_LIMIT;
            next.drain(..drop_n);
        }
        self.messages = Rc::new(next);
        self.msg_list_state.reset(self.messages.len());
        self.msg_list_state.scroll_to_end();
        self.message_total = self.messages.len();
        self.streaming = true;
        self.status = format!("生成中… · {session_id}");

        let port = self.port;
        let mut body = serde_json::json!({
            "session_id": session_id,
            "message": { "role": "user", "content": text },
            "stream": true,
            "temperature": 0.7,
            "max_tokens": 2048
        });
        // 会话级模型：随首条消息一起下发——服务端建会话时就固化，不必等会话 id 生成后再补 PUT
        if let Some((provider, model)) = &self.session_model {
            body["model"] = serde_json::Value::String(model.clone());
            body["provider"] = serde_json::Value::String(provider.clone());
        }
        // 「＋新目录」设置的工作目录：只作用于这一条消息——服务端建会话时固化，
        // 之后的消息再带就没意义了（所以 `take()` 掉）。
        if let Some(dir) = self.new_session_workspace.take() {
            body["working_directory"] = serde_json::Value::String(dir);
        }
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/chat/stream");
            match post_json(url, body).await {
                Ok(_) => {
                    // 新会话首轮：刷新左侧列表把新会话显示出来
                    let _ = this.update(cx, |this, cx| {
                        this.load_sessions(cx);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.streaming = false;
                        this.status = format!("发送失败：{e}");
                        this.append_delta(&format!("\n\n> ⚠️ 发送失败：{e}\n"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn load_messages(&mut self, session_id: String, cx: &mut Context<Self>) {
        let port = self.port;
        println!("[消息] 加载会话 {session_id}");
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/sessions/{session_id}/messages?limit=200"
            );
            match fetch_json(url).await {
                Ok(body) => {
                    let arr = body
                        .get("messages")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let rows: Vec<MsgRow> = arr
                        .iter()
                        .map(|m| {
                            let (segs, tool_calls) = parse_segments(m);
                            MsgRow {
                                role: m
                                    .get("role")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("?")
                                    .to_string(),
                                text: segments_text(m).into(),
                                segs,
                                tool_calls,
                                compression: m
                                    .get("compression_marker")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false),
                            }
                        })
                        .collect();
                    let n = rows.len();
                    println!("[消息] 拉到 {n} 条");
                    // 性能：只渲染最近 RENDER_LIMIT 条（Markdown 布局成本高，
                    // 全量渲染 90+ 条会明显卡顿）。历史加载留待后续做上滚分页。
                    let total = n;
                    let kept: Vec<MsgRow> = if n > RENDER_LIMIT {
                        rows.into_iter().skip(n - RENDER_LIMIT).collect()
                    } else {
                        rows
                    };
                    cx.update(|app| {
                        let _ = this.update(app, |this, cx| {
                            this.messages = Rc::new(kept);
                            // 条数变了必须重建 list state（item_count 在构造时固定）
                            this.msg_list_state = GpuiListState::new(
                                this.messages.len(),
                                ListAlignment::Bottom,
                                px(120.),
                            );
                            this.message_total = total;
                            this.current_title = if total > RENDER_LIMIT {
                                format!("会话消息（共 {total} 条，显示最近 {RENDER_LIMIT} 条）")
                            } else {
                                format!("会话消息（{total} 条）")
                            };
                            cx.notify();
                        });
                    });
                }
                Err(e) => {
                    println!("[消息] 拉取失败: {e}");
                    cx.update(|app| {
                        let _ = this.update(app, |this, cx| {
                            this.current_title = format!("加载失败：{e}");
                            cx.notify();
                        });
                    });
                }
            }
        })
        .detach();
    }
}

impl ClientApp {
    /// 会话视图：左列表 + 右消息 + 输入。
    ///
    /// 布局对齐 `gui-vite` 的 `SessionPage`（左栏会话列表 280px + 右栏 ChatPanel 撑满）。
    fn render_chat(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 模型下拉候选：只列带 `chat` 能力的模型（与 Tauri 版 `ModelSelector` 同口径，
        // capabilities 是 kebab-case 数组，后端拼成 `/` 分隔的串）
        let chat_models: Vec<(String, String)> = self
            .models
            .iter()
            .filter(|m| m.capabilities.split('/').any(|c| c == "chat"))
            .map(|m| (m.provider.clone(), m.name.clone()))
            .collect();
        let menu_open = self.model_menu_open;
        let ctx_label = self.ctx_label();
        let ctx_high = self.ctx_pct() >= 80;
        let mut menu = div()
            .absolute()
            .bottom(px(40.))
            .right(px(0.))
            .w(px(256.))
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER))
            .bg(rgb(colors::BG_PRIMARY))
            .py_1()
            .flex()
            .flex_col();
        for (ix, (provider, name)) in chat_models.iter().enumerate() {
            let provider = provider.clone();
            let name = name.clone();
            let is_current = self
                .session_model
                .as_ref()
                .map(|(p, m)| p == &provider && m == &name)
                .unwrap_or(false);
            let mut row = div()
                .id(("model-option", ix))
                .px_3()
                .py_2()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .cursor_pointer()
                .text_xs();
            if is_current {
                row = row.text_color(rgb(colors::ACCENT));
            } else {
                row = row.text_color(rgb(colors::TEXT_PRIMARY));
            }
            let label = name.clone();
            let prov = provider.clone();
            menu = menu.child(
                row.child(div().flex_1().child(name))
                    .child(div().text_color(rgb(colors::TEXT_TERTIARY)).child(provider))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.select_chat_model(prov.clone(), label.clone(), cx);
                    })),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_row()
            // 左：会话列表（对齐 Tauri 版 SessionList：280px、bg-secondary、右侧分隔线）
            .child(
                div()
                    .w(px(colors::LIST_WIDTH))
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(rgb(colors::BG_SECONDARY))
                    .border_r_1()
                    .border_color(rgb(colors::BORDER))
                    // 头部：状态行 + 新建会话（对齐 Tauri 版 `SessionList` 头部；
                    // 「＋ 新目录」要目录选择器弹窗，留到 workspace 页一起做）
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(rgb(colors::TEXT_SECONDARY))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(self.status.clone()),
                            )
                            .child(
                                div()
                                    .id("new-session")
                                    .size(px(22.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .child(
                                        svg()
                                            .path(IconName::Plus.path())
                                            .size(px(14.))
                                            .text_color(rgb(colors::TEXT_SECONDARY)),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.start_new_session(cx);
                                    })),
                            )
                            // 「＋新目录」：新建一个绑定指定工作目录的会话（目录在
                            // 首条消息时随 `working_directory` 下发并固化）
                            .child(
                                div()
                                    .id("new-workspace")
                                    .size(px(22.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .child(
                                        svg()
                                            .path(IconName::FolderPlus.path())
                                            .size(px(14.))
                                            .text_color(rgb(colors::TEXT_SECONDARY)),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let _ = window;
                                        this.open_workspace_picker(PickerTarget::NewSession, cx);
                                    })),
                            ),
                    )
                    // 分组列表（gpui-component `List`：自带虚拟化与滚动）
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .px_2()
                            .py_2()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(
                                (0..self.session_rows.len())
                                    .map(|ix| self.render_session_row(ix, cx))
                                    .collect::<Vec<_>>(),
                            )
                            .overflow_y_scrollbar(),
                    )
                    // 底部：文件视图入口（占位页；与 Tauri 版同位置）
                    .child(
                        div()
                            .border_t_1()
                            .border_color(rgb(colors::BORDER))
                            .px_2()
                            .py_2()
                            .child(
                                div()
                                    .id("open-workspace")
                                    .w_full()
                                    .px_2()
                                    .py_1()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(rgb(colors::TEXT_SECONDARY))
                                    .child(
                                        svg()
                                            .path(IconName::FileText.path())
                                            .size(px(14.))
                                            .text_color(rgb(colors::TEXT_SECONDARY)),
                                    )
                                    .child("文件视图")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.view = View::Workspace;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            // 右：消息列表
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .text_sm()
                                    .border_b_1()
                                    .child(self.current_title.clone()),
                            )
                            // 消息列：限宽 896 居中（与输入卡片同宽、同左边界）。
                            // 父级用 `flex_col + items_center` 让限宽块水平居中；
                            // 限宽块自身 `w_full + max_w` 才会在宽窗口下取 896。
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_h_0()
                                            .w_full()
                                            .max_w(px(colors::CHAT_MAX_W))
                                            .child({
                                                let items = self.messages.clone();
                                                let expand = self.expand.clone();
                                                let app = self.app_ref.clone();
                                                gpui_list(
                                                    self.msg_list_state.clone(),
                                                    move |ix, _window, _cx| {
                                                        let Some(m) = items.get(ix) else {
                                                            return div().into_any_element();
                                                        };
                                                        // 段渲染口径见 `render_msg_body`（思考块 / 工具卡可折叠，
                                                        // 结果按 `tool_calls` 的 id 关联）
                                                        render_msg_body(m, ix, &expand, &app)
                                                    },
                                                )
                                                .size_full()
                                            }),
                                    ),
                            )
                            // 审批横幅 / 追问卡片 / 待办窄条：与消息列、输入卡片同宽居中
                            // （三者空时各自返回 0 高元素，不占位）
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .child(
                                        div()
                                            .w_full()
                                            .max_w(px(colors::CHAT_MAX_W))
                                            .child(self.render_approval_banner(cx)),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .max_w(px(colors::CHAT_MAX_W))
                                            .child(self.render_clarification(cx)),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .max_w(px(colors::CHAT_MAX_W))
                                            .child(self.render_todo_bar(cx)),
                                    ),
                            )
                            // 底部输入区 = **一体式卡片**（对齐 Tauri 版 `ChatInput`）：
                            //   外框限宽 896px 居中、bg-secondary、rounded-2xl；
                            //   卡片内 = 编辑器 + 底部控件行（当前模型 / 发送或停止）。
                            // 顶部有 border-t 与底色，和消息区之间靠这条线分隔。
                            .child(
                                div()
                                    .border_t_1()
                                    .border_color(rgb(colors::BORDER))
                                    .bg(rgb(colors::BG_PRIMARY))
                                    .px_4()
                                    .pt_3()
                                    .pb_3()
                                    .child(
                                        div()
                                            .w_full()
                                            .max_w(px(896.))
                                            .mx_auto()
                                            .rounded_2xl()
                                            .border_1()
                                            .border_color(rgb(colors::BORDER))
                                            .bg(rgb(colors::BG_SECONDARY))
                                            .p_2()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            // 编辑器自身不画边框与底色（`appearance(false)`），
                                            // 由外层卡片充当「输入框」——否则会看成上下分离的
                                            // 两块（上面一个输入框 + 下面一排按钮），与 Tauri
                                            // 版「按钮在输入框内」的形态不符。
                                            .child(
                                                Textarea::new(&self.input)
                                                    .h(px(64.))
                                                    .appearance(false),
                                            )
                                            .child(
                                                div()
                                                    .relative()
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(div().flex_1())
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .flex_row()
                                                            .items_center()
                                                            .gap_1()
                                                            .child(
                                                                ProgressCircle::new("ctx-ring")
                                                                    .value(
                                                                        self.ctx_pct() as f32
                                                                            / 100.0,
                                                                    )
                                                                    .color(Hsla::from(rgb(
                                                                        if ctx_high {
                                                                            colors::WARNING
                                                                        } else {
                                                                            colors::ACCENT
                                                                        },
                                                                    ))),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(if ctx_high {
                                                                        rgb(colors::WARNING)
                                                                    } else {
                                                                        rgb(colors::TEXT_TERTIARY)
                                                                    })
                                                                    .child(ctx_label),
                                                            ),
                                                    )
                                                    .child(
                                                        div()
                                                            .id("model-select")
                                                            .px_2()
                                                            .py_1()
                                                            .rounded_lg()
                                                            .cursor_pointer()
                                                            .text_xs()
                                                            .text_color(rgb(colors::TEXT_PRIMARY))
                                                            .child(self.current_model_label())
                                                            .on_click(cx.listener(
                                                                |this, _, _window, cx| {
                                                                    this.model_menu_open =
                                                                        !this.model_menu_open;
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    )
                                                    .child({
                                                        let stopping = self.streaming;
                                                        div()
                                                            .id("send")
                                                            .px_3()
                                                            .py_1()
                                                            .rounded_xl()
                                                            .flex()
                                                            .flex_row()
                                                            .items_center()
                                                            .gap_1()
                                                            .bg(if stopping {
                                                                rgb(colors::ERROR)
                                                            } else {
                                                                rgb(colors::ACCENT)
                                                            })
                                                            .text_color(rgb(0xffffff))
                                                            .cursor_pointer()
                                                            // 与 Tauri 版 `ChatInput` 同形：图标 + 文案
                                                            // （发送 = lucide `Send`，停止 = `Square`）
                                                            .child(
                                                                svg()
                                                                    .path(if stopping {
                                                                        IconName::Square.path()
                                                                    } else {
                                                                        IconName::Send.path()
                                                                    })
                                                                    .size(px(14.))
                                                                    .text_color(rgb(0xffffff)),
                                                            )
                                                            .child(if stopping {
                                                                "停止"
                                                            } else {
                                                                "发送"
                                                            })
                                                            .on_click(cx.listener(
                                                                |this, _, window, cx| {
                                                                    if this.streaming {
                                                                        this.stop_current_turn(cx);
                                                                    } else {
                                                                        this.send_current_input(
                                                                            window, cx,
                                                                        );
                                                                    }
                                                                },
                                                            ))
                                                    })
                                                    .children(if menu_open {
                                                        Some(menu)
                                                    } else {
                                                        None
                                                    }),
                                            ),
                                    )
                                    // 底部统计行（对齐 Tauri 版 `ChatInput`）：未命中输入 /
                                    // 缓存命中输入 / 输出 / 缓存命中率 / TTFT / tok/s。
                                    // 两路数据都为空时整行不渲染（不留空白条）。
                                    .child(
                                        if self.session_usage.is_none()
                                            && self.request_perf.is_none()
                                        {
                                            div().into_any_element()
                                        } else {
                                            let mut line = div()
                                                .w_full()
                                                .max_w(px(colors::CHAT_MAX_W))
                                                .mx_auto()
                                                .pt_2()
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .justify_center()
                                                .gap_2()
                                                .text_size(px(11.))
                                                .text_color(rgb(colors::TEXT_TERTIARY));
                                            let dot =
                                                || div().text_color(rgb(colors::BORDER)).child("·");
                                            if let Some((uncached, cached, completion)) =
                                                self.session_usage
                                            {
                                                let total = uncached + cached;
                                                let hit_rate = if total > 0 {
                                                    (cached as f64 / total as f64 * 100.0).round()
                                                        as u64
                                                } else {
                                                    0
                                                };
                                                line = line
                                                    .child(div().child(format!(
                                                        "未命中输入 {}",
                                                        group_thousands(uncached)
                                                    )))
                                                    .child(dot())
                                                    .child(div().child(format!(
                                                        "缓存命中输入 {}",
                                                        group_thousands(cached)
                                                    )))
                                                    .child(dot())
                                                    .child(div().child(format!(
                                                        "输出 {}",
                                                        group_thousands(completion)
                                                    )))
                                                    .child(dot())
                                                    .child(
                                                        div().child(format!(
                                                            "缓存命中率 {hit_rate}%"
                                                        )),
                                                    );
                                            }
                                            if let Some((ttft, duration, completion)) =
                                                self.request_perf
                                            {
                                                if self.session_usage.is_some() {
                                                    line = line.child(dot());
                                                }
                                                line = line.child(div().child(format!(
                                                "TTFT {}",
                                                ttft.map(format_ms)
                                                    .unwrap_or_else(|| "—".to_string())
                                            )));
                                                if let Some(tps) =
                                                    tokens_per_sec(ttft, duration, completion)
                                                {
                                                    line = line
                                                        .child(dot())
                                                        .child(div().child(format!("{tps} tok/s")));
                                                }
                                            }
                                            line.into_any_element()
                                        },
                                    ),
                            ),
                    )
                    .child(self.render_tasks_panel(cx)),
            )
            .into_any_element()
    }
}

impl Render for ClientApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 整体骨架与 `gui-vite/src/components/layout/AppLayout.tsx` 一致：
        //   一级 rail(64px) + main(flex-1, bg-primary)
        div()
            .size_full()
            .flex()
            .flex_row()
            // 相对定位：模态对话框覆盖层用 `absolute` 相对这里铺满整窗
            .relative()
            .bg(rgb(colors::BG_PRIMARY))
            .text_color(rgb(colors::TEXT_PRIMARY))
            .child(self.render_rail(cx))
            .child(div().flex_1().min_w_0().h_full().child(match self.view {
                View::Chat => self.render_chat(cx).into_any_element(),
                View::Settings => self.render_settings(cx).into_any_element(),
                View::Roles => self.render_roles(cx).into_any_element(),
                View::Workspace => self.render_workspace(cx).into_any_element(),
                View::Tools => self.render_tools(cx).into_any_element(),
                View::Skills => self.render_skills(cx).into_any_element(),
                View::Scheduled => self.render_scheduled(cx).into_any_element(),
                View::Memory => self.render_memory(cx).into_any_element(),
                View::Insights => self.render_insights(cx).into_any_element(),
                View::Approval => self.render_approval(cx).into_any_element(),
                View::Knowledge => self.render_knowledge(cx).into_any_element(),
            }))
            .child(if self.dialog.is_some() {
                self.render_dialog(cx)
            } else {
                div().into_any_element()
            })
    }
}

/// 子代理活动流的一行（把消息摊平后的投影）。
enum ActivityRow {
    Thinking { key: String, text: String },
    Text { key: String, text: String },
    Tool { key: String, call: ToolCall },
}

impl ClientApp {
    /// 子代理活动流（对齐 Web 版 `DelegateActivity`）。
    ///
    /// **紧凑单行列表**，不是消息气泡：图标 + 标签 + 摘要 + 字数/耗时，点开看原文。
    /// 相邻同类段合并 —— 流式逐 delta 会切出大量碎片段，不合并会一行一个 token。
    fn render_delegate_activity(
        &self,
        task_id: &str,
        live: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(messages) = self.task_messages.get(task_id) else {
            return div()
                .px_1()
                .py_1()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_TERTIARY))
                .child(if live {
                    "子智能体工作中…"
                } else {
                    "加载中…"
                })
                .into_any_element();
        };
        let mut rows: Vec<ActivityRow> = Vec::new();
        for (mi, m) in messages.iter().enumerate() {
            for (si, seg) in m.segs.iter().enumerate() {
                let key = format!("{task_id}:{mi}:{si}");
                match seg {
                    Seg::Thinking(t) => {
                        if let Some(ActivityRow::Thinking { text, .. }) = rows.last_mut() {
                            text.push_str(t);
                        } else {
                            rows.push(ActivityRow::Thinking {
                                key,
                                text: t.clone(),
                            });
                        }
                    }
                    Seg::Text(t) => {
                        if let Some(ActivityRow::Text { text, .. }) = rows.last_mut() {
                            text.push_str(t);
                        } else {
                            rows.push(ActivityRow::Text {
                                key,
                                text: t.clone(),
                            });
                        }
                    }
                    Seg::Tool { id, name, .. } => {
                        let call = m
                            .tool_calls
                            .iter()
                            .find(|c| &c.id == id)
                            .or_else(|| m.tool_calls.iter().find(|c| &c.name == name))
                            .cloned()
                            .unwrap_or_else(|| ToolCall {
                                id: id.clone(),
                                name: name.clone(),
                                arguments: String::new(),
                                presentation: "generic".to_string(),
                                result: None,
                                duration_ms: None,
                                error: None,
                            });
                        rows.push(ActivityRow::Tool { key, call });
                    }
                }
            }
        }
        if rows.is_empty() {
            return div()
                .px_1()
                .py_1()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_TERTIARY))
                .child(if live {
                    "子智能体工作中…"
                } else {
                    "无过程记录"
                })
                .into_any_element();
        }
        let mut list = div().max_h(px(360.)).flex().flex_col().gap_1().w_full();
        for (ix, row) in rows.into_iter().enumerate() {
            list = list.child(self.render_activity_row(ix, row, cx));
        }
        list.overflow_y_scrollbar().into_any_element()
    }

    /// 活动流的一行：收起态一行（图标 + 标签 + 摘要 + meta + 箭头），展开态一块原文。
    fn render_activity_row(
        &self,
        ix: usize,
        row: ActivityRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open_key = self.expand.borrow().activity.clone();
        let (key, is_tool) = match &row {
            ActivityRow::Thinking { key, .. } | ActivityRow::Text { key, .. } => {
                (key.clone(), false)
            }
            ActivityRow::Tool { key, .. } => (key.clone(), true),
        };
        let open = open_key.as_deref() == Some(key.as_str());
        let chevron = if open {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };

        // 头：图标 + 标签 + 摘要 + meta + 箭头
        let mut head = div()
            .id(("act", ix))
            .w_full()
            .px_1()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .text_size(px(11.));
        let mut details: Vec<AnyElement> = Vec::new();
        match row {
            ActivityRow::Tool { call, .. } => {
                let (icon, label) = presentation_of(&call.presentation);
                let failed = call.error.is_some();
                let settled = call.duration_ms.is_some() || call.result.is_some();
                let target = summarize_tool_target(&call.arguments);
                head = head
                    .child(if failed {
                        svg()
                            .path(IconName::CircleX.path())
                            .size(px(11.))
                            .text_color(rgb(colors::ERROR))
                            .into_any_element()
                    } else {
                        svg()
                            .path(icon.path())
                            .size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .into_any_element()
                    })
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(target),
                    );
                if let Some(ms) = call.duration_ms {
                    head = head.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format_duration(ms)),
                    );
                } else if !settled {
                    head = head.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child("…"),
                    );
                }
                details.push(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_SECONDARY))
                        .child(format_arguments(&call.arguments))
                        .into_any_element(),
                );
                if let Some(err) = call.error {
                    details.push(
                        div()
                            .px_2()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(rgb(colors::ERROR))
                            .child(err)
                            .into_any_element(),
                    );
                }
                if let Some(result) = call.result.filter(|r| !r.is_empty()) {
                    details.push(
                        div()
                            .px_2()
                            .py_1()
                            .max_h(px(220.))
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child(result)
                            .into_any_element(),
                    );
                }
            }
            ActivityRow::Thinking { text, .. } => {
                // 思考行摘要取**最后一行**（模型的当前念头），输出行取第一行
                let banner = text
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                head = head
                    .child(
                        svg()
                            .path(IconName::Brain.path())
                            .size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child("思考"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(banner),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!("{} 字", text.chars().count())),
                    );
                details.push(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_SECONDARY))
                        .child(text)
                        .into_any_element(),
                );
            }
            ActivityRow::Text { text, .. } => {
                let banner = text
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                head = head
                    .child(
                        svg()
                            .path(IconName::TextAlignStart.path())
                            .size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(colors::TEXT_SECONDARY))
                            .child("输出"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(banner),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!("{} 字", text.chars().count())),
                    );
                details.push(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_SECONDARY))
                        .child(text)
                        .into_any_element(),
                );
            }
        }
        head = head.child(
            svg()
                .path(chevron.path())
                .size(px(10.))
                .text_color(rgb(colors::TEXT_TERTIARY)),
        );
        let _ = is_tool;
        let head = head.on_click(cx.listener(move |this, _, _, cx| {
            let key = key.clone();
            let cur = this.expand.borrow().activity.clone();
            this.expand.borrow_mut().activity = if cur.as_deref() == Some(key.as_str()) {
                None
            } else {
                Some(key.clone())
            };
            cx.notify();
        }));

        let mut outer = div().flex().flex_col().w_full().child(head);
        if open {
            for d in details {
                outer = outer.child(d);
            }
        }
        outer.into_any_element()
    }
}

fn main() {
    let config = tianyan_service_config::load_and_install();
    if let Err(e) = tianyan::common::logging::init_logging(&config.logging) {
        eprintln!("日志初始化失败：{e}");
    }
    tianyan::common::panic_hook::install(tianyan::common::panic_hook::default_log_dir());

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    // 单 server 复用（与桌面版共用同一份启动编排）：本机若已有**服务同一数据目录**的
    // 天演实例就直接连它，本进程不再起第二个 —— 否则两个 server 会各跑一遍定时任务
    // 并并发写同一个 SQLite（实测撞上过 3000 + 3001 同时跑）。
    let endpoint = match runtime().block_on(tianyan_desktop::ensure_server(
        config.clone(),
        PREFERRED_PORT,
        shutdown_rx,
    )) {
        Ok(ep) => ep,
        Err(e) => {
            eprintln!("[启动] 内嵌 server 不可用：{e}");
            std::process::exit(1);
        }
    };
    let port = endpoint.port;
    if endpoint.reused {
        println!("[启动] 复用本机已有 server（端口 {port}），本进程未启动 server");
    } else {
        println!("[启动] 内嵌 server 已就绪（端口 {port}）");
    }

    // 图标资源：`gpui-kit-assets` 内嵌完整 Lucide 目录（`AllAssets`），
    // 必须在 `application()` 上注册 —— 否则 `svg().path("icons/*.svg")`
    // 加载不到，图标静默空白（不报错）。
    let app = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets::new(""));
    app.run(move |cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        // 自动化端口（ADR-052）：服务在 tokio 线程、动作用 UI 线程执行，所以先把
        // `AsyncApp` 取出来带进窗口构建闭包——窗口建好才有状态可读。
        let async_cx = cx.to_async();
        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("天演".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    let view = cx.new(|cx| ClientApp::new(window, cx, port));
                    // 未设 `TIANYAN_GUI_AUTOMATION_PORT` 时是空操作
                    automation::maybe_start(&async_cx, view.downgrade());
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("failed to open window");
        })
        .detach();
    });

    let _ = shutdown_tx.send(true);
    println!("[退出] 已发送关停信号");
}

//! 天演原生桌面客户端（GPUI）。
//!
//! 与 `gui-vite`（Tauri + React）**并存**的第二个客户端，连同一个内嵌 axum server，
//! 走同一套 HTTP/SSE 协议。gui-vite 退役前 `tauri/` 必须保留。
//!
//! 当前进度：装配内核 → 起内嵌 server → 拉会话列表 → 点会话看消息（Markdown 渲染）。
//! 未完成：SSE 流式发送、设置页/workspace/审批面板、托盘/通知/快捷键/updater、单实例保护。

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
use std::sync::OnceLock;
use std::time::Duration;
use tokio::runtime::Runtime;

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
#[derive(Clone)]
/// 需要用户确认的模态对话框（自绘覆盖层，不依赖组件库 Dialog）。
enum Dialog {
    /// 删除会话确认
    DeleteSession { id: String, title: String },
    /// 重命名会话（输入框实体在 `ClientApp::dialog_input`）
    RenameSession { id: String },
    /// 新建会话并绑定工作目录（「＋新目录」；输入框实体同上）
    NewWorkspace,
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
    kind: String,
    status: String,
    description: String,
    /// 归属会话 id（左栏角标按它聚合；子代理/终端任务才有）
    parent_session_id: String,
}

/// 消息内的一个展示块（口径对齐 Tauri 版 `MessageSegments` 的分段渲染）。
#[derive(Clone, Debug)]
enum Seg {
    /// 正文（Markdown 渲染）
    Text(String),
    /// 思考过程（灰字，暂不折叠）
    Thinking(String),
    /// 工具调用（卡片：名称 + 参数摘要）
    Tool { name: String, args: String },
    /// 工具结果（名称 + 结果预览）
    Observation { name: String, preview: String },
}

#[derive(Clone, Debug)]
struct MsgRow {
    role: String,
    /// SharedString（Arc 语义）：clone 是 O(1)，避免每帧复制正文
    text: gpui_kit::SharedString,
    /// 结构化分段：历史消息来自 `segments`，流式增量只有正文（segs 为空，用 text 兜底）
    segs: Vec<Seg>,
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
fn parse_segments(msg: &serde_json::Value) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::new();
    let Some(arr) = msg.get("segments").and_then(|v| v.as_array()) else {
        return out;
    };
    for seg in arr {
        let ty = seg.get("type").and_then(|v| v.as_str()).unwrap_or_default();
        match ty {
            "text" => {
                if let Some(t) = seg.get("text").and_then(|v| v.as_str()) {
                    if !t.trim().is_empty() {
                        out.push(Seg::Text(t.to_string()));
                    }
                }
            }
            "thinking" | "reasoning" => {
                if let Some(t) = seg.get("text").and_then(|v| v.as_str()) {
                    if !t.trim().is_empty() {
                        out.push(Seg::Thinking(t.to_string()));
                    }
                }
            }
            "tool" => {
                // 真实结构：{ type: "tool", tool_call: { name, arguments, .. } }
                let tc = seg.get("tool_call");
                let name = tc
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("工具")
                    .to_string();
                let args = tc
                    .and_then(|v| v.get("arguments"))
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                out.push(Seg::Tool { name, args });
            }
            "observation" | "tool_result" => {
                let name = seg
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("工具")
                    .to_string();
                let preview = seg
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .chars()
                    .take(300)
                    .collect::<String>();
                out.push(Seg::Observation { name, preview });
            }
            _ => {}
        }
    }
    out
}

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
    /// 全量后台任务（会话角标聚合用；面板展示的是按当前会话过滤后的 `tasks`）
    all_tasks: Vec<TaskRow>,
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
        let messages: Rc<Vec<MsgRow>> = Rc::new(Vec::new());
        let total: usize = 0;

        let input = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("输入消息 · Enter 发送 / Shift+Enter 换行")
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
            all_tasks: Vec::new(),
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

    /// 打开「新建会话并绑定目录」对话框。
    ///
    /// 预填当前会话的工作目录——多数场景是「在同一个项目下再开一条会话」。
    fn open_new_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self
            .current_session
            .as_ref()
            .and_then(|id| self.sessions_data.iter().find(|s| &s.id == id))
            .map(|s| s.working_directory.clone())
            .unwrap_or_default();
        self.dialog_input = Some(cx.new(|cx| InputState::new(window, cx).default_value(current)));
        self.dialog = Some(Dialog::NewWorkspace);
        cx.notify();
    }

    /// 提交「新目录」：记下待绑定目录并进入新会话空态，目录在首条消息时生效。
    fn submit_new_workspace(&mut self, cx: &mut Context<Self>) {
        let path = self
            .dialog_input
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_string())
            .unwrap_or_default();
        self.dialog = None;
        self.dialog_input = None;
        if path.is_empty() {
            cx.notify();
            return;
        }
        self.new_session_workspace = Some(path.clone());
        self.start_new_session(cx);
        self.status = format!("新会话将绑定目录：{path}");
        cx.notify();
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
            Dialog::NewWorkspace => (
                "新建会话并绑定目录".to_string(),
                Some(
                    "填工作目录的绝对路径（如 F:\\work\\my-project）。\
                     该目录会成为新会话的工作区归属，并出现在左栏分组里。"
                        .to_string(),
                ),
                "新建".to_string(),
                false,
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
        // 重命名与新目录共用一个输入框实体（两个弹窗互斥，不会同时打开）
        if let (Dialog::RenameSession { .. } | Dialog::NewWorkspace, Some(input)) =
            (&dialog, self.dialog_input.as_ref())
        {
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
                                Some(Dialog::NewWorkspace) => this.submit_new_workspace(cx),
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
                let delete_title = title.clone();
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
                // 行内操作：重命名 / 删除。两个回调都 `stop_propagation()`——
                // 整行的 on_click 是「打开会话」，不拦住会把弹窗和切会话一起触发。
                head = head.child(
                    div()
                        .flex_shrink_0()
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
                        ),
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
                        cx.notify();
                    })),
            );
        }
        rail
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
        for (ix, t) in self.tasks.iter().enumerate() {
            let (dot, color) = match t.status.as_str() {
                "running" | "pending" => ("●", colors::ACCENT),
                "completed" | "success" => ("✓", colors::SUCCESS),
                "failed" | "cancelled" | "canceled" => ("✗", colors::ERROR),
                _ => ("○", colors::TEXT_TERTIARY),
            };
            panel = panel.child(
                div()
                    .id(("task", ix))
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().text_xs().text_color(rgb(color)).child(dot))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(colors::TEXT_PRIMARY))
                                    .child(ellipsis(&t.description, 26)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_TERTIARY))
                            .child(format!("{} · {}", t.kind, t.status)),
                    ),
            );
        }
        panel.id("tasks-panel")
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

    /// 未迁移入口的占位页。
    ///
    /// 明确写「未迁移 + 去 Tauri 版用」而不是留白——留白会被当成 bug 或加载失败。
    fn render_placeholder(&self, view: View, _cx: &mut Context<Self>) -> impl IntoElement {
        let label = NAV_ITEMS
            .iter()
            .find(|(_, _, v)| *v == view)
            .map(|(_, label, _)| *label)
            .unwrap_or("该页面");
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .bg(rgb(colors::BG_PRIMARY))
            .text_color(rgb(colors::TEXT_PRIMARY))
            .child(div().text_lg().child(format!("{label} · 未迁移")))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(colors::TEXT_TERTIARY))
                    .child("该页面目前仍在 Tauri 版（gui-vite）中；原生客户端按 设置 → 任务/审批 → workspace 的顺序补齐"),
            )
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
        self.load_messages(id, cx);
        self.load_session_usage(cx);
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
            compression: false,
        });
        next.push(MsgRow {
            role: "assistant".to_string(),
            text: "".into(),
            segs: Vec::new(),
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
                        .map(|m| MsgRow {
                            role: m
                                .get("role")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?")
                                .to_string(),
                            text: segments_text(m).into(),
                            segs: parse_segments(m),
                            compression: m
                                .get("compression_marker")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false),
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
                                        this.open_new_workspace(window, cx);
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
                                                gpui_list(
                                                    self.msg_list_state.clone(),
                                                    move |ix, _window, _cx| {
                                                        let Some(m) = items.get(ix) else {
                                                            return div().into_any_element();
                                                        };
                                                        // 气泡口径对齐 Tauri 版 `MessageBubble`：
                                                        //   用户 = 右对齐蓝气泡（限宽、右下小圆角）
                                                        //   助手 = **无外层框**、全宽平铺（各段独立成块，避免宽窄不一）
                                                        //   压缩点 = 徽章 + 细线（它不是用户输入，不画成气泡）
                                                        let is_user =
                                                            m.role == "user" && !m.compression;
                                                        let mut body =
                                                            div().flex().flex_col().gap_2();
                                                        if m.compression {
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
                                                        .text_xs()
                                                        .text_color(rgb(colors::TEXT_SECONDARY))
                                                        .child("压缩点"),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .h(px(1.))
                                                        .bg(rgb(colors::BORDER)),
                                                ),
                                        );
                                                        }
                                                        if m.segs.is_empty() {
                                                            if !m.text.is_empty() {
                                                                body =
                                                                    body.child(TextView::markdown(
                                                                        ("msg", ix),
                                                                        m.text.clone(),
                                                                    ));
                                                            }
                                                        } else {
                                                            for (si, seg) in
                                                                m.segs.iter().enumerate()
                                                            {
                                                                let sid = ix * 1000 + si;
                                                                body = match seg {
                                                Seg::Text(t) => body.child(TextView::markdown(
                                                    ("msg", sid),
                                                    t.clone(),
                                                )),
                                                Seg::Thinking(t) => body.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(colors::TEXT_TERTIARY))
                                                        .child(format!("🧠 {t}")),
                                                ),
                                                Seg::Tool { name, args } => body.child(
                                                    div()
                                                        .rounded_md()
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
                                                                .text_xs()
                                                                .text_color(rgb(colors::WARNING))
                                                                .child(format!("🔧 {name}")),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(rgb(
                                                                    colors::TEXT_TERTIARY,
                                                                ))
                                                                .child(ellipsis(args, 160)),
                                                        ),
                                                ),
                                                Seg::Observation { name, preview } => body.child(
                                                    div()
                                                        .rounded_md()
                                                        .bg(rgb(colors::BG_SECONDARY))
                                                        .px_3()
                                                        .py_2()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_1()
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(rgb(
                                                                    colors::TEXT_SECONDARY,
                                                                ))
                                                                .child(format!("📄 {name}")),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(rgb(
                                                                    colors::TEXT_TERTIARY,
                                                                ))
                                                                .child(ellipsis(preview, 200)),
                                                        ),
                                                ),
                                            };
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
                                                            div()
                                                                .px_4()
                                                                .py_2()
                                                                .w_full()
                                                                .child(body)
                                                                .into_any_element()
                                                        }
                                                    },
                                                )
                                                // GPUI 内置 `list` 的默认 `sizing_behavior = Auto`：它自己
                                                // **不带任何尺寸**（`request_layout` 只把空 style 交给内核），
                                                // 宽度与高度都取「内容尺寸」。实测 `on_children_prepainted`
                                                // 打印出 `w=636px h=0px` —— 高度塌成 0，`prepaint` 拿到的
                                                // bounds 高 0，于是一条消息都画不出来（症状：标题正确显示
                                                // 「会话消息（共 72 条，显示最近 60 条）」，消息区全空）。
                                                // 必须显式给尺寸；`.size_full()` = 100% x 100%，父容器
                                                // （`flex_1` 分配的高度）是确定值，百分比可解析。
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
                other => self.render_placeholder(other, cx).into_any_element(),
            }))
            .child(if self.dialog.is_some() {
                self.render_dialog(cx)
            } else {
                div().into_any_element()
            })
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

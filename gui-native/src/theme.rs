//! 外观：色板数据 + 三套主题 + 偏好持久化。
//!
//! `Palette` 是 `Copy` 的纯数据（614 处引用都是「取一个颜色值」，为它引入 trait
//! 对象不值当）；`colors` 把这些值包成函数，与 Web 版 CSS 变量一一对应。
//!
//! ⚠️ `gpui_kit::*` 里**也有**一个 `colors` 模块（gpui 自带的颜色常量）。
//! 本模块定义自己的 `colors`，所以这里的 `use gpui_kit::*` 必须放在 `mod colors`
//! 声明之后也不会互相干扰——同名项在**本模块内**以本地定义为准。

/// 界面色板（运行时可切换：设置 → 外观）。
///
/// 用 `Copy` 的纯数据而不是 trait 对象：614 处引用都是「取一个颜色值」，
/// 为它引入间接层不值当。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Palette {
    pub(crate) bg_primary: u32,
    pub(crate) bg_secondary: u32,
    pub(crate) bg_tertiary: u32,
    pub(crate) bg_hover: u32,
    pub(crate) text_primary: u32,
    pub(crate) text_secondary: u32,
    pub(crate) text_tertiary: u32,
    pub(crate) border: u32,
    pub(crate) accent: u32,
    /// 用户消息气泡底色（**独立于 accent**：accent 还要给按钮/链接做前景色，
    /// 而气泡需要「深底 + 反白字」的成对关系，借用 accent 会让浅色主题下的
    /// 白字看不见）
    pub(crate) bubble_bg: u32,
    /// 用户消息气泡文字色（与 `bubble_bg` 成对，对比度 ≥ 4.5:1）
    pub(crate) bubble_text: u32,
    pub(crate) success: u32,
    pub(crate) error: u32,
    pub(crate) warning: u32,
}

/// 石墨灰（默认）：中性深灰，不带蓝调。
pub(crate) const PALETTE_GRAPHITE: Palette = Palette {
    bg_primary: 0x18181b,
    bg_secondary: 0x1f1f23,
    bg_tertiary: 0x2b2b31,
    bg_hover: 0x26262b,
    text_primary: 0xe9e9ec,
    text_secondary: 0xa1a1a8,
    text_tertiary: 0x75757d,
    border: 0x2f2f35,
    accent: 0x60a5fa,
    // 蓝 600 + 白字（vite `bg-blue-500 text-white` 同观感）
    bubble_bg: 0x2563eb,
    bubble_text: 0xffffff,
    success: 0x22c55e,
    error: 0xf87171,
    warning: 0xeab308,
};

/// 羊皮纸：暖米黄纸面 + 深棕墨字（白天长时间阅读比深色底省眼）。
pub(crate) const PALETTE_PARCHMENT: Palette = Palette {
    bg_primary: 0xf3ead6,
    bg_secondary: 0xe9dfc6,
    bg_tertiary: 0xdccea9,
    bg_hover: 0xe4d9bf,
    text_primary: 0x3b3226,
    text_secondary: 0x6a5c48,
    text_tertiary: 0x93856e,
    border: 0xd0c2a2,
    accent: 0x8a5524,
    // 深棕底 + 米白字（≈6.8:1）。此前是 accent 底 + 主题墨字（≈1.4:1，看不清）
    bubble_bg: 0x8a5524,
    bubble_text: 0xfff8ec,
    success: 0x4a7c3f,
    error: 0xa5372a,
    warning: 0x9a6b00,
};

/// 午夜蓝（原配色）：保留给习惯旧观感的人。
pub(crate) const PALETTE_MIDNIGHT: Palette = Palette {
    bg_primary: 0x1a1a2e,
    bg_secondary: 0x16213e,
    bg_tertiary: 0x0f3460,
    bg_hover: 0x1a1a3e,
    text_primary: 0xe4e4e7,
    text_secondary: 0xa1a1aa,
    text_tertiary: 0x71717a,
    border: 0x27272a,
    accent: 0x3b82f6,
    bubble_bg: 0x2563eb,
    bubble_text: 0xffffff,
    success: 0x22c55e,
    error: 0xef4444,
    warning: 0xeab308,
};

/// 可选外观。判别值（0/1/2）被 `ACTIVE_THEME` 当作索引，**顺序不要动**。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThemeKind {
    /// 石墨灰（默认）
    Graphite,
    /// 羊皮纸
    Parchment,
    /// 午夜蓝（旧配色）
    Midnight,
}

impl ThemeKind {
    /// 全部可选外观（设置页按此顺序列出）。
    pub(crate) const ALL: [ThemeKind; 3] = [
        ThemeKind::Graphite,
        ThemeKind::Parchment,
        ThemeKind::Midnight,
    ];

    /// 稳定标识（配置文件与自动化 API 的契约值）。
    pub(crate) fn key(self) -> &'static str {
        match self {
            ThemeKind::Graphite => "graphite",
            ThemeKind::Parchment => "parchment",
            ThemeKind::Midnight => "midnight",
        }
    }

    /// 中文名（设置页展示）。
    pub(crate) fn label(self) -> &'static str {
        match self {
            ThemeKind::Graphite => "石墨灰",
            ThemeKind::Parchment => "羊皮纸",
            ThemeKind::Midnight => "午夜蓝",
        }
    }

    /// 一句话说明（设置页副标题）。
    pub(crate) fn hint(self) -> &'static str {
        match self {
            ThemeKind::Graphite => "中性深灰，长时间盯屏不偏色",
            ThemeKind::Parchment => "暖米黄纸面 + 深棕墨字，白天看更省眼",
            ThemeKind::Midnight => "原来的深蓝配色（保留）",
        }
    }

    /// 对应色板。
    pub(crate) fn palette(self) -> Palette {
        match self {
            ThemeKind::Graphite => PALETTE_GRAPHITE,
            ThemeKind::Parchment => PALETTE_PARCHMENT,
            ThemeKind::Midnight => PALETTE_MIDNIGHT,
        }
    }

    /// 由标识反查。
    pub(crate) fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.key() == key)
    }
}

/// 当前外观（**无锁原子索引**：`colors::*()` 每帧被调几百次，连读锁都嫌重）。
pub(crate) static ACTIVE_THEME: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(ThemeKind::Graphite as u8);

/// 当前色板。
pub(crate) fn palette() -> Palette {
    match ACTIVE_THEME.load(std::sync::atomic::Ordering::Relaxed) {
        1 => PALETTE_PARCHMENT,
        2 => PALETTE_MIDNIGHT,
        _ => PALETTE_GRAPHITE,
    }
}

/// 切换外观（只改变量；调用方负责 `cx.notify()` 触发重绘）。
pub(crate) fn set_theme(kind: ThemeKind) {
    ACTIVE_THEME.store(kind as u8, std::sync::atomic::Ordering::Relaxed);
}

/// 当前外观。
pub(crate) fn current_theme() -> ThemeKind {
    match ACTIVE_THEME.load(std::sync::atomic::Ordering::Relaxed) {
        1 => ThemeKind::Parchment,
        2 => ThemeKind::Midnight,
        _ => ThemeKind::Graphite,
    }
}

/// 客户端偏好文件（外观这类 GUI 侧选择）。
///
/// 不进 `tianyan.toml`：那是**服务端配置**，改它要走 `PUT /config` 热更新
/// （整个 agent 会被重建）—— 为切个主题重建 agent 显然不对。也不进 VFS：
/// 这是本机 UI 偏好，与 `todos.json` / `scheduled_agent_tasks.json` 同类，
/// 属运行期状态文件。
pub(crate) fn gui_prefs_path() -> std::path::PathBuf {
    tianyan::config::get_config()
        .storage
        .data_dir
        .join("gui_prefs.json")
}

/// 读上次选的外观（缺失/损坏一律回落默认，不报错）。
pub(crate) fn load_theme_pref() -> ThemeKind {
    std::fs::read_to_string(gui_prefs_path())
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("theme").and_then(|t| t.as_str()).map(str::to_string))
        .and_then(|k| ThemeKind::from_key(&k))
        .unwrap_or(ThemeKind::Graphite)
}

/// 写外观偏好（失败静默：丢了只是回默认主题，不值得打扰用户）。
pub(crate) fn save_theme_pref(kind: ThemeKind) {
    if let Ok(text) = serde_json::to_string_pretty(&serde_json::json!({ "theme": kind.key() })) {
        let _ = std::fs::write(gui_prefs_path(), text);
    }
}

/// 天演主题色（**运行时读全局色板**，见 [`Palette`]）。
///
/// 为什么不直接用 gpui-component 的 `Theme`：两套色板不同，混用会出现「侧栏是天演的
/// 深灰、消息区是库的灰黑」这种割裂感。这里以自有色板为单一事实源；
/// `Theme::change` 仍然保留（组件库内部控件需要它）。
///
/// 为什么是「大写函数」而不是字段访问：614 处引用都是 `colors::BG_PRIMARY()` 这种
/// 点号取值。改成 `self.palette.bg_primary` 得让每处都拿到 `self`/`cx`（渲染函数
/// 签名连带全改），为一个取色动作不值当。
pub(crate) mod colors {
    #![allow(non_snake_case)]

    use super::palette;

    /// 页面底色。
    pub fn BG_PRIMARY() -> u32 {
        palette().bg_primary
    }
    /// 侧栏 / 次级面。
    pub fn BG_SECONDARY() -> u32 {
        palette().bg_secondary
    }
    /// 三级面（选中块 / 强调块）。
    pub fn BG_TERTIARY() -> u32 {
        palette().bg_tertiary
    }
    /// hover 底色。
    pub fn BG_HOVER() -> u32 {
        palette().bg_hover
    }
    /// 主文字。
    pub fn TEXT_PRIMARY() -> u32 {
        palette().text_primary
    }
    /// 次文字。
    pub fn TEXT_SECONDARY() -> u32 {
        palette().text_secondary
    }
    /// 弱文字。
    pub fn TEXT_TERTIARY() -> u32 {
        palette().text_tertiary
    }
    /// 分隔线。
    pub fn BORDER() -> u32 {
        palette().border
    }
    /// 强调色。
    pub fn ACCENT() -> u32 {
        palette().accent
    }
    /// 用户消息气泡底色。
    pub fn BUBBLE_BG() -> u32 {
        palette().bubble_bg
    }
    /// 用户消息气泡文字色（与 `BUBBLE_BG` 成对）。
    pub fn BUBBLE_TEXT() -> u32 {
        palette().bubble_text
    }
    /// 成功。
    pub fn SUCCESS() -> u32 {
        palette().success
    }
    /// 错误。
    pub fn ERROR() -> u32 {
        palette().error
    }
    /// 警告。
    pub fn WARNING() -> u32 {
        palette().warning
    }
    /// 一级导航栏宽度（`--sidebar-collapsed-width`：64px 图标 rail）
    pub const RAIL_WIDTH: f32 = 64.;
    /// 会话列表宽度（`--sidebar-width`：280px）
    pub const LIST_WIDTH: f32 = 280.;
    /// 会话内容列宽度（Tailwind `max-w-4xl` = 896px）：消息列、底部状态条与输入卡片
    /// 三者必须同宽同左边界——`gui-vite` 的 `ChatPanel` 用 `w-full max-w-4xl mx-auto px-4`，
    /// `ChatInput` 用 `mx-auto max-w-4xl`，靠同一个 max-width 对齐。
    pub const CHAT_MAX_W: f32 = 896.;
}

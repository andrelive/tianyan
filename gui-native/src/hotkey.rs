//! 全局快捷键（D3）：`Ctrl+Alt+T` 切换主窗口显隐。
//!
//! ## 为什么自己 `RegisterHotKey`
//!
//! `global-hotkey` 这类 crate 的 Windows 后端同样是 `RegisterHotKey` + 一个窗口
//! 消息循环，只是把注册、id 分配、回调包装了一遍。本功能只需要一个组合键、一个
//! 动作，直接用系统 API 更少一层不可控因素——而且与 `tray.rs` 用同一个思路
//! （独立线程 + 自己的消息循环），两条系统集成的失败模式因此一致、好排查。
//!
//! ## 为什么线程里直接动窗口
//!
//! `tray::hide_main_window` / `show_main_window` 是纯 Win32（`ShowWindow`），
//! 不碰 GPUI 状态，任何线程都能调。热键线程直接调它们是**最短路径**：不引入
//! channel、不依赖主线程的消息循环是否在跑（窗口隐藏时 GPUI 可能不派发帧）。
//!
//! ## 组合键选择
//!
//! `Ctrl+Alt+T` 与 tauri 版**同一个组合**（`tauri/src/lib.rs` 的全局快捷键）。
//! 两个客户端同时运行时必然有一个注册失败——这是正确行为（一个组合键只能属于
//! 一个进程），日志里会说明。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// 是否注册成功（供自动化断言）。
static REGISTERED: AtomicBool = AtomicBool::new(false);

/// 默认组合键：与 tauri 版同一个组合（`tauri/src/lib.rs` 的全局快捷键），
/// 这样 gui-vite 退役后不用改用户习惯。
const DEFAULT_COMBO: &str = "Ctrl+Alt+T";

/// 覆盖组合键的环境变量名。
///
/// 存在的理由有两个，都不是"为了测试而加开关"：
/// 1. tauri 版与 gui-native **并存期**都注册 `Ctrl+Alt+T` 时，必然有一个失败——
///    冲突环境里能换一个组合做端到端验证；
/// 2. 用户机器上这个组合可能被别的软件占用（注册失败时日志会说明），
///    可配置才谈得上"能解决"。
const COMBO_ENV: &str = "TIANYAN_GUI_HOTKEY";

/// 解析后的组合键（`mods` / `vk` 直接喂 `RegisterHotKey`）。
static COMBO_LABEL: OnceLock<String> = OnceLock::new();

/// `RegisterHotKey` 的标识（进程内唯一即可）。
const HOTKEY_ID: i32 = 0x7A11;

/// 热键是否已注册（被别的程序占用时为 false）。
pub fn is_registered() -> bool {
    REGISTERED.load(Ordering::SeqCst)
}

/// 当前生效的组合键（人读形态，如 `Ctrl+Alt+T`；自动化断言与日志共用）。
pub fn combo() -> &'static str {
    COMBO_LABEL
        .get()
        .map(|s| s.as_str())
        .unwrap_or(DEFAULT_COMBO)
}

/// 启动全局快捷键监听（独立线程；失败只记日志）。
pub fn spawn() {
    std::thread::Builder::new()
        .name("tianyan-hotkey".to_string())
        .spawn(|| {
            if let Err(e) = run() {
                tracing::warn!("全局快捷键不可用：{e}");
            }
        })
        .map(|_| ())
        .unwrap_or_else(|e| tracing::warn!("快捷键线程启动失败：{e}"));
}

fn run() -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
        MOD_SHIFT, MOD_WIN,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

    let raw = std::env::var(COMBO_ENV).unwrap_or_else(|_| DEFAULT_COMBO.to_string());
    let (mod_bits, vk) = parse_combo(&raw)?;
    let _ = COMBO_LABEL.set(raw);
    let mods: HOT_KEY_MODIFIERS = HOT_KEY_MODIFIERS(mod_bits | MOD_NOREPEAT.0);
    // 引用一次，避免"导入了但没用"（它们只在 parse_combo 的位运算里按需出现）
    let _ = (MOD_CONTROL, MOD_ALT, MOD_SHIFT, MOD_WIN);

    // SAFETY: `RegisterHotKey(None, ...)` 把热键投递到**当前线程**的消息队列，
    // 没有窗口句柄参与；失败返回值化处理，不涉及内存。
    unsafe {
        RegisterHotKey(None, HOTKEY_ID, mods, vk)
            .map_err(|e| format!("RegisterHotKey 失败（组合键可能已被其他程序占用）：{e}"))?;
    }
    REGISTERED.store(true, Ordering::SeqCst);
    tracing::info!("全局快捷键已注册：{}（切换主窗口显隐）", combo());

    let mut msg = MSG::default();
    // SAFETY: 标准消息循环；`msg` 由 GetMessage 写入并立即读取，不跨线程共享。
    // `GetMessageW` 阻塞（不像托盘那样需要轮询 channel），热键是窗口消息，
    // 阻塞等待正是想要的：空闲时零 CPU。
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_HOTKEY && msg.wParam.0 as i32 == HOTKEY_ID {
                toggle_main_window();
            }
        }
        let _ = UnregisterHotKey(None, HOTKEY_ID);
    }
    REGISTERED.store(false, Ordering::SeqCst);
    Ok(())
}

/// 解析 `Ctrl+Alt+T` / `ctrl+shift+f5` 形态的组合键 → `(mods 位, 虚拟键码)`。
///
/// 只认修饰键前缀 + 一个主键；主键支持单字母（A-Z）、单数字（0-9）与 F1-F24。
/// 解析失败给出可读原因（会进日志），不做静默兜底——兜底成别的组合比报错更糟。
fn parse_combo(s: &str) -> Result<(u32, u32), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};

    let mut mods = 0u32;
    let mut vk: Option<u32> = None;
    for part in s.split('+').map(|p| p.trim().to_ascii_lowercase()) {
        match part.as_str() {
            "" => {}
            "ctrl" | "control" => mods |= MOD_CONTROL.0,
            "alt" => mods |= MOD_ALT.0,
            "shift" => mods |= MOD_SHIFT.0,
            "win" | "super" | "meta" => mods |= MOD_WIN.0,
            other => {
                if vk.is_some() {
                    return Err(format!("组合键里有多个主键（{s}）"));
                }
                let mut chars = other.chars();
                let (first, rest) = (chars.next(), chars.as_str());
                if rest.is_empty() && first.is_some_and(|c| c.is_ascii_alphanumeric()) {
                    // 单字母/数字：ASCII 大写即虚拟键码
                    let c = first.unwrap_or_default().to_ascii_uppercase();
                    vk = Some(c as u32);
                } else if let Some(n) = other.strip_prefix('f').and_then(|x| x.parse::<u32>().ok()) {
                    if (1..=24).contains(&n) {
                        vk = Some(0x70 + n - 1); // VK_F1 = 0x70
                    } else {
                        return Err(format!("功能键超出 F1-F24（{other}）"));
                    }
                } else {
                    return Err(format!("无法识别的按键：{other}"));
                }
            }
        }
    }
    let vk = vk.ok_or_else(|| format!("组合键缺少主键（如 Ctrl+Alt+T 的 T）：{s}"))?;
    if mods == 0 {
        return Err(format!("组合键至少需要一个修饰键（Ctrl/Alt/Shift/Win）：{s}"));
    }
    Ok((mods, vk))
}

/// 切换主窗口显隐（可见 → 隐藏；隐藏/最小化 → 显示并聚焦）。
pub fn toggle_main_window() {
    if crate::tray::is_main_window_visible() {
        crate::tray::hide_main_window();
    } else {
        crate::tray::show_main_window();
    }
}

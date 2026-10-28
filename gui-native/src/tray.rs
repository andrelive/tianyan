//! 系统托盘（D2）：托盘图标 + 右键菜单 + 窗口显隐。
//!
//! ## 为什么是一个独立线程
//!
//! `tray-icon` 的 Windows 后端要求「创建托盘的线程上有 win32 消息循环」，但**不必
//! 是主线程**（官方 README + issue #198 明确：自己 `PeekMessage` 泵消息即可）。
//! GPUI 独占主线程的消息循环且不暴露 hook，所以这里起一条自己的线程：建托盘 →
//! 泵消息 → 把「用户点了哪个菜单项」通过 channel 送回主线程。主线程只负责执行
//! 动作（动窗口、退出），不碰任何 Win32。
//!
//! ## 窗口显隐为什么不用 GPUI 的 API
//!
//! GPUI 只暴露 `minimize_window` / `activate_window`，**没有 hide/show**。而
//! 「关闭到托盘」的标准语义是窗口从任务栏消失（不是最小化）。所以这里
//! `EnumWindows` 按本进程 PID 找到主窗口，直接 `ShowWindow(SW_HIDE/SW_SHOW)`。
//! 找窗口按 PID + 可见 + 有标题三个条件，不按标题字符串匹配（改标题就失效）。
//!
//! `unsafe` 只出现在本文件的三个 Win32 边界（枚举窗口、显隐窗口、泵消息），
//! 其余逻辑是纯 Rust。
//!
//! ## 关闭到托盘
//!
//! `Window::on_window_should_close` 的回调返回 `false` 就能阻止关闭。配合
//! [`is_quitting`] 标志：用户点托盘「退出天演」时置位，回调放行；否则隐藏到托盘。

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use tokio::sync::mpsc::UnboundedSender;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::TrayIconBuilder;

/// 托盘图标（PNG 内嵌进二进制：便携版/安装版都能拿到，不依赖安装目录里有图片）。
const TRAY_ICON_PNG: &[u8] = include_bytes!("../assets/tray-32.png");

/// 「真的要退出」标志：`on_window_should_close` 据此区分「关闭到托盘」与「退出」。
static QUITTING: AtomicBool = AtomicBool::new(false);

/// 主窗口句柄缓存（`HWND` 的 isize 形态；0 = 尚未找到）。
///
/// **必须缓存**：枚举时用 `IsWindowVisible` 过滤（否则会把同进程的隐藏辅助窗口
/// 也算进来），但窗口一旦被 `SW_HIDE` 隐藏，`IsWindowVisible` 就是 false——
/// 不缓存的话「隐藏之后再显示」找不到窗口，窗口再也回不来（实测踩到）。
/// 命中缓存后只校验 `IsWindow`（句柄还活着），不看可见性。
static MAIN_WINDOW: AtomicIsize = AtomicIsize::new(0);

/// 托盘菜单动作（主线程执行）。
pub enum TrayCommand {
    /// 显示并聚焦主窗口
    Show,
    /// 隐藏主窗口（从任务栏消失，只留托盘图标）
    Hide,
    /// 退出应用（放行窗口关闭）
    Quit,
}

/// 是否处于「正在退出」状态。
pub fn is_quitting() -> bool {
    QUITTING.load(Ordering::SeqCst)
}

/// 标记退出（托盘「退出天演」→ 主线程放行关闭）。
pub fn begin_quit() {
    QUITTING.store(true, Ordering::SeqCst);
}

/// 启动托盘（在独立线程建图标 + 泵消息）。
///
/// 失败只记日志：托盘是增强项，装不上不该让应用起不来（例如资源服务器策略
/// 禁了 Explorer 的通知区域）。
pub fn spawn(tx: UnboundedSender<TrayCommand>) {
    std::thread::Builder::new()
        .name("tianyan-tray".to_string())
        .spawn(move || {
            if let Err(e) = run(tx) {
                tracing::warn!("托盘初始化失败：{e}");
            }
        })
        .map(|_| ())
        .unwrap_or_else(|e| tracing::warn!("托盘线程启动失败：{e}"));
}

/// 托盘线程主体：建图标 → 循环泵消息 → 把菜单动作送回主线程。
fn run(tx: UnboundedSender<TrayCommand>) -> Result<(), String> {
    let menu = Menu::new();
    let show = MenuItem::new("显示主窗口", true, None);
    let hide = MenuItem::new("隐藏到托盘", true, None);
    let quit = MenuItem::new("退出天演", true, None);
    menu.append_items(&[
        &show,
        &hide,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .map_err(|e| format!("构建托盘菜单失败：{e}"))?;

    // `_tray` 必须活到循环结束（drop 即移除图标），所以绑在函数作用域
    let _tray = TrayIconBuilder::new()
        .with_tooltip("天演 —— 右键显示菜单")
        .with_icon(load_icon()?)
        .with_menu(Box::new(menu))
        // 左键单击直接显示主窗口（最常见的操作，不该要多点一次菜单）
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| format!("创建托盘图标失败：{e}"))?;

    loop {
        pump_messages();

        // tray-icon 的事件走它自己的 channel（不是窗口消息），所以这里显式取
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let cmd = if event.id == show.id() {
                Some(TrayCommand::Show)
            } else if event.id == hide.id() {
                Some(TrayCommand::Hide)
            } else if event.id == quit.id() {
                Some(TrayCommand::Quit)
            } else {
                None
            };
            if let Some(cmd) = cmd {
                // 主线程已退出（channel 关闭）→ 本线程也该结束
                if tx.send(cmd).is_err() {
                    return Ok(());
                }
            }
        }

        // 30ms 轮询：菜单点击是低频交互，62Hz 的 CPU 代价没必要；
        // 又不能让延迟被用户感知（30ms 远低于可察觉阈值）。
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

/// 泵当前线程的 win32 消息队列（`PM_REMOVE`），把消息派发给托盘/菜单的隐藏窗口。
///
/// 不阻塞（`PeekMessage` 而非 `GetMessage`）：托盘/菜单事件在 channel 里，
/// 阻塞在消息队列上会漏掉它们（issue #198 的结论）。
fn pump_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };

    // SAFETY: 标准 win32 消息泵；`msg` 是栈上零初始化结构，只被 PeekMessage 写入，
    // 随后立即被 Translate/Dispatch 消费。不跨线程共享，无别名借用。
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 解码内嵌 PNG → `tray_icon::Icon`（要求 RGBA8）。
fn load_icon() -> Result<tray_icon::Icon, String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(TRAY_ICON_PNG));
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("托盘图标解码失败：{e}"))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("托盘图标读取失败：{e}"))?;
    let rgba = &buf[..info.buffer_size()];

    // tray-icon 要 RGBA8；源图若不是 RGBA（如 RGB/灰度），逐像素补齐 alpha=255。
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => rgba.to_vec(),
        png::ColorType::Rgb => rgba
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Grayscale => rgba.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => rgba
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Indexed => {
            return Err("托盘图标不能是索引色（请用 RGBA/RGB PNG）".to_string())
        }
    };

    tray_icon::Icon::from_rgba(rgba, info.width, info.height)
        .map_err(|e| format!("创建托盘图标失败：{e}"))
}

/// 取本进程主窗口：优先用缓存，缓存失效（窗口重建/关闭）才重新枚举。
pub fn find_main_window() -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;

    let cached = MAIN_WINDOW.load(Ordering::SeqCst);
    if cached != 0 {
        let hwnd = HWND(cached as *mut std::ffi::c_void);
        // SAFETY: 句柄值来自上次成功的枚举；`IsWindow` 对失效句柄只返回 false，
        // 不解引用已释放内存。
        if unsafe { IsWindow(Some(hwnd)).as_bool() } {
            return Some(hwnd);
        }
        MAIN_WINDOW.store(0, Ordering::SeqCst);
    }

    let found = enumerate_main_window()?;
    MAIN_WINDOW.store(found.0 as isize, Ordering::SeqCst);
    Some(found)
}

/// 枚举本进程的主窗口（可见 + 有标题的第一个顶层窗口，不做缓存）。
///
/// 按 **PID** 而不是标题字符串：改窗口标题不该让托盘功能失效。
fn enumerate_main_window() -> Option<windows::Win32::Foundation::HWND> {
    // windows 0.62：`BOOL` 已从 `Win32::Foundation` 移到 crate 根的 `core`
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible,
    };

    struct Search {
        pid: u32,
        found: Option<HWND>,
    }

    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` 是下面传入的 `&mut Search` 的地址，调用期间有效；
        // 回调由 EnumWindows 在同一线程同步调用，不存在别名借用。
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut pid = 0u32;
        // SAFETY: 标准签名；`pid` 是栈上变量，函数只写不读其他内存。
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        if pid == search.pid
            // SAFETY: hwnd 来自 EnumWindows，是当前有效的窗口句柄
            && unsafe { IsWindowVisible(hwnd).as_bool() }
            // SAFETY: 同上
            && unsafe { GetWindowTextLengthW(hwnd) } > 0
        {
            search.found = Some(hwnd);
            return BOOL(0); // 找到即停
        }
        BOOL(1) // 继续枚举
    }

    let mut search = Search {
        pid: std::process::id(),
        found: None,
    };
    // SAFETY: 回调与 lparam 生命周期覆盖整次枚举（同步调用）；失败仅表示没找到。
    unsafe {
        let _ = EnumWindows(Some(callback), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

/// 隐藏主窗口（从任务栏消失，只留托盘图标）。
pub fn hide_main_window() {
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};

    if let Some(hwnd) = find_main_window() {
        // SAFETY: hwnd 是本进程的有效窗口；SW_HIDE 不涉及内存所有权。
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

/// 显示并聚焦主窗口（托盘「显示主窗口」/ 双击图标）。
pub fn show_main_window() {
    use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_SHOW};

    if let Some(hwnd) = find_main_window() {
        // SAFETY: 同上；SW_SHOW 让窗口恢复可见，SetForegroundWindow 尝试置前
        // （系统可能拒绝前台切换，返回值忽略即可——窗口至少已可见）。
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// 托盘是否可用（供自动化断言；找不到主窗口也算不可用）。
pub fn is_available() -> bool {
    find_main_window().is_some()
}

/// 主窗口当前是否可见（供自动化断言"关闭到托盘"确实藏起来了）。
pub fn is_main_window_visible() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;

    match find_main_window() {
        // SAFETY: 句柄来自 `find_main_window`（已过 `IsWindow` 校验）
        Some(hwnd) => unsafe { IsWindowVisible(hwnd).as_bool() },
        None => false,
    }
}

/// 向主窗口投递 `WM_CLOSE` —— **等价于用户点标题栏的关闭按钮**。
///
/// 给自动化端口用：验证「关闭 → 藏到托盘（进程不退）」这条真实路径。
/// 不用 `Window::remove_window()`：那是强制移除，会绕过
/// `on_window_should_close` 的拦截逻辑，验证不出任何东西。
pub fn post_close() {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

    if let Some(hwnd) = find_main_window() {
        // SAFETY: hwnd 已校验；PostMessage 只投递消息，不涉及内存所有权
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}


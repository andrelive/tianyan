//! 滚动保位诊断（可开关）。
//!
//! 为什么落文件：安装版没有控制台，用户复现「上滚被拽回底部」时唯一拿得到的
//! 证据就是一份文件；`println!` 只对开发者的终端可见。
//!
//! 为什么**默认开着**：这类缺陷只能由用户在自己的时序里撞上，而「装好、复现、再把
//! 日志交出来」是这个循环里最贵的一段。让诊断常开（只在状态**变化**时写、超过
//! 2 MB 轮转一次）比每次都要用户配环境变量可靠。要关掉：`TIANYAN_GUI_SCROLL_DIAG=0`。
//!
//! 产物：`%LOCALAPPDATA%\tianyan\logs\gui-scroll.log`（与服务端日志同目录）。
//!
//! 判读口径（对齐 `chat-scroll-position-recovery` 的四条验收）：
//!
//! - `[保位] frame ...` 只在**任一字段变化**时落一行：看「用户没操作、`item_ix`
//!   却跳到最后一条」的那一帧，前后两行即是现场。
//! - `scrolled_up=true` 表示用户滚离底部（标志来自引擎的 `is_following_tail`）。
//!   它为 false 而用户明明滚上去了 ⇒ 标志判定失效。
//! - `count` 变化 = 流式新开了一条消息（`splice`）；紧跟着的位置跳变即是那条路径。
//! - `viewport_h` 变化 = 列表可用高度变了（横幅/待办条/输入卡片高度变化），
//!   贴底位置会跟着重算 —— 「空了半个屏幕」多半能在这一列看到突变。
//! - `[保位] autoscroll <file>:<line>` 记录**谁**在跟着滚底（`#[track_caller]`
//!   拿到的真实调用点），用来抓「第二处无条件贴底」这类重复写者。

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;

fn path() -> Option<&'static PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        if std::env::var("TIANYAN_GUI_SCROLL_DIAG").ok().as_deref() == Some("0") {
            return None;
        }
        let base = std::env::var("LOCALAPPDATA").ok()?;
        let dir = std::path::Path::new(&base).join("tianyan").join("logs");
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir.join("gui-scroll.log"))
    })
    .as_ref()
}

/// 诊断是否开启（默认开；`TIANYAN_GUI_SCROLL_DIAG=0` 关闭）。
pub(crate) fn on() -> bool {
    path().is_some()
}

/// 单文件上限：超过就先把它挪成 `.1`（覆盖上一次的），再继续写。
///
/// 常开必然要限体积：一个长会话里滚动事件是几万行量级，不轮转会把这个文件写成
/// 几百 MB —— 那和「不记日志」一样没用，还白占用户磁盘。
const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// 落一行诊断（未开启时是空操作）。
pub(crate) fn log(line: impl AsRef<str>) {
    let Some(p) = path() else { return };
    if std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) > MAX_BYTES {
        let mut rotated = p.clone();
        rotated.set_extension("log.1");
        let _ = std::fs::rename(p, rotated);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        let _ = writeln!(f, "{}", line.as_ref());
    }
}

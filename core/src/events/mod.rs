//! 事件驱动触发核心（T1 路线：文件监听 + webhook → 事件总线 → 规则动作）。
//!
//! - [`EventBus`]：进程内发布/订阅总线（tokio mpsc unbounded，可 Clone）。
//! - [`FileWatcher`]：用 notify 监控目录，把文件系统事件映射为 [`Event`] 发布。
//! - [`EventRule`] / [`parse_rules`]：事件匹配规则（路径 glob / `webhook:<名称>`）。
//!
//! server 侧在此之上消费事件：`POST /api/v1/events` 发布 `Event::Webhook`，
//! 事件处理器按规则触发任务或唤醒会话（装配由集成者完成）。

mod bus;
mod rules;
mod types;
mod watcher;

pub use bus::EventBus;
pub use rules::{parse_rules, EventAction, EventRule};
pub use types::Event;
pub use watcher::FileWatcher;

//! 事件驱动 API（T1 路线：webhook 接入 + 事件消费处理器）。
//!
//! - `handlers`：`POST /api/v1/events` webhook 接入（令牌校验 → 发布
//!   `Event::Webhook` 到事件总线）。
//! - `processor`：事件消费处理器（订阅总线，按规则触发任务 / 唤醒会话）。
//! - `routes`：路由挂载。
//!
//! 装配说明（由集成者完成）：
//! - 创建 `Arc<EventBus>`，经 [`routes::routes`] 挂载事件路由；
//! - webhook 令牌从共享配置（`config.events.webhook_token`）读取，
//!   handler 在请求时经 `AppState` 获取，无需启动期设置；
//! - 创建 [`processor::ProcessorDeps`] 并调用 [`processor::EventProcessor::start`]。

use std::sync::Arc;

use tianyan::events::EventBus;

/// 事件路由状态：共享事件总线。
pub type EventBusState = Arc<EventBus>;

pub mod handlers;
pub mod processor;
pub mod routes;

pub use routes::routes;

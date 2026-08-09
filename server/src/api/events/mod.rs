//! 事件驱动 API（T1 路线：webhook 接入 + 事件消费处理器）。
//!
//! - `handlers`：`POST /api/v1/events` webhook 接入（令牌校验 → 发布
//!   `Event::Webhook` 到事件总线）。
//! - `processor`：事件消费处理器（订阅总线，按规则触发任务 / 唤醒会话）。
//! - `routes`：路由与 `build_router`（注入事件总线后可直接 merge 进主路由）。
//!
//! 装配说明（由集成者完成）：
//! - 创建 `Arc<EventBus>`，调用 [`routes::build_router`] 挂载事件路由；
//! - 调用 [`handlers::set_webhook_token`] 设置令牌（配置 `events.webhook_token`）；
//! - 创建 [`processor::ProcessorDeps`] 并调用 [`processor::EventProcessor::start`]。

use std::sync::Arc;

use tianyan::events::EventBus;

/// 事件路由状态：共享事件总线。
pub type EventBusState = Arc<EventBus>;

pub mod handlers;
pub mod processor;
pub mod routes;

pub use routes::routes;

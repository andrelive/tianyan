//! 剪贴板 I/O API。
//!
//! 链路：tauri 监听循环捕获系统剪贴板 → `POST /clipboard/capture`（配置门控，
//! auto_capture 直接沉淀或存 pending）→ 前端确认条 `POST /clipboard/respond`
//! （remember → 记忆 / knowledge → 知识库 / ignore）；反向：agent
//! `clipboard_write` 动态工具 → outbox → `GET /clipboard/outbox` → tauri 写回系统剪贴板。

pub mod handlers;
pub mod routes;
pub mod services;
pub mod tool;
pub mod types;

#[cfg(test)]
mod tests;

pub use routes::routes;

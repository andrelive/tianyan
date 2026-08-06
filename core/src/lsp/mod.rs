//! LSP（Language Server Protocol）集成模块。
//!
//! 自实现的轻量级 LSP 客户端栈，基于 tokio 进程管道与 `Content-Length` 分帧：
//!
//! - [`registry`]：多语言服务器注册表（`ServerSpec`）+ 项目根探测
//! - [`client`]：JSON-RPC 2.0 客户端（进程管道 / 内存流双通道，便于测试）
//! - [`diagnostics`]：`LspManager` —— 按项目根的服务池 + 推送诊断存储
//!
//! 设计要点：
//! - 服务器缺失/损坏时优雅降级为错误（"executor: lsp: 服务器 X 不可用：…，安装提示：…"），
//!   绝不阻塞其他工具执行；
//! - 所有请求带 10s 超时；
//! - 诊断通过 `textDocument/publishDiagnostics` 推送实时入库，
//!   编辑类工具可将 `"diagnostics": [...]` 附加到结果（"LSP errors detected, please fix"）。

pub mod client;
pub mod diagnostics;
pub mod registry;

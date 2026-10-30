//! 一级入口的页面渲染（每个入口一个文件）。
//!
//! 统一约定：
//! - `render_<page>` 返回 `impl IntoElement`，只读 `ClientApp` 的状态，不发请求；
//! - `load_<page>` 系列负责拉接口（各自 `cx.spawn`），写完状态后 `cx.notify()`；
//! - 页面之间**不互相调用**——共享的构件下沉到 `widgets.rs`，共享的格式化下沉到
//!   `format.rs`，跨页面的异步状态（任务/审批/待办）在 `tasks.rs`。
//!
//! 这条约定是拆分的收益所在：改一个页面时不需要读别的页面。

pub(crate) mod approval;
pub(crate) mod insights;
pub(crate) mod knowledge;
pub(crate) mod memory;
pub(crate) mod roles;
pub(crate) mod scheduled;
pub(crate) mod settings;
pub(crate) mod setup;
pub(crate) mod skills;
pub(crate) mod tools;
pub(crate) mod workspace;

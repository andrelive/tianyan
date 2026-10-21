//! storage-vector 插件：LanceDB 嵌入式向量存储（`VectorStorage` 实现）。
//!
//! ## 为什么独立成 crate（ADR-049 期 2 序 3）
//!
//! 这是**收益最大**的一次搬移：`lancedb` → `arrow` → `datafusion` 依赖链是首次
//! 全量 debug 构建的最大耗时源（>8 分钟）。搬进插件后 core 不再承担它——只改
//! 提示词 / 工具 / 调度逻辑的增量构建不必再等这条链。
//!
//! ## 契约仍在 core
//!
//! `VectorStorage` trait（`tianyan::vfs::VectorStorage`）与向量数据结构
//! （`tianyan::vfs::{VectorPoint, VectorSearchQuery, VectorSearchResult, VectorType}`）
//! **留在 core**：它们是契约（VFS 依赖 trait 而非实现），不是实现。core 因此
//! 不反向依赖本 crate，VFS 在无向量实现时也能构建（装配层注入空实现）。
//!
//! ## 装配：类型擦除的工厂（`VectorStorageFactory`）
//!
//! server 库不认识本 crate：装配层（`app` bin / `tauri` 桌面端）把
//! [`LanceDbVectorStore::new`] 包成 `tianyan_server::VectorStorageFactory`
//! （`Arc<dyn Fn(&StorageConfig) -> Pin<Box<dyn Future<Output = Result<Arc<dyn
//! VectorStorage>>>>`>）注入启动入口——与本 crate 的依赖只出现在装配层，
//! 改一行向量实现不会重编 server 库（闸门 A）。
//!
//! ## examples：一次性运维工具
//!
//! `examples/` 下的四个工具（`memory_cleanup` / `memory_path_repair` /
//! `rebuild_overviews` / `skill_import`）原本在 core，因装配 LanceDB 而随之
//! 搬家（core 已无向量实现）；它们只依赖 core 的公开 API + 本 crate 的
//! [`LanceDbVectorStore`]。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod lancedb;

pub use lancedb::LanceDbVectorStore;

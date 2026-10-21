//! 向量存储 —— `VectorStorage` 契约（实现已迁至 `plugins/storage-vector`）。
//!
//! 本模块只留**契约**：`VectorStorage` trait（VFS 依赖它而非具体实现）。
//! 具体实现（LanceDB 嵌入式向量库）在 `tianyan-storage-vector` 插件里——core
//! 因此不再承担 `lancedb` / `arrow` / `datafusion` 这条重依赖链（ADR-049
//! 期 2 序 3）。装配层（bin / 桌面端）把实现包成
//! `tianyan_server::VectorStorageFactory` 注入启动入口。

mod traits;

pub use traits::VectorStorage;

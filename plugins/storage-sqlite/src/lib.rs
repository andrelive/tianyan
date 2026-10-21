//! storage-sqlite 插件：VFS 结构化存储后端（`StorageBackend` 的两个 adapter）。
//!
//! ## 为什么独立成 crate（ADR-049 期 2 序 4）
//!
//! `vfs/backend/{sqlite,local}.rs` 是 VFS 的**存储实现**（SQL 语句、UPSERT 语义、
//! 前缀删除/列举、文件写入锁），不是契约。搬进插件后改后端行为（表结构、时间戳
//! 解析、目录语义）只重编本 crate + 装配包，`tianyan-core`（49K 源码）与
//! `tianyan-server`（13.4K）不动——与 `tools-code` / `tools-web` / `storage-vector`
//! 同一判据（ADR-049 D7：按「变化原因」切）。
//!
//! ## 契约仍在 core
//!
//! [`tianyan::vfs::StorageBackend`] trait 留在 core（公开路径 `vfs::StorageBackend`
//! 不变）：VFS 依赖 trait 而非实现，`VirtualFileSystemImpl` 只认
//! `Arc<dyn StorageBackend>`。core 因此**不反向依赖本 crate**（D2-1/D2-3）。
//! `UriMapper` / `StorageConfig` / `ContextEntry` / `EntryMetadata` 同样是 core
//! 的公开类型（URI→路径映射、配置、条目契约），实现随插件、契约留内核。
//!
//! ## `rusqlite` 搬不走：本序收窄的是**重编面积**，不是依赖面
//!
//! `rusqlite` + 共享连接（`tianyan::db::Database`）仍在 core——`core/src/db/*` 与
//! `core/src/session/*` 有 80 处 rusqlite 用法，`Database` 门面与 4 个 Repository
//! 是 ADR-020 的**全系统写入门面**，`SessionStore` 更是 ADR-018 的会话唯一权威。
//! 所以本插件**不试图**把 rusqlite 从 core 的依赖表里摘掉（那需要先拆 ADR-020）
//! ——这是序 3（lancedb/arrow/datafusion 整体改道）与序 4 性质上的分水岭：
//! 序 3 收窄依赖面，序 4 收窄重编面积。
//!
//! 同理，`SqliteBackend` 必须把 SQL 打在**同一根连接**上（ADR-005：禁止第二个
//! SQLite 连接），因此本 crate 直接依赖 `rusqlite` 是刻意的，而不是漏删。
//!
//! ## 装配：类型擦除的工厂（`StorageBackendFactory`）
//!
//! server 库不认识本 crate：装配层（`app` bin / `tauri` 桌面端）把
//! [`SqliteBackend::new`] / [`LocalFileBackend::new`] 包成
//! `tianyan_server::StorageBackendFactory`
//! （`Arc<dyn Fn(&StorageConfig, Arc<Database>) -> Result<Arc<dyn StorageBackend>>>`）
//! 注入启动入口。`config.storage.backend`（`Local` / `Sqlite`）的选择语义因此落在
//! **装配层**（ADR-049 D5），不在 server 库里。
//!
//! ## `vfs/backend/sqlite_db.rs` 为什么留在 core
//!
//! 它只是 `crate::db::sqlite_db` 的 re-export 兼容层（一行 `pub use`），而 `db`
//! 模块（共享连接 + schema 集中初始化）**必须在 core**（ADR-020）。把它搬进本
//! 插件只会让 core 反向依赖插件，正是 D2 要防的循环。
//!
//! ## ⚠️ 源码纪律：只用 `tianyan::…` 绝对路径，禁止 `crate::…`
//!
//! core 的单元测试需要构造**真实后端**（`evolution_task::real_vfs()` 刻意用
//! `SqliteBackend` 锁定生产条目形态——退化成文件后端/桩会重演「Mock 形态掩盖
//! 缺陷」的事故）。而「core 的 `[dev-dependencies]` 指向本 crate」**行不通**：
//! core 会被编译两份（`cfg(test)` 的测试单元 + 本 crate 依赖的普通 lib），
//! `StorageBackend` trait 与 `StorageConfig` / `Arc<Database>` 随之出现两个互不
//! 兼容的实例，单测无法把 core 的类型传进本 crate 的 API（实测 22 处 E0308/E0277）。
//!
//! 所以 core 在 `#[cfg(test)]` 下用 `#[path]` **同源包含**本 crate 的
//! `src/{sqlite,local}.rs`（唯一副本仍在这里，正常构建完全不参与；见
//! `core/src/vfs/backend/mod.rs` 的注释）。由此产生一条**硬纪律**：
//!
//! > 这两个文件及其 `#[cfg(test)] mod tests` 内只允许 `tianyan::…` 绝对路径，
//! > 不得出现 `crate::…`。
//!
//! 同源包含时 `crate::` 会解析到 **core**（而非本 crate），编译错误会指向
//! 完全不相干的位置，极难定位。`#[path]` 的路径也硬编码在 core 侧，改动本 crate
//! 的文件布局须同步（否则 core 测试直接编译失败——这就是守卫）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod local;
mod sqlite;

pub use local::LocalFileBackend;
pub use sqlite::SqliteBackend;

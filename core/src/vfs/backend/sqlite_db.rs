//! SQLite 数据库连接（**已移至 [`crate::db::sqlite_db`]**）。
//!
//! 本模块仅作 re-export 兼容（历史引用 `crate::vfs::backend::sqlite_db::SqliteDb` 仍可用）；
//! 新代码直接引用 [`crate::db::SqliteDb`]。移动原因：数据库是全系统基础设施（ADR-005），
//! 归位 `db` 层消除 `db → vfs` 依赖（SqliteBackend 经 Database 访问，单向）。

pub use crate::db::sqlite_db::*;

//! 存储后端 —— 本地文件系统和 SQLite 适配器。

mod local;
mod sqlite;

pub use local::LocalFileBackend;
pub use sqlite::SqliteBackend;

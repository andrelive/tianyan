//! 统一写入门面（Database Repository 层）。
//!
//! **单连接 + 唯一结构化存储入口**（ADR-005：禁止第二个 SQLite 连接）：
//! - [`Database`] 持有 [`SqliteDb`]（共享 `Arc<Mutex<Connection>>`），提供兼容的
//!   [`Database::lock`]——业务组件改为依赖门面，不再各自持 SqliteDb；
//! - schema 集中初始化（[`Database::init_schemas`] 委托 [`SqliteDb::init_all_schemas`]）；
//! - 后续按域拆 Repository（session/stats/trace/execution/usage/vfs），
//!   SQL 逐步从组件收敛到门面下的仓储方法。

use std::path::PathBuf;
use std::sync::Arc;

pub mod session;
pub mod stats;
pub mod trace;

use crate::common::error::{Result, TianyanError};
use crate::vfs::backend::sqlite_db::SqliteDb;

/// 统一数据库门面。
#[derive(Clone)]
pub struct Database {
    /// 共享单连接（ADR-005：全系统唯一 SQLite 连接）。
    sqlite: SqliteDb,
}

impl Database {
    /// 打开（或创建）数据库（schema 由调用方 [`Database::init_schemas`] 初始化）。
    pub fn open(path: PathBuf) -> Result<Arc<Self>> {
        let sqlite = SqliteDb::open(path)
            .map_err(|e| TianyanError::Custom(format!("db: 打开数据库失败：{e}")))?;
        Ok(Arc::new(Self { sqlite }))
    }

    /// 打开内存数据库（测试用）。
    pub fn open_in_memory() -> Result<Arc<Self>> {
        let sqlite = SqliteDb::open_in_memory()
            .map_err(|e| TianyanError::Custom(format!("db: 打开内存数据库失败：{e}")))?;
        Ok(Arc::new(Self { sqlite }))
    }

    /// 初始化全部 schema（启动时调用一次；集中管理入口）。
    pub async fn init_schemas(&self) -> Result<()> {
        self.sqlite
            .init_all_schemas()
            .await
            .map_err(|e| TianyanError::Custom(format!("db: 初始化 schema 失败：{e}")))
    }

    /// 底层连接（统一访问点；与 SqliteDb.lock 语义一致，组件迁移后方法不变）。
    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, rusqlite::Connection> {
        self.sqlite.lock().await
    }

    /// 非阻塞获取连接引用（热路径组件用，锁不可用时跳过）。
    pub fn try_lock(&self) -> std::result::Result<tokio::sync::MutexGuard<'_, rusqlite::Connection>, tokio::sync::TryLockError> {
        self.sqlite.try_lock()
    }

    /// 数据库文件路径。
    pub fn path(&self) -> &PathBuf {
        self.sqlite.path()
    }
}

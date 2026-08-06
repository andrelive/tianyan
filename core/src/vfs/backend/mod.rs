//! 存储后端 —— 本地文件系统和 SQLite 适配器。
//!
//! [`StorageBackend`] 是 VFS 结构化存储的 adapter seam（ADR-005）：
//! `LocalFileBackend`（本地文件系统）与 `SqliteBackend`（SQLite）两个 adapter
//! 共享同一接口，`VirtualFileSystemImpl` 通过 `Arc<dyn StorageBackend>` 操作，
//! 不感知具体后端。两个 adapter 的存在使该 seam 为真实 seam。

use async_trait::async_trait;

use crate::common::error::Result;
use crate::common::types::{ContentLevel, TianyanUri};
use crate::vfs::types::ContextEntry;

/// 结构化存储后端 —— VFS 内容存储的 adapter seam。
///
/// 接口与 LocalFileBackend/SqliteBackend 的固有方法一一对应，
/// 新增后端只需实现本 trait 即可接入 VFS。
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// 初始化后端（创建目录/表结构）。
    async fn initialize(&self) -> Result<()>;

    /// 检查条目是否存在。
    async fn exists(&self, uri: &TianyanUri) -> Result<bool>;

    /// 读取条目。
    async fn read_entry(&self, uri: &TianyanUri) -> Result<ContextEntry>;

    /// 写入条目。
    async fn write_entry(&self, entry: &ContextEntry) -> Result<()>;

    /// 删除条目及其子条目。
    async fn delete_entry(&self, uri: &TianyanUri) -> Result<()>;

    /// 列出目录的直接子条目。
    async fn list_directory(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>>;

    /// 读取指定层级的内容。
    async fn read_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String>;

    /// 写入指定层级的内容。
    async fn write_content(
        &self,
        uri: &TianyanUri,
        level: ContentLevel,
        content: &str,
    ) -> Result<()>;

    /// 追加内容到指定层级末尾。
    async fn append_content(
        &self,
        uri: &TianyanUri,
        level: ContentLevel,
        content: &str,
    ) -> Result<()>;
}

mod local;
mod sqlite;
/// 共享 SQLite 连接（VFS 与 UsageStats 共用，ADR-005 单连接）。
pub mod sqlite_db;

pub use local::LocalFileBackend;
pub use sqlite::SqliteBackend;

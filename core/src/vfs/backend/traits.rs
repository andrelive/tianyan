//! 存储后端 trait。

use async_trait::async_trait;

use crate::common::error::Result;
use crate::common::types::{ContentLevel, TianyanUri};
use crate::vfs::types::ContextEntry;

/// 存储后端 trait。
///
/// 定义持久化存储的核心操作，解耦具体存储介质（本地文件、云存储等）。
/// `LocalFileBackend` 是默认适配器。
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// 初始化存储后端（创建必要的目录结构）。
    async fn initialize(&self) -> Result<()>;

    /// 检查条目是否存在。
    async fn exists(&self, uri: &TianyanUri) -> Result<bool>;

    /// 读取条目。
    async fn read_entry(&self, uri: &TianyanUri) -> Result<ContextEntry>;

    /// 写入条目。
    async fn write_entry(&self, entry: &ContextEntry) -> Result<()>;

    /// 删除条目。
    async fn delete_entry(&self, uri: &TianyanUri) -> Result<()>;

    /// 列出指定 URI 下的子条目。
    async fn list_directory(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>>;

    /// 读取指定层级的内容。
    async fn read_content(
        &self,
        uri: &TianyanUri,
        level: ContentLevel,
    ) -> Result<String>;

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

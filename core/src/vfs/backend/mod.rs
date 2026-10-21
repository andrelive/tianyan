//! 存储后端契约 —— VFS 结构化存储的 adapter seam。
//!
//! [`StorageBackend`] 是 VFS 结构化存储的 adapter seam（ADR-005）：
//! `VirtualFileSystemImpl` 通过 `Arc<dyn StorageBackend>` 操作，不感知具体后端。
//!
//! **实现已出 core（ADR-049 期 2 序 4）**：`LocalFileBackend`（本地文件系统）与
//! `SqliteBackend`（SQLite）两个 adapter 整文件迁入 `plugins/storage-sqlite`，
//! 由装配层经 `StorageBackendFactory` 注入。本模块只留契约——契约留内核、实现
//! 进插件，core 因此不反向依赖插件（D2）。两个 adapter（在插件内共享本 trait）
//! 的存在仍使该 seam 为真实 seam。
//!
//! [`sqlite_db`] 留在这里是**兼容 re-export**（指向 [`crate::db::sqlite_db`]）：
//! 共享连接与 schema 集中在 `db` 模块（ADR-005/020），搬走只会造成 core →
//! 插件的反向依赖。

use async_trait::async_trait;

use crate::common::error::Result;
use crate::common::types::{ContentLevel, TianyanUri};
use crate::vfs::types::ContextEntry;

/// 结构化存储后端 —— VFS 内容存储的 adapter seam。
///
/// 接口与 `LocalFileBackend` / `SqliteBackend`（`plugins/storage-sqlite`）的固有
/// 方法一一对应，新增后端只需实现本 trait 即可接入 VFS。
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

/// 共享 SQLite 连接（VFS 与 UsageStats 共用，ADR-005 单连接）。
pub mod sqlite_db;

// ── 测试期同源包含：实现的**唯一副本仍在插件里** ───────────────────────────
//
// core 的单元测试（`cargo test -p tianyan-core --lib`）必须能构造真实后端：
// `vfs::test_utils::create_test_vfs`、`vfs_tests` 的两条「生产同构后端」用例、
// 角色存储的三处 tempdir VFS，以及 `scheduler::tasks::evolution_task` 的
// `real_vfs()`（它刻意用 SqliteBackend 锁定生产条目形态——退化成文件后端/桩会
// 重演「Mock 形态掩盖缺陷」那类事故）。
//
// 为什么不能靠 `[dev-dependencies] tianyan-storage-sqlite`：单测要**把 core 的
// 类型（`StorageConfig` / `Arc<Database>` / `dyn StorageBackend`）传给插件 API**，
// 而 `cfg(test)` 的 core 与插件依赖的 core 是**两个编译单元**（同一个 crate 的两
// 个实例），类型与 trait 都不等价——直接编译失败（实测）。
//
// 所以测试期用 `#[path]` 把插件源码**同源包含**进来：一份源码、零复制、零漂移，
// 正常构建（`cargo build`/`cargo check`/发布）完全不参与——core 的生产依赖图里
// 没有任何插件，重编面积收益不受影响。
//
// 判别力说明：插件自带同一批 `#[cfg(test)] mod tests`（14 个），在插件 crate 里
// 也照常跑一遍（`cargo test -p tianyan-storage-sqlite`）；此处只是让 core 的单测
// 也能拿到**同一个实现**。
#[cfg(test)]
#[path = "../../../../plugins/storage-sqlite/src/local.rs"]
mod local;
#[cfg(test)]
#[path = "../../../../plugins/storage-sqlite/src/sqlite.rs"]
mod sqlite;

/// 本地文件系统存储后端（**仅测试期**；生产实现见 `plugins/storage-sqlite`）。
#[cfg(test)]
pub use local::LocalFileBackend;
/// SQLite 存储后端（**仅测试期**；生产实现见 `plugins/storage-sqlite`）。
#[cfg(test)]
pub use sqlite::SqliteBackend;

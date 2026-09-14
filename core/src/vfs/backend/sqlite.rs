//! SQLite 存储后端 —— 替代 LocalFileBackend。
//!
//! 所有 VFS 条目内容存储在 SQLite 的 `vfs_entries` 表中，
//! 使用 URI 作为主键。

use async_trait::async_trait;

use std::sync::Arc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContentLevel, EntryMetadata, TianyanUri};
use crate::db::Database;
use crate::vfs::backend::StorageBackend;
use crate::vfs::types::{ContextEntry, CURRENT_SCHEMA_VERSION};

/// 基于 SQLite 的 VFS 存储后端。
#[derive(Clone)]
pub struct SqliteBackend {
    db: Arc<Database>,
}

/// 解析存储层时间戳字符串（兼容 SQLite `datetime('now')` 格式与 RFC3339）。
///
/// SQLite `datetime('now')` 产出 `YYYY-MM-DD HH:MM:SS`（UTC，秒级），
/// 并非 RFC3339——此前仅按 RFC3339 解析，失败后条目时间退回
/// `EntryMetadata::new` 的构造时刻（`Utc::now()`），导致时间信息失真。
fn parse_stored_timestamp(ts: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(ts) {
        return Some(t.with_timezone(&chrono::Utc));
    }
    chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

impl SqliteBackend {
    /// 使用共享的 SqliteDb 创建后端。
    /// Schema 应已由 `SqliteDb::init_all_schemas()` 创建。
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    // ── VFS 表扩展 Schema ─────────────────────────────────────────────────
    //   在现有 init_all_schemas 基础上追加 vfs_entries 表。
    //   如果 SqliteDb 尚未包含此表，调用此方法。

    /// 确保 vfs_entries 表存在。
    pub async fn ensure_schema(&self) -> Result<()> {
        let conn = self.db.lock().await;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS vfs_entries (
                uri              TEXT PRIMARY KEY,
                is_directory     INTEGER DEFAULT 0,
                abstract_content TEXT,
                overview_content TEXT,
                detail_content   TEXT,
                created_at       TEXT DEFAULT (datetime('now')),
                updated_at       TEXT DEFAULT (datetime('now'))
            );",
        )
        .map_err(|e| TianyanError::Custom(format!("存储后端错误：创建 vfs_entries 表失败: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl StorageBackend for SqliteBackend {
    /// 初始化 SQLite VFS 后端，确保 vfs_entries 表存在。
    async fn initialize(&self) -> Result<()> {
        self.ensure_schema().await?;
        tracing::info!("SQLite VFS 后端已初始化");
        Ok(())
    }

    /// 检查指定 URI 的条目是否存在。
    async fn exists(&self, uri: &TianyanUri) -> Result<bool> {
        let conn = self.db.lock().await;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM vfs_entries WHERE uri = ?1",
                rusqlite::params![uri.to_string()],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(count > 0)
    }

    /// 读取指定 URI 的完整条目（含 L0/L1/L2 三层内容）。
    async fn read_entry(&self, uri: &TianyanUri) -> Result<ContextEntry> {
        let conn = self.db.lock().await;
        let uri_str = uri.to_string();

        conn.query_row(
            "SELECT is_directory, abstract_content, overview_content, detail_content,
                    created_at, updated_at
             FROM vfs_entries WHERE uri = ?1",
            rusqlite::params![&uri_str],
            |row| {
                let is_dir: bool = row.get::<_, i32>(0)? != 0;
                let created_at: Option<String> = row.get(4)?;
                let updated_at: Option<String> = row.get(5)?;
                let mut metadata = EntryMetadata::new(uri.clone(), "unknown");
                metadata.is_directory = is_dir;
                if let Some(ref ts) = created_at {
                    if let Some(t) = parse_stored_timestamp(ts) {
                        metadata.created_at = t;
                    }
                }
                if let Some(ref ts) = updated_at {
                    if let Some(t) = parse_stored_timestamp(ts) {
                        metadata.updated_at = t;
                    }
                }
                Ok(ContextEntry {
                    schema_version: CURRENT_SCHEMA_VERSION,
                    abstract_content: row.get(1)?,
                    overview_content: row.get(2)?,
                    detail_content: row.get(3)?,
                    metadata,
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => TianyanError::not_found(uri_str.clone()),
            other => TianyanError::Custom(format!("存储后端错误：读取条目失败: {other}")),
        })
    }

    /// 写入完整的条目内容（UPSERT 语义）。
    async fn write_entry(&self, entry: &ContextEntry) -> Result<()> {
        let conn = self.db.lock().await;
        let uri = entry.uri().to_string();
        let is_dir = entry.metadata.is_directory as i32;

        conn.execute(
            "INSERT INTO vfs_entries
             (uri, is_directory, abstract_content, overview_content, detail_content, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
             ON CONFLICT(uri) DO UPDATE SET
                is_directory = excluded.is_directory,
                abstract_content = excluded.abstract_content,
                overview_content = excluded.overview_content,
                detail_content = excluded.detail_content,
                updated_at = excluded.updated_at",
            rusqlite::params![
                uri,
                is_dir,
                entry.abstract_content,
                entry.overview_content,
                entry.detail_content,
            ],
        )
        .map_err(|e| TianyanError::Custom(format!("存储后端错误：写入条目失败: {e}")))?;
        Ok(())
    }

    /// 删除指定 URI 及其所有子条目。
    ///
    /// 使用 `substr` 做精确前缀匹配（而非 LIKE）：LIKE 会把 URI 中的
    /// `%`/`_` 当作通配符，且 `uri%` 会误删同前缀的兄弟条目
    /// （如删除 `.../ab` 时连带删除 `.../abc`）。
    ///
    /// 长度用 SQL 侧 `length()`（按字符计）而非 Rust 的 `str::len()`（按字节）：
    /// 两者单位不同，URI 含非 ASCII 字符时字节数会大于字符数 → 前缀匹配失效。
    async fn delete_entry(&self, uri: &TianyanUri) -> Result<()> {
        let conn = self.db.lock().await;
        let uri_str = uri.to_string();
        // 子条目前缀：URI + "/"，仅匹配直接或间接子条目。
        let dir_prefix = format!("{}/", uri_str);

        let deleted = conn
            .execute(
                "DELETE FROM vfs_entries WHERE uri = ?1 OR substr(uri, 1, length(?2)) = ?2",
                rusqlite::params![&uri_str, &dir_prefix],
            )
            .map_err(|e| TianyanError::Custom(format!("存储后端错误：删除条目失败: {e}")))?;

        if deleted == 0 {
            return Err(TianyanError::not_found(uri_str));
        }
        Ok(())
    }

    /// 列出指定 URI 下的一级子条目（基于 SQL 前缀匹配过滤）。
    ///
    /// 前缀匹配用 `substr + length`（按字符计）而非 `LIKE`：LIKE 会把 URI
    /// 里的 `%`/`_` 当通配符，可能返回不属于该前缀的条目（与
    /// [`Self::delete_entry`] 同一口径——长度一律由 SQL 侧计算）。
    async fn list_directory(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>> {
        let conn = self.db.lock().await;
        let prefix = format!("{}/", uri);
        // 只列出一级子条目：匹配 prefix，且 prefix 之后不含 '/'

        let mut stmt = conn
            .prepare(
                "SELECT uri, is_directory, abstract_content, overview_content, detail_content,
                        created_at, updated_at
                 FROM vfs_entries WHERE substr(uri, 1, length(?1)) = ?1",
            )
            .map_err(|e| TianyanError::Custom(format!("存储后端错误：查询目录失败: {e}")))?;

        let prefix_len = prefix.len();

        let rows = stmt
            .query_map(rusqlite::params![&prefix], |row| {
                let full_uri: String = row.get(0)?;
                // 过滤掉二级及以下的子条目
                let rest = &full_uri[prefix_len..];
                if rest.contains('/') {
                    // 二级子条目，跳过
                    return Ok(None);
                }
                let is_dir: bool = row.get::<_, i32>(1)? != 0;
                let parsed = TianyanUri::parse(&full_uri).unwrap_or_else(|_| {
                    // fallback: 从 full_uri 构造，可能丢失部分信息但不会 panic
                    TianyanUri::new(
                        crate::common::types::ContextNamespace::Knowledge,
                        vec![full_uri.clone()],
                    )
                });
                let mut metadata = EntryMetadata::new(parsed, "unknown");
                metadata.is_directory = is_dir;
                let created_at: Option<String> = row.get(5)?;
                let updated_at: Option<String> = row.get(6)?;
                if let Some(ref ts) = created_at {
                    if let Some(t) = parse_stored_timestamp(ts) {
                        metadata.created_at = t;
                    }
                }
                if let Some(ref ts) = updated_at {
                    if let Some(t) = parse_stored_timestamp(ts) {
                        metadata.updated_at = t;
                    }
                }
                Ok(Some(ContextEntry {
                    schema_version: CURRENT_SCHEMA_VERSION,
                    abstract_content: row.get(2)?,
                    overview_content: row.get(3)?,
                    detail_content: row.get(4)?,
                    metadata,
                }))
            })
            .map_err(|e| TianyanError::Custom(format!("存储后端错误：遍历目录失败: {e}")))?;

        Ok(rows.filter_map(|r| r.ok().flatten()).collect())
    }

    /// 读取指定 URI 的某一层级内容（L0/L1/L2）。
    ///
    /// 缺失语义与 local 后端对齐：行不存在或该层列为 NULL（从未写入）均返回
    /// `not_found`——`ContextEntry` 各层本就是 `Option<String>`，缺层是合法状态；
    /// 调用方可据此区分"无内容"（如 search_vfs 展示为空）与"存储故障"。
    async fn read_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String> {
        let conn = self.db.lock().await;
        let column = match level {
            ContentLevel::Abstract => "abstract_content",
            ContentLevel::Overview => "overview_content",
            ContentLevel::Detail => "detail_content",
        };
        let sql = format!("SELECT {} FROM vfs_entries WHERE uri = ?1", column);

        let value: Option<String> = conn
            .query_row(&sql, rusqlite::params![uri.to_string()], |row| row.get(0))
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    TianyanError::not_found(format!("{uri} 无 {level:?} 层级内容"))
                }
                other => TianyanError::Custom(format!("存储后端错误：读取内容失败: {other}")),
            })?;
        value.ok_or_else(|| TianyanError::not_found(format!("{uri} 无 {level:?} 层级内容")))
    }

    /// 写入指定 URI 的某一层级内容（UPSERT 语义）。
    async fn write_content(
        &self,
        uri: &TianyanUri,
        level: ContentLevel,
        content: &str,
    ) -> Result<()> {
        let conn = self.db.lock().await;
        let column = match level {
            ContentLevel::Abstract => "abstract_content",
            ContentLevel::Overview => "overview_content",
            ContentLevel::Detail => "detail_content",
        };
        let sql = format!(
            "INSERT INTO vfs_entries (uri, {0}, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(uri) DO UPDATE SET {0} = excluded.{0}, updated_at = excluded.updated_at",
            column
        );

        conn.execute(&sql, rusqlite::params![uri.to_string(), content])
            .map_err(|e| TianyanError::Custom(format!("存储后端错误：写入内容失败: {e}")))?;
        Ok(())
    }

    /// 追加内容到指定 URI 的某一层级（利用 SQL COALESCE 拼接）。
    async fn append_content(
        &self,
        uri: &TianyanUri,
        level: ContentLevel,
        content: &str,
    ) -> Result<()> {
        let conn = self.db.lock().await;
        let column = match level {
            ContentLevel::Abstract => "abstract_content",
            ContentLevel::Overview => "overview_content",
            ContentLevel::Detail => "detail_content",
        };
        let sql = format!(
            "INSERT INTO vfs_entries (uri, {0}, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(uri) DO UPDATE SET
                {0} = COALESCE(vfs_entries.{0}, '') || excluded.{0},
                updated_at = excluded.updated_at",
            column
        );

        conn.execute(&sql, rusqlite::params![uri.to_string(), content])
            .map_err(|e| TianyanError::Custom(format!("存储后端错误：追加内容失败: {e}")))?;
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::ContextNamespace;
    use crate::db::Database;

    async fn setup() -> SqliteBackend {
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        let backend = SqliteBackend::new(db);
        backend.ensure_schema().await.unwrap();
        backend
    }

    fn knowledge_uri(path: &str) -> TianyanUri {
        TianyanUri::new(ContextNamespace::Knowledge, vec![path.to_string()])
    }

    #[tokio::test]
    async fn test_initialize_and_exists() {
        let be = setup().await;
        let uri = knowledge_uri("test.md");
        assert!(!be.exists(&uri).await.unwrap());

        let entry = ContextEntry::new_file(uri.clone());
        be.write_entry(&entry).await.unwrap();
        assert!(be.exists(&uri).await.unwrap());
    }

    #[tokio::test]
    async fn test_write_read_entry() {
        let be = setup().await;
        let uri = knowledge_uri("guide.md");
        let mut entry = ContextEntry::new_file(uri.clone());
        entry.abstract_content = Some("摘要".into());
        entry.overview_content = Some("概览".into());
        entry.detail_content = Some("详情".into());

        be.write_entry(&entry).await.unwrap();
        let read = be.read_entry(&uri).await.unwrap();

        assert_eq!(read.abstract_content, Some("摘要".into()));
        assert_eq!(read.overview_content, Some("概览".into()));
        assert_eq!(read.detail_content, Some("详情".into()));
    }
    /// 回归：读取"条目存在但该层从未写入（列为 NULL）"→ not_found（与 local
    /// 后端"缺层"语义对齐），而不是把 NULL 当存储故障。
    /// 判别力：修复前 NULL 列 `row.get::<String>` 报 `Invalid column type Null`
    /// （Custom 错误）→ `is_not_found()` 断言必红。
    #[tokio::test]
    async fn test_read_content_null_level_is_not_found() {
        let be = setup().await;
        let uri = knowledge_uri("partial.md");
        let mut entry = ContextEntry::new_file(uri.clone());
        entry.detail_content = Some("仅详情".into());
        be.write_entry(&entry).await.unwrap();

        // 未写入的层（NULL）→ not_found，而非存储故障
        let err_overview = be
            .read_content(&uri, ContentLevel::Overview)
            .await
            .unwrap_err();
        assert!(
            err_overview.is_not_found(),
            "NULL 层应报 not_found（缺层为合法状态），实际: {err_overview}"
        );
        let err_abstract = be
            .read_content(&uri, ContentLevel::Abstract)
            .await
            .unwrap_err();
        assert!(err_abstract.is_not_found());

        // 已写入的层正常读取
        assert_eq!(
            be.read_content(&uri, ContentLevel::Detail).await.unwrap(),
            "仅详情"
        );

        // 行不存在 → 同为 not_found（语义一致）
        let missing = knowledge_uri("missing.md");
        let err_missing = be
            .read_content(&missing, ContentLevel::Detail)
            .await
            .unwrap_err();
        assert!(err_missing.is_not_found());
    }

    #[tokio::test]
    async fn test_delete_entry() {
        let be = setup().await;
        let uri = knowledge_uri("del.md");
        be.write_entry(&ContextEntry::new_file(uri.clone()))
            .await
            .unwrap();
        be.delete_entry(&uri).await.unwrap();
        assert!(!be.exists(&uri).await.unwrap());
    }

    /// 回归测试：删除条目不得误删同前缀的兄弟条目，但应级联删除子条目。
    #[tokio::test]
    async fn test_delete_entry_prefix_collision() {
        let be = setup().await;
        let parent = knowledge_uri("ab");
        let sibling = knowledge_uri("abc");
        let child = TianyanUri::new(
            ContextNamespace::Knowledge,
            vec!["ab".to_string(), "x.md".to_string()],
        );
        for uri in [&parent, &sibling, &child] {
            be.write_entry(&ContextEntry::new_file(uri.clone()))
                .await
                .unwrap();
        }

        // 删除父条目：应保留同前缀兄弟 "abc"，删除子条目 "ab/x.md"
        be.delete_entry(&parent).await.unwrap();

        assert!(!be.exists(&parent).await.unwrap(), "父条目应被删除");
        assert!(!be.exists(&child).await.unwrap(), "子条目应被级联删除");
        assert!(
            be.exists(&sibling).await.unwrap(),
            "同前缀兄弟条目不应被误删"
        );
    }

    #[tokio::test]
    async fn test_list_directory() {
        let be = setup().await;
        for name in &["a", "b", "c"] {
            let uri = TianyanUri::new(
                ContextNamespace::Knowledge,
                vec!["dir".to_string(), name.to_string()],
            );
            be.write_entry(&ContextEntry::new_file(uri)).await.unwrap();
        }
        let parent = TianyanUri::new(ContextNamespace::Knowledge, vec!["dir".to_string()]);
        // Parent may not exist as a separate row, but list_directory should still work
        be.write_entry(&ContextEntry::new_file(parent.clone()))
            .await
            .unwrap();
        let entries = be.list_directory(&parent).await.unwrap();
        assert_eq!(entries.len(), 3);
    }
    /// 回归测试：目录列举按**精确前缀**匹配——URI 中的 `_` 不得被当作
    /// LIKE 通配符。
    ///
    /// 旧实现 `uri LIKE 'prefix/%'` 会把 `_` 解释为「任意单字符」，
    /// 于是 `a_b/` 的列举结果会混入 `axb/` 下的条目。
    #[tokio::test]
    async fn test_list_directory_exact_prefix_underscore() {
        let be = setup().await;
        let target = TianyanUri::new(
            ContextNamespace::Knowledge,
            vec!["a_b".to_string(), "one.md".to_string()],
        );
        let decoy = TianyanUri::new(
            ContextNamespace::Knowledge,
            vec!["axb".to_string(), "two.md".to_string()],
        );
        for uri in [&target, &decoy] {
            be.write_entry(&ContextEntry::new_file(uri.clone()))
                .await
                .unwrap();
        }

        let parent = knowledge_uri("a_b");
        let entries = be.list_directory(&parent).await.unwrap();
        assert_eq!(entries.len(), 1, "只应列出 a_b 下的一级子条目");
        assert!(
            entries[0].metadata.uri.to_string().contains("a_b/one.md"),
            "列举结果误含通配符匹配到的兄弟目录条目：{}",
            entries[0].metadata.uri
        );
    }

    /// 回归测试：含非 ASCII 的 URI 前缀删除不得受「字节长度 vs 字符长度」
    /// 差异影响（SQL `substr`/`length` 按字符计，Rust `str::len()` 按字节）。
    #[tokio::test]
    async fn test_delete_entry_non_ascii_prefix() {
        let be = setup().await;
        let parent = knowledge_uri("中文目录");
        let child = TianyanUri::new(
            ContextNamespace::Knowledge,
            vec!["中文目录".to_string(), "c.md".to_string()],
        );
        for uri in [&parent, &child] {
            be.write_entry(&ContextEntry::new_file(uri.clone()))
                .await
                .unwrap();
        }

        be.delete_entry(&parent).await.unwrap();
        assert!(!be.exists(&parent).await.unwrap(), "父条目应被删除");
        assert!(
            !be.exists(&child).await.unwrap(),
            "非 ASCII 前缀下的子条目应被级联删除（长度单位须一致）"
        );
    }

    #[tokio::test]
    async fn test_append_content() {
        let be = setup().await;
        let uri = knowledge_uri("log.md");
        be.write_content(&uri, ContentLevel::Detail, "line1\n")
            .await
            .unwrap();
        be.append_content(&uri, ContentLevel::Detail, "line2\n")
            .await
            .unwrap();

        let content = be.read_content(&uri, ContentLevel::Detail).await.unwrap();
        assert_eq!(content, "line1\nline2\n");
    }

    #[tokio::test]
    async fn test_read_nonexistent_entry() {
        let be = setup().await;
        let uri = knowledge_uri("nope.md");
        let err = be.read_entry(&uri).await.unwrap_err();
        assert!(err.to_string().contains("未找到"));
    }
}

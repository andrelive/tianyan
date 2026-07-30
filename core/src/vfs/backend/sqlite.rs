//! SQLite 存储后端 —— 替代 LocalFileBackend。
//!
//! 所有 VFS 条目内容存储在 SQLite 的 `vfs_entries` 表中，
//! 使用 URI 作为主键。

use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContentLevel, EntryMetadata, TianyanUri};
use crate::vfs::types::{ContextEntry, CURRENT_SCHEMA_VERSION};

use crate::observability::sqlite_db::SqliteDb;

/// 基于 SQLite 的 VFS 存储后端。
#[derive(Clone)]
pub struct SqliteBackend {
    db: SqliteDb,
}

impl SqliteBackend {
    /// 使用共享的 SqliteDb 创建后端。
    /// Schema 应已由 `SqliteDb::init_all_schemas()` 创建。
    pub fn new(db: SqliteDb) -> Self {
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

    // ── 公共 API（与 LocalFileBackend 同签名）────────────────────────────

    /// 初始化 SQLite VFS 后端，确保 vfs_entries 表存在。
    pub async fn initialize(&self) -> Result<()> {
        self.ensure_schema().await?;
        tracing::info!("SQLite VFS 后端已初始化");
        Ok(())
    }

    /// 检查指定 URI 的条目是否存在。
    pub async fn exists(&self, uri: &TianyanUri) -> Result<bool> {
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
    pub async fn read_entry(&self, uri: &TianyanUri) -> Result<ContextEntry> {
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
                    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(ts) {
                        metadata.created_at = t.with_timezone(&chrono::Utc);
                    }
                }
                if let Some(ref ts) = updated_at {
                    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(ts) {
                        metadata.updated_at = t.with_timezone(&chrono::Utc);
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
            rusqlite::Error::QueryReturnedNoRows => TianyanError::Custom(format!("条目未找到：{uri_str}")),
            other => TianyanError::Custom(format!("存储后端错误：读取条目失败: {other}")),
        })
    }

    /// 写入完整的条目内容（UPSERT 语义）。
    pub async fn write_entry(&self, entry: &ContextEntry) -> Result<()> {
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
    pub async fn delete_entry(&self, uri: &TianyanUri) -> Result<()> {
        let conn = self.db.lock().await;
        let uri_str = uri.to_string();
        let pattern = format!("{}%", uri_str);

        let deleted = conn
            .execute(
                "DELETE FROM vfs_entries WHERE uri = ?1 OR uri LIKE ?2",
                rusqlite::params![&uri_str, &pattern],
            )
            .map_err(|e| TianyanError::Custom(format!("存储后端错误：删除条目失败: {e}")))?;

        if deleted == 0 {
            return Err(TianyanError::Custom(format!("条目未找到：{uri_str}")));
        }
        Ok(())
    }

    /// 列出指定 URI 下的一级子条目（基于 SQL LIKE 过滤）。
    pub async fn list_directory(&self, uri: &TianyanUri) -> Result<Vec<ContextEntry>> {
        let conn = self.db.lock().await;
        let prefix = format!("{}/", uri.to_string());
        // 只列出一级子条目：匹配 prefix，且 prefix 之后不含 '/'
        let pattern = format!("{}%", prefix);

        let mut stmt = conn
            .prepare(
                "SELECT uri, is_directory, abstract_content, overview_content, detail_content,
                        created_at, updated_at
                 FROM vfs_entries WHERE uri LIKE ?1",
            )
            .map_err(|e| TianyanError::Custom(format!("存储后端错误：查询目录失败: {e}")))?;

        let prefix_len = prefix.len();

        let rows = stmt
            .query_map(rusqlite::params![&pattern], |row| {
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
                    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(ts) {
                        metadata.created_at = t.with_timezone(&chrono::Utc);
                    }
                }
                if let Some(ref ts) = updated_at {
                    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(ts) {
                        metadata.updated_at = t.with_timezone(&chrono::Utc);
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
    pub async fn read_content(&self, uri: &TianyanUri, level: ContentLevel) -> Result<String> {
        let conn = self.db.lock().await;
        let column = match level {
            ContentLevel::Abstract => "abstract_content",
            ContentLevel::Overview => "overview_content",
            ContentLevel::Detail => "detail_content",
        };
        let sql = format!("SELECT {} FROM vfs_entries WHERE uri = ?1", column);

        conn.query_row(&sql, rusqlite::params![uri.to_string()], |row| row.get(0))
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    TianyanError::Custom(format!("条目未找到：{uri} 无 {level:?} 层级内容"))
                }
                other => TianyanError::Custom(format!("存储后端错误：读取内容失败: {other}")),
            })
    }

    /// 写入指定 URI 的某一层级内容（UPSERT 语义）。
    pub async fn write_content(
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
    pub async fn append_content(
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
    use crate::observability::sqlite_db::SqliteDb;

    async fn setup() -> SqliteBackend {
        let db = SqliteDb::open_in_memory().unwrap();
        db.init_all_schemas().await.unwrap();
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

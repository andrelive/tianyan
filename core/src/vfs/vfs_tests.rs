use super::VirtualFileSystemImpl;
use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::vfs::test_utils::create_test_vfs;
use crate::vfs::{ContentStore, VfsCore, VfsSearch};

#[tokio::test]
async fn test_vfs_initialize() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();
}

#[tokio::test]
async fn test_vfs_create_directory() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let uri = TianyanUri::new(ContextNamespace::User, vec!["test_dir".to_string()]);
    let entry = vfs.create_directory(&uri).await.unwrap();

    assert!(entry.is_directory());
    assert!(vfs.exists(&uri).await.unwrap());
}

#[tokio::test]
async fn test_vfs_create_file() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let uri = TianyanUri::new(
        ContextNamespace::User,
        vec!["test_dir".to_string(), "test_file".to_string()],
    );
    let entry = vfs.create_file(&uri).await.unwrap();

    assert!(!entry.is_directory());
    assert!(vfs.exists(&uri).await.unwrap());
}

#[tokio::test]
async fn test_vfs_write_read_content() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);
    vfs.create_file(&uri).await.unwrap();

    vfs.write(&uri, ContentLevel::Detail, "测试详情")
        .await
        .unwrap();

    let detail_content = vfs.read(&uri, ContentLevel::Detail).await.unwrap();
    assert_eq!(detail_content, "测试详情");
}

#[tokio::test]
async fn test_vfs_delete() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);
    vfs.create_file(&uri).await.unwrap();

    assert!(vfs.exists(&uri).await.unwrap());

    vfs.delete(&uri).await.unwrap();
    assert!(!vfs.exists(&uri).await.unwrap());
}

#[tokio::test]
async fn test_vfs_list() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let parent_uri = TianyanUri::new(ContextNamespace::User, vec!["parent".to_string()]);
    vfs.create_directory(&parent_uri).await.unwrap();

    for i in 0..3 {
        let child_uri = parent_uri.append(&format!("child_{}", i));
        vfs.create_file(&child_uri).await.unwrap();
    }

    let entries = vfs.list(&parent_uri).await.unwrap();
    assert_eq!(entries.len(), 3);
}

#[tokio::test]
async fn test_vfs_move() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let source = TianyanUri::new(ContextNamespace::User, vec!["source".to_string()]);
    let dest = TianyanUri::new(ContextNamespace::User, vec!["dest".to_string()]);

    vfs.create_file(&source).await.unwrap();
    vfs.write(&source, ContentLevel::Detail, "测试内容")
        .await
        .unwrap();

    vfs.move_entry(&source, &dest).await.unwrap();

    assert!(!vfs.exists(&source).await.unwrap());
    assert!(vfs.exists(&dest).await.unwrap());

    let content = vfs.read(&dest, ContentLevel::Detail).await.unwrap();
    assert_eq!(content, "测试内容");
}

#[tokio::test]
async fn test_content_loader() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);
    vfs.create_file(&uri).await.unwrap();
    vfs.write(&uri, ContentLevel::Detail, "测试内容")
        .await
        .unwrap();

    let content = vfs.read(&uri, ContentLevel::Detail).await.unwrap();
    assert_eq!(content, "测试内容");

    let has_content = vfs.has_content(&uri, ContentLevel::Detail).await.unwrap();
    assert!(has_content);
}

#[test]
fn test_validate_uri() {
    let valid_uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);
    assert!(VirtualFileSystemImpl::validate_uri(&valid_uri).is_ok());

    let root_uri = TianyanUri::new(ContextNamespace::User, vec![]);
    assert!(VirtualFileSystemImpl::validate_uri(&root_uri).is_ok());
}

#[tokio::test]
async fn test_search_session() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let session_uri = TianyanUri::new(ContextNamespace::Session, vec!["session_test".to_string()]);
    vfs.create_file(&session_uri).await.unwrap();
    vfs.write(&session_uri, ContentLevel::Detail, "会话搜索测试")
        .await
        .unwrap();

    let content = vfs.read(&session_uri, ContentLevel::Detail).await.unwrap();
    assert_eq!(content, "会话搜索测试");

    let results = vfs
        .search("搜索", 10, Some(ContextNamespace::Session))
        .await;
    assert!(results.is_err());
}

#[tokio::test]
async fn test_search_memory() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let memory_uri = TianyanUri::new(ContextNamespace::Memory, vec!["memory_test".to_string()]);
    vfs.create_file(&memory_uri).await.unwrap();
    vfs.write(&memory_uri, ContentLevel::Detail, "记忆搜索测试")
        .await
        .unwrap();

    let results = vfs.search("搜索", 10, Some(ContextNamespace::Memory)).await;
    assert!(results.is_err());
}

#[tokio::test]
async fn test_search_vfs() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let knowledge_uri = TianyanUri::new(
        ContextNamespace::Knowledge,
        vec!["knowledge_test".to_string()],
    );
    vfs.create_file(&knowledge_uri).await.unwrap();
    vfs.write(&knowledge_uri, ContentLevel::Detail, "知识搜索测试")
        .await
        .unwrap();

    let results = vfs
        .search("搜索", 10, Some(ContextNamespace::Knowledge))
        .await;
    assert!(results.is_err());
}

#[tokio::test]
async fn test_search_skill() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let skill_uri = TianyanUri::new(ContextNamespace::Skill, vec!["skill_test".to_string()]);
    vfs.create_file(&skill_uri).await.unwrap();
    vfs.write(&skill_uri, ContentLevel::Detail, "技能搜索测试")
        .await
        .unwrap();

    let results = vfs.search("搜索", 10, Some(ContextNamespace::Skill)).await;
    assert!(results.is_err());
}

#[tokio::test]
async fn test_update_summary_vectors_delegates_to_index_entry() {
    // 收敛守卫：update_summary_vectors 必须委托 index_entry 全量 upsert 路径，
    // 并保留既有 visual_vector（图像搜索用）——重复双写/覆盖回归防护。
    use std::sync::Arc;
    use tempfile::tempdir;

    use crate::config::StorageConfig;
    use crate::test_utils::{InMemoryVectorStorage, MockEmbeddingService};
    use crate::vfs::backend::LocalFileBackend;
    use crate::vfs::types::{VectorPoint, VectorType};
    use crate::vfs::vector::VectorStorage;

    let dir = tempdir().unwrap();
    let config = StorageConfig {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let storage = Arc::new(LocalFileBackend::new(config.clone()));
    let vector_storage: Arc<dyn VectorStorage> = Arc::new(InMemoryVectorStorage::new());
    let vfs = VirtualFileSystemImpl::new(storage, vector_storage.clone(), config)
        .with_embedding_provider(Arc::new(MockEmbeddingService), "test-embedding");
    vfs.initialize().await.unwrap();

    let uri = TianyanUri::new(
        ContextNamespace::Knowledge,
        vec!["delegate_test".to_string()],
    );
    vfs.create_file(&uri).await.unwrap();
    vfs.write(&uri, ContentLevel::Detail, "委托收敛测试内容")
        .await
        .unwrap();

    // 首次全量索引（含 visual_vector）
    let payload = crate::common::types::EntryMetadata::new(
        uri.clone(),
        ContextNamespace::Knowledge.to_string(),
    );
    let visual = vec![0.1f32, 0.2, 0.3];
    vfs.index_entry(
        &uri,
        "摘要",
        "概览",
        Some(visual.clone()),
        payload,
        "test-embedding",
    )
    .await
    .unwrap();

    // 经 update_summary_vectors 更新文本向量——visual_vector 必须保留
    vfs.update_summary_vectors(&uri, "新摘要", "新概览")
        .await
        .unwrap();

    let point = vector_storage
        .get_point(&uri.to_point_id())
        .await
        .unwrap()
        .expect("索引后向量点应存在");
    assert_eq!(
        point.visual_vector.as_deref(),
        Some(visual.as_slice()),
        "update_summary_vectors 不得覆盖既有 visual_vector"
    );
    // 文本向量已更新（MockEmbeddingService 返回固定 768 维向量）
    assert!(
        point.abstract_vector.is_some() && point.overview_vector.is_some(),
        "abstract/overview 向量应已写入"
    );
    let _ = VectorPoint::from_entry; // 保留类型引用（编译期确认 VectorPoint 仍可寻址）
    let _ = VectorType::Abstract;
}

#[tokio::test]
async fn test_move_entry_preserves_vector_point() {
    // 回归防护：move_entry 必须保留源向量点（abstract/overview/visual 向量与 payload 元数据），
    // 并将点 ID 强制重写为目标 to_point_id()。LanceDB 以 id 为主键 merge_insert、
    // 所有其他路径均按 to_point_id() 读写——旧实现 VectorPoint::from_entry 置空全部向量
    // 且 id 取 URI 字符串，导致目标向量点双重孤儿化（get_point/search 均不可达）。
    use std::sync::Arc;
    use tempfile::tempdir;

    use crate::config::StorageConfig;
    use crate::test_utils::{InMemoryVectorStorage, MockEmbeddingService};
    use crate::vfs::backend::LocalFileBackend;
    use crate::vfs::vector::VectorStorage;

    let dir = tempdir().unwrap();
    let config = StorageConfig {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let storage = Arc::new(LocalFileBackend::new(config.clone()));
    let vector_storage: Arc<dyn VectorStorage> = Arc::new(InMemoryVectorStorage::new());
    let vfs = VirtualFileSystemImpl::new(storage, vector_storage.clone(), config)
        .with_embedding_provider(Arc::new(MockEmbeddingService), "test-embedding");
    vfs.initialize().await.unwrap();

    let source = TianyanUri::new(ContextNamespace::User, vec!["src".to_string()]);
    let destination = TianyanUri::new(ContextNamespace::User, vec!["dst".to_string()]);
    vfs.create_file(&source).await.unwrap();
    vfs.write(&source, ContentLevel::Detail, "移动测试内容")
        .await
        .unwrap();

    // 完整索引（含 visual_vector），再经 update_summary_vectors 刷新文本向量
    let payload = crate::common::types::EntryMetadata::new(
        source.clone(),
        ContextNamespace::User.to_string(),
    );
    let visual = vec![0.5f32, 0.6, 0.7];
    vfs.index_entry(
        &source,
        "摘要",
        "概览",
        Some(visual.clone()),
        payload,
        "test-embedding",
    )
    .await
    .unwrap();
    vfs.update_summary_vectors(&source, "新摘要", "新概览")
        .await
        .unwrap();

    vfs.move_entry(&source, &destination).await.unwrap();

    // 源向量点已删除
    assert!(
        vector_storage
            .get_point(&source.to_point_id())
            .await
            .unwrap()
            .is_none(),
        "move 后源向量点应已删除"
    );
    // 目标向量点存在，ID 为规范 point_id，三向量与 payload.uri 全部保留
    let dst_point = vector_storage
        .get_point(&destination.to_point_id())
        .await
        .unwrap()
        .expect("move 后目标向量点应存在（以目标 to_point_id() 为主键）");
    assert_eq!(
        dst_point.id,
        destination.to_point_id(),
        "目标点 ID 应为目标 URI 的 to_point_id()"
    );
    assert_eq!(
        dst_point.payload.uri, destination,
        "目标点 payload.uri 应指向目标 URI"
    );
    assert!(
        dst_point.abstract_vector.is_some() && dst_point.overview_vector.is_some(),
        "abstract/overview 向量应保留"
    );
    assert_eq!(
        dst_point.visual_vector.as_deref(),
        Some(visual.as_slice()),
        "visual_vector 应保留"
    );

    // 检索可命中目标、且不再命中源（MockEmbeddingService 返回固定向量，余弦相似度 = 1.0）
    let results = vfs.search("查询", 10, None).await.unwrap();
    assert!(
        results.iter().any(|r| r.uri == destination),
        "move 后检索应命中目标条目，实际命中：{:?}",
        results.iter().map(|r| r.uri.as_str()).collect::<Vec<_>>()
    );
    assert!(
        !results.iter().any(|r| r.uri == source),
        "move 后检索不应命中源条目"
    );
}

#[tokio::test]
async fn test_move_entry_directory_with_children_conflict() {
    // 子条目守卫：move 仅迁移顶层条目（delete_entry 在两种后端下均为递归删除，
    // 但 move 只拷贝顶层条目），源目录含子条目时必须返回冲突错误，
    // 防止子条目被静默孤儿化。
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let source = TianyanUri::new(ContextNamespace::User, vec!["dir".to_string()]);
    vfs.create_directory(&source).await.unwrap();
    let child = source.append("child");
    vfs.create_file(&child).await.unwrap();
    let destination = TianyanUri::new(ContextNamespace::User, vec!["moved".to_string()]);

    let err = vfs.move_entry(&source, &destination).await.unwrap_err();
    assert!(
        err.is_conflict(),
        "含子条目的源目录移动应返回冲突错误，实际：{err}"
    );
    // 移动未发生：源目录与子条目均保留，目标不存在
    assert!(vfs.exists(&source).await.unwrap());
    assert!(vfs.exists(&child).await.unwrap());
    assert!(!vfs.exists(&destination).await.unwrap());
}

// ── T1-18：向量-内容对账 ───────────────────────────────────────────────

/// 对账：内容有、向量缺 → 重建索引；向量有、内容无 → 删除孤儿点。
#[tokio::test]
async fn test_reconcile_vector_index_rebuilds_missing_and_removes_orphans() {
    use std::sync::Arc;

    use crate::common::types::EntryMetadata;
    use crate::config::StorageConfig;
    use crate::db::Database;
    use crate::test_utils::{InMemoryVectorStorage, MockEmbeddingService};
    use crate::vfs::backend::SqliteBackend;
    use crate::vfs::traits::VectorReconcileStats;
    use crate::vfs::types::VectorPoint;
    use crate::vfs::vector::VectorStorage;
    use crate::vfs::{VirtualFileSystem, CURRENT_SCHEMA_VERSION};

    let dir = tempfile::tempdir().unwrap();
    let config = StorageConfig {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    // 生产同构后端（SqliteBackend：条目 is_directory 由表字段决定）。
    // LocalFileBackend 把条目映射为目录（内含 .meta/content.* 内部文件），
    // 枚举语义与生产不一致（会把条目当成目录递归进去）。
    let db = Database::open_in_memory().unwrap();
    db.init_schemas().await.unwrap();
    let backend = Arc::new(SqliteBackend::new(db));
    backend.ensure_schema().await.unwrap();
    let vector = Arc::new(InMemoryVectorStorage::new());
    let vfs = VirtualFileSystemImpl::new(backend, vector.clone(), config)
        .with_embedding_provider(Arc::new(MockEmbeddingService), "test-model");

    let uri = TianyanUri::new(
        ContextNamespace::Knowledge,
        vec!["reconcile.md".to_string()],
    );
    vfs.create_file(&uri).await.unwrap();
    vfs.write(&uri, ContentLevel::Abstract, "摘要文本")
        .await
        .unwrap();
    vfs.write(&uri, ContentLevel::Overview, "概览文本")
        .await
        .unwrap();
    vfs.update_summary_vectors(&uri, "摘要文本", "概览文本")
        .await
        .unwrap();
    let point_id = uri.to_point_id();
    assert!(
        vector.get_point(&point_id).await.unwrap().is_some(),
        "前置：索引已建立"
    );

    // 漂移 A：向量点丢失（删除失败 / 表被重建）
    vector.delete_point(&point_id).await.unwrap();
    // 漂移 B：孤儿点（内容已不存在，向量残留）
    let ghost_uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["ghost.md".to_string()]);
    let orphan = VectorPoint {
        schema_version: CURRENT_SCHEMA_VERSION,
        id: ghost_uri.to_point_id(),
        abstract_vector: Some(vec![0.0; 8]),
        overview_vector: Some(vec![0.0; 8]),
        visual_vector: None,
        payload: EntryMetadata::new(ghost_uri.clone(), "knowledge"),
    };
    vector.upsert_point(&orphan).await.unwrap();

    let stats = vfs.reconcile_vector_index().await.unwrap();
    assert_eq!(
        (stats.reindexed, stats.orphans_removed, stats.errors),
        (1, 1, 0),
        "对账结果不符（实际 stats = {stats:?}）"
    );
    assert!(
        vector.get_point(&point_id).await.unwrap().is_some(),
        "重建后索引应存在"
    );
    assert!(
        vector.get_point(&orphan.id).await.unwrap().is_none(),
        "孤儿点应被清理"
    );

    // 幂等：再对账无待修复项
    let again = vfs.reconcile_vector_index().await.unwrap();
    assert_eq!(again, VectorReconcileStats::default());
}

/// P0-7 回归：系统/运维路径不参与向量对账重建（归档/评审不得建向量）。
#[tokio::test]
async fn test_reconcile_skips_system_paths() {
    use std::sync::Arc;

    use crate::config::StorageConfig;
    use crate::db::Database;
    use crate::test_utils::{InMemoryVectorStorage, MockEmbeddingService};
    use crate::vfs::backend::SqliteBackend;
    use crate::vfs::vector::VectorStorage;
    use crate::vfs::{VfsCore, VirtualFileSystem};

    let dir = tempfile::tempdir().unwrap();
    let config = StorageConfig {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let db = Database::open_in_memory().unwrap();
    db.init_schemas().await.unwrap();
    let backend = Arc::new(SqliteBackend::new(db));
    backend.ensure_schema().await.unwrap();
    let vector = Arc::new(InMemoryVectorStorage::new());
    let vfs = VirtualFileSystemImpl::new(backend, vector.clone(), config)
        .with_embedding_provider(Arc::new(MockEmbeddingService), "test-model");

    let learned = TianyanUri::new(ContextNamespace::Agent, vec!["learned".to_string()]);
    let archive = learned.append("archive");
    let reviews = TianyanUri::new(ContextNamespace::Skill, vec!["_reviews".to_string()]);
    vfs.create_directory(&learned).await.unwrap();
    vfs.create_directory(&archive).await.unwrap();
    vfs.create_directory(&reviews).await.unwrap();

    // 活跃规则（应重建）+ 归档规则 / 评审记录（系统路径，不得建向量）
    let live = learned.append("rule-1");
    let archived = archive.append("old-rule");
    let review = reviews.append("s1.jsonl");
    for uri in [&live, &archived, &review] {
        vfs.create_file(uri).await.unwrap();
        vfs.write(uri, ContentLevel::Abstract, "摘要文本")
            .await
            .unwrap();
        vfs.write(uri, ContentLevel::Overview, "概览文本")
            .await
            .unwrap();
    }

    let stats = vfs.reconcile_vector_index().await.unwrap();
    assert_eq!(stats.errors, 0, "对账不应报错（stats = {stats:?}）");
    assert!(
        vector
            .get_point(&live.to_point_id())
            .await
            .unwrap()
            .is_some(),
        "活跃条目应重建索引"
    );
    assert!(
        vector
            .get_point(&archived.to_point_id())
            .await
            .unwrap()
            .is_none(),
        "归档路径不得建向量（reconcile 应跳过）"
    );
    assert!(
        vector
            .get_point(&review.to_point_id())
            .await
            .unwrap()
            .is_none(),
        "评审记录不得建向量（reconcile 应跳过）"
    );
}

use super::*;
use crate::common::types::{ContextNamespace, EntryMetadata, TianyanUri};
use crate::config::StorageConfig;
use crate::vfs::CURRENT_SCHEMA_VERSION;
use tempfile::tempdir;
/// 创建一个测试用的向量（维度 8）。
fn test_vec() -> Vec<f32> {
    vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
}

/// 创建一个不同的测试向量。
fn test_vec2() -> Vec<f32> {
    vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
}

/// 辅助函数：创建测试 VectorPoint。
fn make_point(uri: &str, abs_vec: Option<Vec<f32>>, ov_vec: Option<Vec<f32>>) -> VectorPoint {
    let parsed_uri = TianyanUri::parse(uri)
        .unwrap_or_else(|_| TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]));
    VectorPoint {
        schema_version: CURRENT_SCHEMA_VERSION,
        id: uri.replace("tianyan://", "").replace('/', "_"),
        abstract_vector: abs_vec,
        overview_vector: ov_vec,
        visual_vector: None,
        payload: EntryMetadata::new(parsed_uri, "test"),
    }
}

/// 辅助函数：创建隔离的 LanceDbVectorStore + TempDir。
async fn create_store() -> (LanceDbVectorStore, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let mut config = StorageConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };
    config.vector.vector_dimension = 8;
    config.vector.collection_name = "test_lancedb".to_string();
    let store = LanceDbVectorStore::new(&config).await.unwrap();
    (store, dir)
}

// ─── 基础创建与初始化 ───

#[tokio::test]
async fn test_new_store() {
    let (_store, _dir) = create_store().await;
}

#[tokio::test]
async fn test_initialize() {
    let (store, _dir) = create_store().await;
    // initialize() is a no-op for LanceDB but should succeed
    store.initialize().await.unwrap();
}

// ─── 插入与获取 ───

#[tokio::test]
async fn test_upsert_and_get_point() {
    let (store, _dir) = create_store().await;
    let point = make_point("tianyan://knowledge/test_doc", Some(test_vec()), None);
    store.upsert_point(&point).await.unwrap();

    let id = point.uri().to_point_id();
    let retrieved = store.get_point(&id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.abstract_vector, point.abstract_vector);
    assert_eq!(retrieved.overview_vector, point.overview_vector);
    assert_eq!(retrieved.visual_vector, point.visual_vector);
    // id 字段在存储时由 to_point_id() 重新生成
    assert_eq!(retrieved.id, id);
    assert_eq!(retrieved.schema_version, CURRENT_SCHEMA_VERSION);
}

#[tokio::test]
async fn test_upsert_updates_existing() {
    let (store, _dir) = create_store().await;
    let uri = "tianyan://memory/event_1";
    let p1 = make_point(uri, Some(test_vec()), None);
    store.upsert_point(&p1).await.unwrap();

    // 用不同的向量 upsert 同一个点
    let p2 = make_point(uri, Some(test_vec2()), None);
    store.upsert_point(&p2).await.unwrap();

    let id = p2.uri().to_point_id();
    let retrieved = store.get_point(&id).await.unwrap().unwrap();
    // 应该拿到更新的向量
    assert_eq!(retrieved.abstract_vector, Some(test_vec2()));
    assert_eq!(retrieved.overview_vector, None);
}

// ─── 获取不存在 ───

#[tokio::test]
async fn test_get_point_nonexistent() {
    let (store, _dir) = create_store().await;
    let result = store.get_point("nonexistent-id-12345").await.unwrap();
    assert!(result.is_none());
}

// ─── 搜索 ───

#[tokio::test]
async fn test_search_empty() {
    let (store, _dir) = create_store().await;
    // 空表搜索：nearest_to 可能返回错误或空结果
    let query = VectorSearchQuery {
        vector: test_vec(),
        vector_type: VectorType::Abstract,
        limit: 10,
        category_filter: None,
        min_score: None,
    };
    match store.search(query).await {
        Ok(results) => assert!(results.is_empty()),
        Err(_) => {
            // LanceDB empty-table search 可能失败，接受两种行为
        }
    }
}

#[tokio::test]
async fn test_search_with_data() {
    let (store, _dir) = create_store().await;
    let p1 = make_point("tianyan://knowledge/doc_a", Some(test_vec()), None);
    let p2 = make_point("tianyan://knowledge/doc_b", Some(test_vec2()), None);
    store.upsert_point(&p1).await.unwrap();
    store.upsert_point(&p2).await.unwrap();

    let query = VectorSearchQuery {
        vector: test_vec(),
        vector_type: VectorType::Abstract,
        limit: 10,
        category_filter: None,
        min_score: None,
    };
    let results = store.search(query).await.unwrap();
    assert!(!results.is_empty());
    // 第一个结果应该与 test_vec 相似（doc_a）
    assert_eq!(results[0].id, p1.uri().to_point_id());
    // 分数应接近 1.0（完全相同）
    assert!((results[0].score - 1.0).abs() < 0.01);
}

#[tokio::test]
async fn test_search_with_category_filter() {
    let (store, _dir) = create_store().await;
    // 插入 knowledge 和 memory 两个命名空间
    let p_know = make_point("tianyan://knowledge/ref1", Some(test_vec()), None);
    let p_mem = make_point("tianyan://memory/ref1", Some(test_vec()), None);
    store.upsert_point(&p_know).await.unwrap();
    store.upsert_point(&p_mem).await.unwrap();

    let query = VectorSearchQuery {
        vector: test_vec(),
        vector_type: VectorType::Abstract,
        limit: 10,
        category_filter: Some("knowledge".to_string()),
        min_score: None,
    };
    let results = store.search(query).await.unwrap();
    assert!(!results.is_empty());
    for r in &results {
        assert_eq!(r.payload.uri.namespace().to_string(), "knowledge");
    }
}

#[tokio::test]
async fn test_search_with_min_score() {
    let (store, _dir) = create_store().await;
    let point = make_point("tianyan://knowledge/only_one", Some(test_vec()), None);
    store.upsert_point(&point).await.unwrap();

    // 使用远超可能范围的 min_score
    let query = VectorSearchQuery {
        vector: test_vec(),
        vector_type: VectorType::Abstract,
        limit: 10,
        category_filter: None,
        min_score: Some(10.0),
    };
    let results = store.search(query).await.unwrap();
    assert!(results.is_empty());
}

// ─── 删除 ───

#[tokio::test]
async fn test_delete_point() {
    let (store, _dir) = create_store().await;
    let point = make_point("tianyan://knowledge/to_delete", Some(test_vec()), None);
    store.upsert_point(&point).await.unwrap();
    let id = point.uri().to_point_id();

    // 确认存在
    assert!(store.get_point(&id).await.unwrap().is_some());

    // 删除
    store.delete_point(&id).await.unwrap();
    assert!(store.get_point(&id).await.unwrap().is_none());
}

// ─── search_abstract_and_overview（默认 trait 方法） ───

#[tokio::test]
async fn test_search_abstract_and_overview() {
    let (store, _dir) = create_store().await;
    let p1 = make_point("tianyan://knowledge/a", Some(test_vec()), None);
    store.upsert_point(&p1).await.unwrap();

    // 通过 trait 调用默认方法
    use crate::vfs::vector::VectorStorage;
    let results = store
        .search_abstract_and_overview(test_vec(), 10, None)
        .await
        .unwrap();

    // RRF 融合仅决定排序；score 保持相似度尺度（1.0 - distance），
    // 与单列 search() 一致，供 ContentLoadStrategy 绝对阈值使用。
    assert!(!results.is_empty());
    assert_eq!(results[0].id, p1.uri().to_point_id());
    assert!(
        results[0].score > 0.5,
        "融合搜索分数应为相似度尺度（≈1.0），实际: {}",
        results[0].score
    );
}

// ─── 维度不匹配防御（闪退根因回归） ───
//
// 历史故障：配置 vector_dimension(3072) ≠ 嵌入 API 实际输出(1024) 时，
// arrow FixedSizeListBuilder 产出非法数组 → lance 内部 panic →
// release panic=abort 整进程闪退（BEX64，日志无输出）。
// 回归锁定：维度不匹配必须在存储层转为干净错误，绝不 panic。

#[tokio::test]
async fn test_upsert_dimension_mismatch_returns_clean_error() {
    let (store, _dir) = create_store().await;
    // 4 维向量 vs 建表 8 维
    let point = make_point(
        "tianyan://knowledge/dim_mismatch",
        Some(vec![1.0, 0.0, 0.0, 0.0]),
        Some(vec![1.0, 0.0, 0.0, 0.0]),
    );
    let err = store.upsert_point(&point).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("维度不匹配"), "错误信息应可行动：{msg}");
}

#[tokio::test]
async fn test_search_dimension_mismatch_returns_clean_error() {
    let (store, _dir) = create_store().await;
    let query = VectorSearchQuery {
        vector: vec![1.0, 0.0, 0.0, 0.0], // 4 维 vs 建表 8 维
        vector_type: VectorType::Abstract,
        limit: 10,
        category_filter: None,
        min_score: None,
    };
    let err = store.search(query).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("维度不匹配"), "错误信息应可行动：{msg}");
}

#[tokio::test]
async fn test_embedding_dim_exposes_table_dimension() {
    let (store, _dir) = create_store().await;
    use crate::vfs::vector::VectorStorage as _;
    assert_eq!(store.embedding_dim(), 8);
}

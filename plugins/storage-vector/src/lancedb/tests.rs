use super::*;
use tempfile::tempdir;
use tianyan::common::types::{ContextNamespace, EntryMetadata, TianyanUri};
use tianyan::config::StorageConfig;
use tianyan::vfs::CURRENT_SCHEMA_VERSION;
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

// ─── T1-14：显式 limit（LanceDB 默认 top-k=10 静默截断）───

#[tokio::test]
async fn test_get_point_beyond_default_topk_limit() {
    // T1-14：LanceDB Query 默认 top-k=10——未显式 limit 时第 11 行之后的 ID
    // 永远查不到（move/回填时向量点静默丢失）。
    let (store, _dir) = create_store().await;
    let mut last_id = String::new();
    for i in 0..15 {
        let point = make_point(
            &format!("tianyan://knowledge/doc_{i:02}"),
            Some(test_vec()),
            None,
        );
        last_id = point.uri().to_point_id();
        store.upsert_point(&point).await.unwrap();
    }
    let got = store.get_point(&last_id).await.unwrap();
    assert!(got.is_some(), "第 15 个点应能查到（默认 top-k 截断修复）");
}

#[tokio::test]
async fn test_search_returns_requested_limit_beyond_default_topk() {
    // T1-14：search 同样受默认 top-k=10 限制——请求 15 条此前只得 10 条。
    let (store, _dir) = create_store().await;
    for i in 0..15 {
        let point = make_point(
            &format!("tianyan://knowledge/sdoc_{i:02}"),
            Some(test_vec()),
            None,
        );
        store.upsert_point(&point).await.unwrap();
    }
    let results = store
        .search(VectorSearchQuery {
            vector: test_vec(),
            vector_type: VectorType::Abstract,
            limit: 15,
            category_filter: None,
            min_score: None,
        })
        .await
        .unwrap();
    assert_eq!(
        results.len(),
        15,
        "请求 15 条应返回 15 条（此前被默认 top-k=10 截断）"
    );
}

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

/// 回归（知识检索恒空）：目标命名空间的条目**排在全局 top_k 之外**时仍须命中。
///
/// 旧实现把 `category_filter` 放在「取回 top_k 之后」用内存 `retain` 过滤 —— 全局
/// top_k 被别的命名空间占满时，过滤后一条不剩。真实数据上表现为
/// `vfs.search(q, 20, Some(Knowledge))` **恒为 0**（知识库 20 条全都有 L0/L1 向量），
/// 而同一 query 不加过滤能命中 7 条 knowledge。
///
/// 修复 = 过滤下推 LanceDB（`only_if`；`QueryRequest.prefilter` 默认 true →
/// 先过滤再 ANN）。上面的 `test_search_with_category_filter` 两个点都在全局 top_k 内，
/// 所以从未暴露这个退化 —— 本测试必须让目标条目在全局 top_k **之外**才有意义。
#[tokio::test]
async fn test_search_category_filter_not_swallowed_by_global_topk() {
    let (store, _dir) = create_store().await;
    // 12 条 memory：向量与 query 完全相同（相似度 1.0）→ 必然占满全局 top-5
    for i in 0..12 {
        let p = make_point(
            &format!("tianyan://memory/noise_{i}"),
            Some(test_vec()),
            None,
        );
        store.upsert_point(&p).await.unwrap();
    }
    // 1 条 knowledge：向量不同、相似度更低 → 排不进全局 top-5
    let target = make_point("tianyan://knowledge/target", Some(test_vec2()), None);
    store.upsert_point(&target).await.unwrap();

    let query = VectorSearchQuery {
        vector: test_vec(),
        vector_type: VectorType::Abstract,
        limit: 5,
        category_filter: Some("knowledge".to_string()),
        min_score: None,
    };
    let results = store.search(query).await.unwrap();
    assert_eq!(
        results.len(),
        1,
        "过滤下推后应命中 knowledge 条目（旧实现返回空）"
    );
    assert_eq!(results[0].id, target.uri().to_point_id());
}

/// 同上，但走 RRF 融合路径（`search_abstract_and_overview` = 知识检索实际入口）。
#[tokio::test]
async fn test_search_fused_category_filter_not_swallowed_by_global_topk() {
    let (store, _dir) = create_store().await;
    for i in 0..12 {
        let p = make_point(
            &format!("tianyan://memory/noise_{i}"),
            Some(test_vec()),
            Some(test_vec()),
        );
        store.upsert_point(&p).await.unwrap();
    }
    let target = make_point(
        "tianyan://knowledge/target",
        Some(test_vec2()),
        Some(test_vec2()),
    );
    store.upsert_point(&target).await.unwrap();

    let results = store
        .search_abstract_and_overview(test_vec(), 5, Some("knowledge"))
        .await
        .unwrap();
    assert_eq!(results.len(), 1, "RRF 路径同样不得被全局 top_k 吞掉");
    assert_eq!(results[0].payload.uri.namespace().to_string(), "knowledge");
}

/// 距离口径回归：分数必须是**余弦相似度**，正交向量应为 0 而不是 -1。
///
/// 两个检索入口都显式指定 `DistanceType::Cosine`（`_distance = 1 - cos`），
/// 所以 `score = 1 - distance = cos`。若退回 LanceDB 默认的 L2（平方欧氏）：
/// 未归一化向量下 `1 - distance` 毫无意义，归一化向量下也等于 `2cos-1`
/// —— 知识检索整排负分（用户截图里的 -0.047 / -0.133）就是它。
#[tokio::test]
async fn test_search_score_is_cosine_similarity() {
    let (store, _dir) = create_store().await;
    // 正交（cos = 0）
    let orth = make_point(
        "tianyan://knowledge/orth",
        Some(vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        None,
    );
    store.upsert_point(&orth).await.unwrap();
    // 同向（cos = 1）
    let same = make_point("tianyan://knowledge/same", Some(test_vec()), None);
    store.upsert_point(&same).await.unwrap();

    let query = VectorSearchQuery {
        vector: test_vec(),
        vector_type: VectorType::Abstract,
        limit: 10,
        category_filter: None,
        min_score: None,
    };
    let results = store.search(query).await.unwrap();
    let score_of = |id: &str| {
        results
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.score)
            .unwrap_or_else(|| panic!("结果里缺少 {id}"))
    };
    let same_score = score_of(&same.uri().to_point_id());
    let orth_score = score_of(&orth.uri().to_point_id());
    assert!(
        (same_score - 1.0).abs() < 0.01,
        "同向余弦相似度应≈1，实际 {same_score}"
    );
    assert!(
        orth_score.abs() < 0.01,
        "正交余弦相似度应≈0，实际 {orth_score}（L2 口径会得 -1）"
    );
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
    use tianyan::vfs::VectorStorage as _;
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
// 当时 release 为 panic=abort：整进程闪退（BEX64，日志无输出）。
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
    use tianyan::vfs::VectorStorage as _;
    assert_eq!(store.embedding_dim(), 8);
}

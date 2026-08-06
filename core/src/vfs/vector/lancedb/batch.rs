//! RecordBatch ↔ VectorPoint 转换辅助。
//!
//! Arrow 列构建/提取的纯转换函数，与存储生命周期无关。

use std::collections::HashMap;

use arrow_array::builder::{FixedSizeListBuilder, Float32Builder};
use arrow_array::cast::AsArray;
use arrow_array::types::Float32Type;
use arrow_array::{Array, RecordBatch};

use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContextNamespace, EntryMetadata, TianyanUri};
use crate::vfs::types::{VectorPoint, VectorSearchResult, VectorType, CURRENT_SCHEMA_VERSION};

pub(super) fn append_vector(
    builder: &mut FixedSizeListBuilder<Float32Builder>,
    vec: &Option<Vec<f32>>,
) {
    match vec {
        Some(v) => {
            for &x in v {
                builder.values().append_value(x);
            }
            builder.append(true);
        }
        None => {
            // Arrow FixedSizeListBuilder::finish() 要求 values.len() == value_length * len()。
            // 即使 null 条目也需要填充 value_length 个占位 null 值。
            let dim = builder.value_length() as usize;
            for _ in 0..dim {
                builder.values().append_null();
            }
            builder.append(false);
        }
    }
}

pub(super) fn extract_vector(col: &dyn Array, row: usize) -> Option<Vec<f32>> {
    if col.is_null(row) {
        return None;
    }
    Some(
        col.as_fixed_size_list()
            .value(row)
            .as_primitive::<Float32Type>()
            .values()
            .to_vec(),
    )
}

/// 从 RecordBatch 的行中提取基础字段并构造 EntryMetadata。
/// 返回 `(id, payload)`，当 id 或 uri 列为空时返回 `None`。
pub(super) fn extract_base_fields(
    batch: &RecordBatch,
    row: usize,
) -> Option<(String, EntryMetadata)> {
    let id = batch
        .column_by_name("id")?
        .as_string::<i32>()
        .value(row)
        .to_string();
    let uri_s = batch
        .column_by_name("uri")?
        .as_string::<i32>()
        .value(row)
        .to_string();
    let ns = batch
        .column_by_name("namespace")
        .map(|c| c.as_string::<i32>().value(row).to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let importance = batch
        .column_by_name("importance")
        .map(|c| c.as_primitive::<Float32Type>().value(row))
        .unwrap_or(0.5);
    let custom: HashMap<String, serde_json::Value> = batch
        .column_by_name("custom_json")
        .and_then(|c| serde_json::from_str(c.as_string::<i32>().value(row)).ok())
        .unwrap_or_default();

    let parsed_uri = TianyanUri::parse(&uri_s)
        .unwrap_or_else(|_| TianyanUri::new(ContextNamespace::User, vec![id.clone()]));
    let mut payload = EntryMetadata::new(parsed_uri, &ns);
    payload.importance = importance;
    payload.custom = custom;

    Some((id, payload))
}

pub(super) fn batch_to_point(batch: &RecordBatch, row: usize) -> Option<VectorPoint> {
    let (id, payload) = extract_base_fields(batch, row)?;
    Some(VectorPoint {
        schema_version: CURRENT_SCHEMA_VERSION,
        id,
        abstract_vector: extract_vector(batch.column_by_name("abstract_vec")?, row),
        overview_vector: extract_vector(batch.column_by_name("overview_vec")?, row),
        visual_vector: extract_vector(batch.column_by_name("visual_vec")?, row),
        payload,
    })
}

/// 将 VectorType 映射到 LanceDB 中的列名。
pub(super) fn vector_column_name(vec_type: VectorType) -> &'static str {
    match vec_type {
        VectorType::Abstract => "abstract_vec",
        VectorType::Overview => "overview_vec",
        VectorType::Visual => "visual_vec",
    }
}

pub(super) fn batch_to_results(batch: &RecordBatch) -> Result<Vec<VectorSearchResult>> {
    let score_data = batch
        .column_by_name("_distance")
        .map(|c| c.as_primitive::<Float32Type>());

    let mut results = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        let (id, payload) = extract_base_fields(batch, i)
            .ok_or_else(|| TianyanError::Custom("向量数据库错误：缺少 id 或 uri 列".to_string()))?;
        let score = score_data.as_ref().map(|c| 1.0 - c.value(i)).unwrap_or(0.0);
        results.push(VectorSearchResult { id, score, payload });
    }
    Ok(results)
}

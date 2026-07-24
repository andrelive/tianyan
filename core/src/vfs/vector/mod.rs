//! 向量存储 —— VectorStorage 适配器实现。

mod lancedb;
mod traits;

pub use lancedb::LanceDbVectorStore;
pub use traits::VectorStorage;

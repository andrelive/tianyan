//! 存储后端 —— StorageBackend trait 及 LocalFileBackend 适配器。

mod local;
mod traits;

pub use local::LocalFileBackend;
pub use traits::StorageBackend;

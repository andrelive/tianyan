//! 结构化 Trace API 模块（G6）：span 回放查询。

pub mod handlers;
mod routes;

pub use routes::routes;

#[cfg(test)]
mod tests;

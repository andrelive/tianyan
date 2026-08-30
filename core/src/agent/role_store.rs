//! 角色 VFS 存储（ADR-016：注册表持久化）——**已移至 [`crate::role_store`]**。
//!
//! 本模块仅作 re-export 兼容（agent 内部历史引用）；新代码直接引用
//! [`crate::role_store`]。移动原因：scheduler（演化任务）与 agent 都需角色存储，
//! 放 agent 下迫使 scheduler → agent 循环——独立存储层解决。

pub use crate::role_store::*;

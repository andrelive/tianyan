//! 可注入上下文类型。
//!
//! 定义 ContextPipeline 填充、ContextAssembler 组装使用的可注入上下文结构。
//! 该类型原本位于 `agent::session_state`，提取到此处以消除 `context` → `agent` 的倒置依赖。
//!
//! `Serialize`/`Deserialize` 用于会话快照持久化（SessionHeader）：
//! 前缀内容（soul/rules/memories）随会话固化，重启后沿用同一份，
//! 不重新检索——保证旧会话前缀稳定，prompt 缓存不失效。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 可注入的上下文内容，由 ContextPipeline 填充，由 ContextAssembler 组装使用。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InjectableContext {
    /// 智能体核心人格（soul.md）。
    pub soul: String,
    /// 项目指令（会话绑定工作目录下的 AGENTS.md；无则空）。
    ///
    /// 会话首次加载上下文时读取一次（会话绑定工作目录后不变），
    /// 随快照持久化（旧快照缺省空，向后兼容）。
    #[serde(default)]
    pub project_instructions: String,
    /// 经验与方法论。
    pub rules_and_experiences: Vec<String>,
    /// 用户画像与环境事实。
    pub memories: Vec<String>,
    /// 最后更新时间。
    pub last_updated: DateTime<Utc>,
}

impl InjectableContext {
    /// 创建空的注入上下文。
    pub fn new() -> Self {
        Self {
            last_updated: Utc::now(),
            ..Default::default()
        }
    }
}

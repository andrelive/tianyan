//! 可注入上下文类型。
//!
//! 定义 ContextPipeline 填充、ContextAssembler 组装使用的可注入上下文结构。
//! 该类型原本位于 `agent::session_state`，提取到此处以消除 `context` → `agent` 的倒置依赖。

use chrono::{DateTime, Utc};

/// 可注入的上下文内容，由 ContextPipeline 填充，由 ContextAssembler 组装使用。
#[derive(Debug, Clone, Default)]
pub struct InjectableContext {
    /// 智能体核心人格（soul.md）。
    pub soul: String,
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

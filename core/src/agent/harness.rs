//! Agent 的 Harness 工程子系统。
//!
//! 封装 AgentMetrics，提供可观测性能力。

use std::sync::Arc;

use crate::observability::AgentMetrics;

/// Agent 的 Harness 工程子系统。
#[derive(Clone)]
pub struct AgentHarness {
    /// 可观测性指标。
    pub metrics: Arc<AgentMetrics>,
}

impl AgentHarness {
    /// 创建新的 Harness 子系统。
    pub fn new(
        metrics: Arc<AgentMetrics>,
    ) -> Self {
        Self {
            metrics,
        }
    }
}

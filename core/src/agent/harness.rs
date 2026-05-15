//! Agent 的 Harness 工程子系统。
//!
//! 封装 RuleRecorder、RuleSuggester 和 AgentMetrics，
//! 三者紧密协作，共同实现失败驱动增强的完整闭环。

use std::sync::Arc;

use crate::context::{RuleRecorder, RuleSuggester};
use crate::observability::AgentMetrics;

/// Agent 的 Harness 工程子系统。
#[derive(Clone)]
pub struct AgentHarness {
    /// 规则记录器（失败时追加规则）。
    pub rule_recorder: RuleRecorder,
    /// 规则建议器（从记忆聚类提炼规则）。
    pub rule_suggester: Option<RuleSuggester>,
    /// 可观测性指标。
    pub metrics: Arc<AgentMetrics>,
}

impl AgentHarness {
    /// 创建新的 Harness 子系统。
    pub fn new(
        rule_recorder: RuleRecorder,
        rule_suggester: Option<RuleSuggester>,
        metrics: Arc<AgentMetrics>,
    ) -> Self {
        Self {
            rule_recorder,
            rule_suggester,
            metrics,
        }
    }
}

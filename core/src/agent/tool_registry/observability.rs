//! 内置工具可观测性后置监听器（A1 迁移：原 execute_single 尾部观测逻辑 → 管线监听器）。
//!
//! 把"统计改为监听器"（DSH 吸收清单 A1）：usage stats / Trace / GEPA 执行历史 /
//! 失败规则学习从 execute_single 尾部硬编码块迁移为第一个 post-execute 监听器，
//! 行为完全一致（注册于 ToolRegistry::new，时序与原尾部相同），但自此可被替换、
//! 组合或旁路——观测不再是管线的一部分，而是管线上的一个消费者。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::pipeline::ToolPostExecuteListener;
use crate::agent::tool_params::CallSkillParams;
use crate::common::error::TianyanError;
use crate::model::types::ToolCall;
use crate::observability::trace::TraceCollector;
use crate::observability::usage_stats::UsageStats;
use crate::scheduler::tasks::RuleRecorder;
use crate::skills::learning::ExecutionHistory;

/// 可观测性状态（std Mutex 短临界区：仅克隆 Arc / 同步 push，跨 await 不持锁）。
#[derive(Default)]
struct ObservabilityState {
    usage_stats: Option<Arc<UsageStats>>,
    trace_collector: Option<Arc<TraceCollector>>,
    execution_history: Vec<ExecutionHistory>,
    rule_recorder: Option<Arc<RuleRecorder>>,
}

/// 内置可观测性监听器：统计 / Trace / GEPA 历史 / 失败规则学习。
#[derive(Clone, Default)]
pub(crate) struct ToolObservabilityListener {
    inner: Arc<Mutex<ObservabilityState>>,
}

impl ToolObservabilityListener {
    /// 设置使用统计追踪器（由 ToolRegistry::with_usage_stats 委托）。
    pub fn set_usage_stats(&self, stats: Arc<UsageStats>) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .usage_stats = Some(stats);
    }

    /// 设置 Trace 收集器（由 ToolRegistry::with_trace_collector 委托）。
    pub fn set_trace_collector(&self, collector: Arc<TraceCollector>) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .trace_collector = Some(collector);
    }

    /// 设置规则记录器（由 ToolRegistry::with_rule_recorder 委托）。
    pub fn set_rule_recorder(&self, recorder: Arc<RuleRecorder>) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .rule_recorder = Some(recorder);
    }

    /// 排空已收集的执行轨迹（供 GEPA 引擎消费）。
    pub fn drain_execution_history(&self) -> Vec<ExecutionHistory> {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut state.execution_history)
    }
}

#[async_trait]
impl ToolPostExecuteListener for ToolObservabilityListener {
    async fn on_post_execute(
        &self,
        call: &ToolCall,
        session_id: &str,
        result: &mut Result<serde_json::Value, TianyanError>,
        elapsed: std::time::Duration,
    ) {
        let (usage_stats, trace_collector, rule_recorder) = {
            let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.usage_stats.clone(),
                state.trace_collector.clone(),
                state.rule_recorder.clone(),
            )
        };
        let arguments = &call.function.arguments;
        let success = result.is_ok();

        // Record usage stats for tool call
        if let Some(ref stats) = usage_stats {
            stats.record_skill_call(&call.function.name, success, elapsed.as_micros() as u64);
        }

        // G6 结构化 Trace：工具 span（归属当前轮；参数摘要截断控制体积）
        if let Some(ref trace) = trace_collector {
            trace.record_tool(
                session_id,
                &call.function.name,
                &super::truncate_trace_params(arguments),
                elapsed.as_millis() as i64,
                success,
                result.as_ref().err().map(|e| e.to_string()),
            );
        }

        // Record execution history for GEPA
        {
            let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            state.execution_history.push(ExecutionHistory {
                task_description: format!("{}: {}", call.function.name, arguments),
                steps: vec![],
                result: match result {
                    Ok(v) => format!("{}", v),
                    Err(e) => e.to_string(),
                },
                success,
                execution_time_ms: elapsed.as_millis() as u64,
                skills_used: if call.function.name == "call_skill" {
                    serde_json::from_str::<CallSkillParams>(arguments)
                        .map(|p| vec![p.skill_id])
                        .unwrap_or_default()
                } else {
                    vec![]
                },
            });
        }

        // Record tool failures as learned rules for GEPA evolution.
        // Only Logic/System failures are persisted; Transient errors (e.g.,
        // network timeouts) are skipped by RuleRecorder::record_with_kind.
        if !success {
            if let Some(ref recorder) = rule_recorder {
                let tool_name = &call.function.name;
                let err_msg = result
                    .as_ref()
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_default();
                let abstract_text = format!("{} 工具执行失败", tool_name);
                let detail_text = format!(
                    "工具: {}\n参数: {}\n错误: {}\n",
                    tool_name, arguments, err_msg
                );
                // Use a fixed session_id — recordings are later aggregated
                // by RuleSuggester across all sessions.
                if let Err(e) = recorder
                    .record_with_kind(
                        &abstract_text,
                        &detail_text,
                        "tool-execution",
                        crate::scheduler::tasks::rule_recorder::FailureKind::Logic,
                    )
                    .await
                {
                    tracing::debug!(
                        tool = %tool_name,
                        error = %e,
                        "RuleRecorder 记录失败（将被 RuleSuggester 后续聚合）"
                    );
                }
            }
        }
    }
}

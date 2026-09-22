//! 工具执行管线：可插拔瀑布（DSH tools/pre-execute → guard → execute → post-execute → result 吸收）。
//!
//! 天演吸收 DSH 插件化思想的最小核心（dsh-comparison-2026-08.md §3.3 A1/A4）：
//! 工具执行不再只是固定管线（SecurityPolicy → ApprovalWorkflow → VerificationGate），
//! 而是可组合的监听器瀑布。管线语义：
//!
//! 1. **pre-execute 监听器**（ToolPreExecuteListener）：执行前决策，允许/拒绝。
//!    按注册顺序依次调用，**任一拒绝即中止**（fail-closed）——后续监听器无法
//!    把拒绝改回允许。
//! 2. **单调守卫**（ToolGuard）：只允许拒绝、不允许放行（DSH monotonic guard
//!    思想）。守卫在 pre-execute 之后执行，返回 Some(原因) 即拒绝；守卫
//!    本身**没有**"允许"路径，因此监听器顺序永远无法把守卫的拒绝反转成放行
//!    ——拒绝是单调的。
//! 3. **post-execute 监听器**（ToolPostExecuteListener）：执行后观察/改写结果。
//!    与 DSH 的 post-execute（可改结果）与 result（只读观察）两个阶段合并，
//!    Rust 形态下用一个可改写结果的监听器即可表达（DSH 拆分两者是为了
//!    TypeScript 流水线的可重放性，天演无此需求）。

use async_trait::async_trait;
use serde_json::Value;

use crate::common::error::TianyanError;
use crate::model::types::ToolCall;

/// 工具执行前置决策（DSH PreDecision 吸收）。
///
/// - Allow：放行，继续后续监听器与执行。
/// - Deny(reason)：拒绝执行，整条管线立即中止（fail-closed）。
/// - Ask(reason)：需要人类确认。当前天演形态下"询问"由审批工作流
///   （ApprovalWorkflow，执行器内 ensure_approved）承担，pre-execute
///   监听器返回 Ask 时按 Deny 处理并附加说明——第三方监听器不应
///   自行实现交互式询问。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreDecision {
    /// 放行。
    Allow,
    /// 拒绝（携带原因）。
    Deny(String),
    /// 需要人类确认（携带原因；当前映射为拒绝语义）。
    Ask(String),
}

impl PreDecision {
    /// 是否放行。
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }

    /// 提取拒绝/询问原因（Allow 时为 None）。
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Allow => None,
            Self::Deny(reason) | Self::Ask(reason) => Some(reason),
        }
    }
}

/// 工具执行前置监听器（DSH tools/pre-execute 吸收）。
///
/// 在工具实际执行前被调用，可放行或拒绝。拒绝必须**只减权限**：
/// 监听器返回 Deny 后整条管线中止，不存在后续监听器"重新允许"的路径。
#[async_trait]
pub trait ToolPreExecuteListener: Send + Sync {
    /// 前置决策回调。
    ///
    /// call：本次工具调用；session_id：归属会话；subagent：是否子 agent
    /// 上下文（ADR-011：子 agent 无交互审批）。返回 PreDecision。
    async fn on_pre_execute(
        &self,
        call: &ToolCall,
        session_id: &str,
        subagent: bool,
    ) -> PreDecision;
}

/// 单调守卫（DSH monotonic guard 吸收，A4）。
///
/// 与 ToolPreExecuteListener 的区别：守卫**只能拒绝**。返回值
/// Option<String>——None 表示"不拒绝"（继续），Some(原因) 表示拒绝。
/// 守卫没有"允许"分支，因此无论监听器以什么顺序注册、执行，守卫的拒绝
/// 都无法被任何后续步骤反转——拒绝单调，杜绝"监听器顺序把拒绝变允许"竞态。
#[async_trait]
pub trait ToolGuard: Send + Sync {
    /// 守卫检查：返回拒绝原因则拒绝执行；返回 None 则放行。
    async fn guard(&self, call: &ToolCall, session_id: &str, subagent: bool) -> Option<String>;
}

/// 工具执行后置监听器（DSH tools/post-execute + tools/result 吸收）。
///
/// 在工具执行完成后被调用，可观察结果（审计）或改写结果（如给大输出挂
/// spill locator、把错误包装成统一格式）。**不得**用于改变"已执行"的事实。
#[async_trait]
pub trait ToolPostExecuteListener: Send + Sync {
    /// 后置回调。
    ///
    /// call：本次工具调用；session_id：归属会话；result：可改写的执行结果；
    /// elapsed：执行耗时（完整精度，内部按需取 ms/µs）。
    async fn on_post_execute(
        &self,
        call: &ToolCall,
        session_id: &str,
        result: &mut Result<Value, TianyanError>,
        elapsed: std::time::Duration,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PreDecision 语义：reason 提取与 is_allow。
    #[test]
    fn test_pre_decision_semantics() {
        assert!(PreDecision::Allow.is_allow());
        assert_eq!(PreDecision::Allow.reason(), None);
        assert!(!PreDecision::Deny("不安全".into()).is_allow());
        assert_eq!(PreDecision::Deny("不安全".into()).reason(), Some("不安全"));
        // Ask 当前映射为拒绝语义（reason 可读）
        assert!(!PreDecision::Ask("需要确认".into()).is_allow());
        assert_eq!(
            PreDecision::Ask("需要确认".into()).reason(),
            Some("需要确认")
        );
    }

    // ── 管线集成测试（经 ToolRegistry.execute_single 验证 A1/A4 语义）──

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use crate::agent::tool_registry::ToolRegistry;
    use crate::common::types::tool::{FunctionCall, ToolCallType};
    use crate::executor::SecurityPolicy;
    use crate::model::types::ToolCall;

    /// 构造一个简单工具调用（read_file 路径不存在，执行必然失败——
    /// 用于区分"被管线拒绝"与"正常执行失败"）。
    fn make_call() -> ToolCall {
        ToolCall {
            id: "call_pipeline".into(),
            call_type: ToolCallType::Function,
            function: FunctionCall {
                name: "read_file".into(),
                arguments: r#"{"path":"/nonexistent/pipeline-test"}"#.into(),
            },
        }
    }

    /// 计数 pre-execute 监听器（记录调用次数，可按工具名拒绝）。
    struct CountingPreListener {
        calls: Arc<AtomicUsize>,
        deny: bool,
    }

    #[async_trait]
    impl ToolPreExecuteListener for CountingPreListener {
        async fn on_pre_execute(
            &self,
            _call: &ToolCall,
            _session_id: &str,
            _subagent: bool,
        ) -> PreDecision {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.deny {
                PreDecision::Deny("pre 拒绝".into())
            } else {
                PreDecision::Allow
            }
        }
    }

    /// 计数 post-execute 监听器（记录调用次数与观测到的结果）。
    struct CountingPostListener {
        calls: Arc<AtomicUsize>,
        observed_ok: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ToolPostExecuteListener for CountingPostListener {
        async fn on_post_execute(
            &self,
            _call: &ToolCall,
            _session_id: &str,
            result: &mut Result<Value, TianyanError>,
            _elapsed: std::time::Duration,
        ) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if result.is_ok() {
                self.observed_ok.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    /// 单调守卫：只按配置拒绝特定工具，其余放行。
    struct DenyGuard {
        deny_tool: &'static str,
    }

    #[async_trait]
    impl ToolGuard for DenyGuard {
        async fn guard(
            &self,
            call: &ToolCall,
            _session_id: &str,
            _subagent: bool,
        ) -> Option<String> {
            if call.function.name == self.deny_tool {
                Some(format!("守卫拒绝 {}", call.function.name))
            } else {
                None
            }
        }
    }

    /// P1-31 回归：并行工具结果按 `calls` 原序返回（完成顺序不确定，
    /// 但落库/上下文/推送顺序必须稳定——否则打掉前缀缓存）。
    #[tokio::test]
    async fn test_execute_parallel_preserves_call_order() {
        struct SlowFirstGuard;
        #[async_trait]
        impl ToolPreExecuteListener for SlowFirstGuard {
            async fn on_pre_execute(
                &self,
                call: &ToolCall,
                _session_id: &str,
                _subagent: bool,
            ) -> PreDecision {
                // 首个调用（id=slow）延迟 300ms → 并行完成顺序变成 fast 先完成
                if call.id == "slow" {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                }
                PreDecision::Allow
            }
        }

        let mut registry = ToolRegistry::new(SecurityPolicy::default());
        registry.register_pre_execute_listener(Arc::new(SlowFirstGuard));
        let make = |id: &str| ToolCall {
            id: id.into(),
            call_type: ToolCallType::Function,
            function: FunctionCall {
                name: "read_file".into(),
                arguments: r#"{"path":"/nonexistent/order-test"}"#.into(),
            },
        };
        let results = registry
            .execute_parallel(&[make("slow"), make("fast")], "s", false)
            .await;
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].0, "slow",
            "结果必须按 calls 原序（不得按完成顺序）"
        );
        assert_eq!(results[1].0, "fast");
    }

    /// A1：pre-execute 监听器拒绝时，工具不执行、post-execute 不触发（fail-closed）。
    #[tokio::test]
    async fn test_pre_execute_deny_aborts_pipeline() {
        let mut registry = ToolRegistry::new(SecurityPolicy::default());
        let pre_calls = Arc::new(AtomicUsize::new(0));
        let post_calls = Arc::new(AtomicUsize::new(0));
        registry.register_pre_execute_listener(Arc::new(CountingPreListener {
            calls: pre_calls.clone(),
            deny: true,
        }));
        registry.register_post_execute_listener(Arc::new(CountingPostListener {
            calls: post_calls.clone(),
            observed_ok: Arc::new(AtomicUsize::new(0)),
        }));
        let results = registry.execute_parallel(&[make_call()], "s", false).await;
        let err = results[0].1.as_ref().unwrap_err().to_string();
        assert!(err.contains("pre 拒绝"), "错误应来自监听器拒绝: {err}");
        assert_eq!(pre_calls.load(Ordering::SeqCst), 1, "pre 监听器应被调用");
        assert_eq!(post_calls.load(Ordering::SeqCst), 0, "拒绝后 post 不应触发");
    }

    /// A1：pre-execute 全部放行时，工具执行（失败也算执行），post 观察结果。
    #[tokio::test]
    async fn test_pre_allow_reaches_execution_and_post() {
        let mut registry = ToolRegistry::new(SecurityPolicy::default());
        let pre_calls = Arc::new(AtomicUsize::new(0));
        let post_calls = Arc::new(AtomicUsize::new(0));
        registry.register_pre_execute_listener(Arc::new(CountingPreListener {
            calls: pre_calls.clone(),
            deny: false,
        }));
        registry.register_post_execute_listener(Arc::new(CountingPostListener {
            calls: post_calls.clone(),
            observed_ok: Arc::new(AtomicUsize::new(0)),
        }));
        let results = registry.execute_parallel(&[make_call()], "s", false).await;
        // read_file 路径不存在 → 执行失败（真实执行错误，非管线拒绝）
        assert!(results[0].1.is_err(), "应到达执行阶段并因路径不存在失败");
        assert_eq!(pre_calls.load(Ordering::SeqCst), 1);
        assert_eq!(post_calls.load(Ordering::SeqCst), 1, "执行后 post 应触发");
    }

    /// A4：单调守卫拒绝时管线中止；守卫放行时执行继续。
    #[tokio::test]
    async fn test_guard_deny_is_monotonic() {
        let mut registry = ToolRegistry::new(SecurityPolicy::default());
        // 守卫拒绝 read_file——无论其他监听器怎么放行，都到不了执行
        registry.register_guard(Arc::new(DenyGuard {
            deny_tool: "read_file",
        }));
        let results = registry.execute_parallel(&[make_call()], "s", false).await;
        let err = results[0].1.as_ref().unwrap_err().to_string();
        assert!(err.contains("守卫拒绝"), "错误应来自守卫: {err}");

        // 守卫放行其他工具（unknown 工具 → 未知工具错误，证明过了守卫）
        let call = ToolCall {
            id: "call_other".into(),
            call_type: ToolCallType::Function,
            function: FunctionCall {
                name: "no_such_tool".into(),
                arguments: "{}".into(),
            },
        };
        let results = registry.execute_parallel(&[call], "s", false).await;
        let err = results[0].1.as_ref().unwrap_err().to_string();
        assert!(err.contains("未知工具"), "守卫放行后应到达执行阶段: {err}");
    }

    /// A4 单调性：守卫拒绝后，post-execute 观察到的仍是拒绝错误（无"重新允许"路径）。
    #[tokio::test]
    async fn test_guard_deny_reaches_post_with_denial() {
        let mut registry = ToolRegistry::new(SecurityPolicy::default());
        registry.register_guard(Arc::new(DenyGuard {
            deny_tool: "read_file",
        }));
        let post_calls = Arc::new(AtomicUsize::new(0));
        registry.register_post_execute_listener(Arc::new(CountingPostListener {
            calls: post_calls.clone(),
            observed_ok: Arc::new(AtomicUsize::new(0)),
        }));
        let results = registry.execute_parallel(&[make_call()], "s", false).await;
        // 注意：守卫拒绝发生在执行前，post 不应触发（拒绝即中止，无结果可观察）
        assert!(results[0].1.is_err());
        assert_eq!(
            post_calls.load(Ordering::SeqCst),
            0,
            "守卫拒绝后 post 不应触发"
        );
    }
}

//! 审批模块测试（生产实现已迁至 `plugins/policy-approval`，其 23 个用例随实现走）。
//!
//! 这里只剩**留在 core 的纯函数**的用例——`is_user_confirmation` 是
//! `executor/approval/types.rs` 的定义，`tool_registry/agent_ops.rs:382`（用户回答解析）
//! 在用它，与审批实现无关。

use super::*;

#[test]
fn test_is_user_confirmation() {
    assert!(is_user_confirmation("允许"));
    assert!(is_user_confirmation("可以，继续吧"));
    assert!(is_user_confirmation("好的，同意执行"));
    assert!(is_user_confirmation("yes, go ahead"));
    assert!(!is_user_confirmation("不允许"));
    assert!(!is_user_confirmation("拒绝执行"));
    assert!(!is_user_confirmation("不可以"));
    assert!(!is_user_confirmation("no"));
    assert!(!is_user_confirmation(""));
    assert!(!is_user_confirmation("请不要执行"));
}

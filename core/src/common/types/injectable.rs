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
///
/// `PartialEq` 用于「前缀是否实际变化」判定（工作集写收口 ③：无关字段更新
/// 不得导致前缀白失效、打掉 prompt 缓存）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InjectableContext {
    /// 智能体核心人格（soul.md）。
    pub soul: String,
    /// 项目指令（会话绑定工作目录下的 AGENTS.md；无则空）。
    ///
    /// 会话首次加载上下文时读取一次（会话绑定工作目录后不变），
    /// 随快照持久化（旧快照缺省空，向后兼容）。
    #[serde(default)]
    pub project_instructions: String,
    /// 会话绑定工作目录（`None` = 未绑定）。
    ///
    /// 渲染成系统前缀里的「## 当前环境」段，让模型知道相对路径的基准。
    /// 工作目录在会话生命周期内**不变**（用户确认「只要确定就不会更换」），
    /// 故可安全进前缀、不引起 prompt 缓存抖动。旧快照缺该字段 → `None`。
    #[serde(default)]
    pub working_directory: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// serde 向后兼容：旧快照 JSON（无 `working_directory` 字段）必须能反序列化，
    /// 且该字段取 `None`（不注入环境段、不报错）。
    ///
    /// 判别力：去掉字段上的 `#[serde(default)]` 后本用例直接反序列化失败
    /// （`missing field 'working_directory'`）——升级后读旧会话即崩。
    #[test]
    fn test_deserialize_legacy_snapshot_without_working_directory() {
        let legacy = r#"{
            "soul": "you are tianyan",
            "project_instructions": "项目规范",
            "rules_and_experiences": ["先读后写"],
            "memories": ["用户偏好 Rust"],
            "last_updated": "2026-01-02T03:04:05Z"
        }"#;
        let ctx: InjectableContext = serde_json::from_str(legacy).unwrap();
        assert_eq!(ctx.soul, "you are tianyan");
        assert_eq!(ctx.project_instructions, "项目规范");
        assert_eq!(ctx.rules_and_experiences, vec!["先读后写".to_string()]);
        assert_eq!(ctx.memories, vec!["用户偏好 Rust".to_string()]);
        assert_eq!(
            ctx.working_directory, None,
            "旧快照缺该字段 → None（不注入环境段）"
        );
    }

    /// 新快照往返：带工作目录写入后读回一致（字段真的落了库，不是只在内存里）。
    #[test]
    fn test_roundtrip_with_working_directory() {
        let ctx = InjectableContext {
            soul: "soul".to_string(),
            working_directory: Some("F:\\work\\git\\tianyan".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let back: InjectableContext = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.working_directory.as_deref(),
            Some("F:\\work\\git\\tianyan")
        );
        assert_eq!(back, ctx);
    }
}

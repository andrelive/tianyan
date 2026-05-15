//! 上下文组装模块。
//!
//! 将 ContextWindow 与 SessionState 中的动态数据（对话历史、执行历史）
//! 组装为最终发送给 LLM 的 prompt 字符串。

use crate::common::types::Message;
use crate::context::types::ContextWindow;

/// 从 ContextWindow 和会话状态中的动态数据构建最终 prompt。
pub fn assemble_prompt(
    window: &ContextWindow,
    conversation: &[Message],
    current_input: &str,
) -> String {
    let mut prompt = String::new();

    prompt.push_str(&window.system_prompt);
    prompt.push_str("\n\n---\n\n");

    if !conversation.is_empty() {
        prompt.push_str("## 对话历史\n\n");
        for msg in conversation {
            let role = match msg.role {
                crate::common::types::MessageRole::User => "用户",
                crate::common::types::MessageRole::Assistant => "助手",
                crate::common::types::MessageRole::System => "系统",
                crate::common::types::MessageRole::Tool => "工具",
            };
            prompt.push_str(&format!("{}: {}\n\n", role, msg.content));
        }
        prompt.push_str("---\n\n");
    }

    if let Some(ref summary) = window.summary {
        prompt.push_str("## 历史对话摘要\n");
        prompt.push_str(summary);
        prompt.push_str("\n\n");
    }

    if !window.retrieved.is_empty() {
        let (memories, knowledge): (Vec<_>, Vec<_>) = window
            .retrieved
            .iter()
            .partition(|r| r.category == "memory");

        if !memories.is_empty() {
            prompt.push_str("## 相关历史记忆\n\n");
            for m in &memories {
                if let Some(ref content) = m.content {
                    prompt.push_str(&format!("- {}\n", content));
                }
            }
            prompt.push('\n');
        }

        if !knowledge.is_empty() {
            prompt.push_str("## 检索到的相关知识\n\n");
            for (i, k) in knowledge.iter().enumerate() {
                if let Some(ref content) = k.content {
                    prompt.push_str(&format!("{}. {}\n", i + 1, content));
                }
            }
            prompt.push('\n');
        }
    }

    prompt.push_str("## 当前输入\n\n");
    prompt.push_str(current_input);
    prompt.push_str("\n\n## 你的执行计划\n");

    prompt
}

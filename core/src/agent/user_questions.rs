//! 用户问题服务（对齐 DSH user-questions seam）：ask_user 工具执行挂起等待用户回答。
//!
//! 回答经独立 HTTP 端点（server 层）提交，按会话索引等待通道；
//! 同一会话同一时刻只有一个活动轮（turn 锁串行化），单通道即可。
//! 工具调用事件（ToolCall chunk）已携带问题（arguments JSON），前端据此拉起
//! 组件——本服务只负责「等待 + 提交」，不负责推送。

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{oneshot, Mutex};

use crate::common::error::{Result, TianyanError};

/// 用户问题服务。
#[derive(Clone, Default)]
pub struct UserQuestionService {
    /// 按会话索引的回答等待通道（回答提交端点 → 工具执行恢复）。
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>>,
}

impl UserQuestionService {
    /// 创建服务。
    pub fn new() -> Self {
        Self::default()
    }

    /// 挂起等待用户回答（ask_user 工具执行路径）。
    ///
    /// - `session_id` — 归属会话（回答提交端点按此索引）
    /// - `cancelled` — 同步取消检查（主循环停止时不再等待，返回取消错误）
    ///
    /// **无超时**：多会话场景用户可能稍后才看到提问——等待直到回答提交或
    /// 主动取消（200ms 轮询取消标志，ADR-036 取消语义保留）；等待期间
    /// 轮锁被本 turn 持有，本会话不受理新轮，等待解除（提交/取消）即恢复。
    pub async fn ask(
        &self,
        session_id: &str,
        cancelled: impl Fn() -> bool,
    ) -> Result<serde_json::Value> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            // 同一会话不应有并发等待（turn 锁保证）；残留通道覆盖（防御）
            pending.insert(session_id.to_string(), tx);
        }
        let mut rx = rx;
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(200));
        let answer = loop {
            tokio::select! {
                answer = &mut rx => {
                    match answer {
                        Ok(value) => break Ok(value),
                        Err(_) => break Err(TianyanError::Custom(
                            "tool: 执行失败：追问回答通道关闭".to_string(),
                        )),
                    }
                }
                _ = interval.tick() => {
                    if cancelled() {
                        break Err(TianyanError::Custom(
                            "tool: 执行失败：追问等待被取消".to_string(),
                        ));
                    }
                }
            }
        };
        // 等待解除（提交/取消）后清理登记；remove 必须在等待完成之后
        // （放在循环前会让 submit 找不到通道，回答永远送不进）
        self.pending.lock().await.remove(session_id);
        answer
    }

    /// 提交用户回答（server 回答端点调用）：按会话找到等待通道并发送。
    ///
    /// 返回是否找到等待中的追问（无等待时返回 false，不报错——
    /// 前端在无追问时提交是幂等空操作）。
    pub async fn submit(&self, session_id: &str, answers: serde_json::Value) -> bool {
        let tx = self.pending.lock().await.remove(session_id);
        match tx {
            Some(tx) => {
                let _ = tx.send(answers);
                true
            }
            None => false,
        }
    }
}
